use super::*;
use crate::graphql::budget::{blocking, ReadBudget};
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn query_fanout_queues_instead_of_rejecting_the_ninth_timeline() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = runtime(directory.path());
    for key in 0..20 {
        runtime
            .submit_text_as(
                "cwp://example.test/ingress",
                key.to_string(),
                crate::test_refs::actor_ref("person"),
                None,
                "A message",
            )
            .unwrap();
    }
    let server = start(
        runtime,
        "cwp://example.test/runtime".into(),
        directory.path(),
        crate::idle::Activity::default(),
        Arc::new(|_| Ok(())),
        tokio::sync::watch::channel(()).1,
    )
    .unwrap();
    let result = query(
        &client(&server.endpoint),
        &server.endpoint,
        "{runtime{conversations(first:20){edges{node{legacyTimeline{edges{node{...on Message{id}}}}}}}}}",
        json!({}),
    );
    assert!(result.get("errors").is_none(), "{result}");
    assert_eq!(
        result["data"]["runtime"]["conversations"]["edges"]
            .as_array()
            .unwrap()
            .len(),
        20
    );
}

#[test]
fn queued_reads_use_the_original_deadline_without_starting_after_expiry() {
    let executor = tokio::runtime::Runtime::new().unwrap();
    executor.block_on(async {
        let budget = ReadBudget::new(Duration::from_millis(150));
        let permits = Arc::new(Semaphore::new(1));
        let held = permits.clone().acquire_owned().await.unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let called = calls.clone();
        let started = std::time::Instant::now();
        let error = blocking(permits.clone(), budget.clone(), move || {
            called.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .await
        .unwrap_err();
        assert_eq!(
            error.extensions.unwrap().get("code").unwrap(),
            &async_graphql::Value::from("READ_BUDGET_EXCEEDED")
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        drop(held);
        assert!(blocking(permits, budget, || Ok(())).await.is_err());
    });
}

#[test]
fn cancellation_stops_registered_database_reads_and_releases_worker_capacity() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = runtime(directory.path());
    let executor = tokio::runtime::Runtime::new().unwrap();
    executor.block_on(async {
        let permits = Arc::new(Semaphore::new(1));
        let budget = ReadBudget::new(Duration::from_secs(10));
        let worker_budget = budget.clone();
        let (registered_tx, registered_rx) = tokio::sync::oneshot::channel();
        let (stopped_tx, stopped_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(blocking(permits.clone(), budget.clone(), move || {
            let result = runtime.conversation_records(
                &cwr::v2026_09::conversation::ConversationRecordReadRequest::Resource {
                    resource_ref: "cwp://example.test/missing".into(),
                },
                worker_budget.deadline(),
                || worker_budget.cancelled(),
                |read| {
                    worker_budget.register(read);
                    let _ = registered_tx.send(());
                    // Deliberately hold the worker until its async owner cancels.
                    while !worker_budget.cancelled() {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                },
            );
            let _ = stopped_tx.send(());
            result.map(|_| ())
        }));
        registered_rx.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        tokio::time::timeout(Duration::from_secs(1), stopped_rx)
            .await
            .unwrap()
            .unwrap();
        let _permit = tokio::time::timeout(Duration::from_secs(1), permits.acquire_owned())
            .await
            .unwrap()
            .unwrap();
        assert!(budget.cancelled());
    });
}

#[test]
fn nested_invocation_hydration_cannot_reset_an_expired_selection_budget() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = mutations::fixture(directory.path());
    let admission = runtime
        .admit_action(None, "@worker--run--task", json!({"value":"test"}))
        .unwrap();
    let actor = runtime.catalog().unwrap().acting_actor.actor_ref;
    let service = ReadService {
        configurations: Default::default(),
        action_titles: Default::default(),
        publications: Default::default(),
        runtime,
        runtime_ref: "cwp://example.test/runtime".into(),
        actor_ref: actor,
        owner: None,
        ingress: "cwp://org.coworkerprotocol.local/integrations/graphql".into(),
        budget: Some(ReadBudget::new(Duration::ZERO)),
        reads: Arc::new(Semaphore::new(8)),
        publisher: Arc::new(|_| Ok(())),
    };
    assert!(crate::graphql::mutations::read_invocation(
        &service,
        admission.invocation_ref().unwrap()
    )
    .is_err());
    // A new snapshot gets a new deadline, without changing actor or owner scope.
    let fresh = service.fresh_selection();
    assert_eq!(fresh.actor_ref, service.actor_ref);
    assert_eq!(fresh.owner, service.owner);
    assert!(crate::graphql::mutations::read_invocation(
        &fresh,
        admission.invocation_ref().unwrap()
    )
    .unwrap()
    .is_some());
}

#[test]
fn subscription_gets_a_fresh_budget_after_its_first_snapshot_expires() {
    use super::subscriptions::{connect_session, read, send};
    let directory = tempfile::tempdir().unwrap();
    let runtime = runtime(directory.path());
    let (changed, observe) = tokio::sync::watch::channel(());
    let server = start(
        runtime.clone(),
        "cwp://example.test/runtime".into(),
        directory.path(),
        crate::idle::Activity::default(),
        Arc::new(|_| Ok(())),
        observe,
    )
    .unwrap();
    let endpoint = session_endpoint(&client(&server.endpoint), &server.endpoint, None);
    let mut socket = connect_session(&endpoint);
    send(
        &mut socket,
        json!({"id":"watch","type":"subscribe","payload":{"query":"subscription{runtime{conversations{totalCount edges{node{legacyTimeline{edges{node{...on Message{id}}}}}}}}}"}}),
    );
    assert_eq!(
        read(&mut socket)["payload"]["data"]["runtime"]["conversations"]["totalCount"],
        0
    );
    std::thread::sleep(Duration::from_secs(11));
    runtime
        .submit_text_as(
            "cwp://example.test/ingress",
            "later",
            &endpoint.actor_ref,
            None,
            "Later message",
        )
        .unwrap();
    changed.send_replace(());
    let next = read(&mut socket);
    assert!(next["payload"].get("errors").is_none(), "{next}");
    assert_eq!(
        next["payload"]["data"]["runtime"]["conversations"]["totalCount"],
        1
    );
    socket.close(None).unwrap();
}

#[test]
fn action_admission_waits_for_capacity_and_expired_waiters_do_not_admit() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = mutations::fixture(directory.path());
    let executor = tokio::runtime::Runtime::new().unwrap();
    executor.block_on(async {
        let permits = Arc::new(Semaphore::new(1));
        let service = ReadService {
            configurations: Default::default(),
            action_titles: Default::default(),
        publications: Default::default(),
            actor_ref: runtime.catalog().unwrap().acting_actor.actor_ref,
            runtime,
            runtime_ref: "cwp://example.test/runtime".into(),
            owner: None,
            ingress: "cwp://org.coworkerprotocol.local/integrations/graphql".into(),
            budget: Some(ReadBudget::new(Duration::from_secs(5))),
            reads: permits.clone(),
            publisher: Arc::new(|_| Ok(())),
        };
        let request = "mutation{invokeAction(url:\"cwp://example.test/runtime\",activation:\"@worker--run--task\",input:{value:\"queued\"},requestId:\"queued-action\"){__typename ...on ActionInvocation{id}}}";
        let schema = |service| async_graphql::Schema::build(
            crate::graphql::schema::Query,
            crate::graphql::mutations::Mutation::default(),
            async_graphql::EmptySubscription,
        ).data(service).finish();
        let held = permits.clone().acquire_owned().await.unwrap();
        let queued_schema = schema(service.clone());
        let task = tokio::spawn(async move { queued_schema.execute(request).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!task.is_finished());
        drop(held);
        let admitted = task.await.unwrap();
        assert!(admitted.errors.is_empty(), "{:?}", admitted.errors);
        assert_eq!(admitted.data.clone().into_json().unwrap()["invokeAction"]["__typename"], "AdmittedInvocation");
        let recovered = schema(service.clone()).execute(request).await;
        assert_eq!(admitted.data, recovered.data);
        let held = permits.clone().acquire_owned().await.unwrap();
        let mut expired = service.clone();
        expired.budget = Some(ReadBudget::new(Duration::from_millis(50)));
        let other = request.replace("queued-action", "expired-action");
        let response = schema(expired).execute(other.clone()).await;
        assert!(!response.errors.is_empty());
        drop(held);
        let response = schema(service).execute(other).await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
    });
}
