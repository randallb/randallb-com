use super::*;
use cwr::v2026_09::upstream::{UpstreamAction, UpstreamCoworker, UpstreamEvent};

#[test]
fn independent_viewers_and_reconnect_read_stopping_until_provider_settles() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = mutations::fixture(directory.path());
    let person = runtime.catalog().unwrap().acting_actor.actor_ref;
    let (events, receiver) = std::sync::mpsc::channel();
    let (stops, _stop_events) = std::sync::mpsc::sync_channel(1);
    let actor = UpstreamCoworker {
        actor_ref: "cwp://example.test/actors/remote".into(),
        coworker_ref: "cwp://example.test/coworkers/remote".into(),
        handle: "@remote".into(),
        name: "Remote".into(),
        actions: vec![UpstreamAction {
            action_name: "run--task".into(),
            title: "Run".into(),
            description: "Run".into(),
            input_schema: json!({"type":"object"}),
            output_schema: json!({"type":"object"}),
        }],
    };
    let session = runtime
        .register_upstream_provider(
            "owner",
            vec![actor.clone()],
            Arc::new(move |event| {
                events.send(event)?;
                Ok(())
            }),
            stops,
        )
        .unwrap();
    runtime
        .enable_upstream_coworker("owner", &session, &actor.actor_ref, &person, true)
        .unwrap();
    let work = runtime
        .admit_action_in_upstream_scope(
            "cwp://example.test/integrations/graphql",
            "remote-work",
            &person,
            None,
            "@remote--run--task",
            json!({}),
            Some("owner"),
        )
        .unwrap();
    let worker_runtime = runtime.clone();
    let worker_admission = work.clone();
    let worker = std::thread::spawn(move || {
        worker_runtime
            .execute_admission(&worker_admission, |_| {})
            .unwrap()
    });
    assert!(matches!(
        receiver.recv_timeout(Duration::from_secs(2)).unwrap(),
        UpstreamEvent::WorkRequested { .. }
    ));
    let server = start(
        runtime.clone(),
        "cwp://example.test/runtime".into(),
        directory.path(),
        crate::idle::Activity::default(),
        Arc::new(|_| Ok(())),
        tokio::sync::watch::channel(()).1,
    )
    .unwrap();
    let client = client(&server.endpoint);
    runtime
        .stop_invocation(work.invocation_ref().unwrap(), None)
        .unwrap();
    assert!(matches!(
        receiver.recv_timeout(Duration::from_secs(2)).unwrap(),
        UpstreamEvent::StopRequested { .. }
    ));
    worker.join().unwrap();
    let document = "query($ref:ID!){runtime{invocation(ref:$ref){__typename state finishedAt stopSettlement outcome{status} currentExecution{generation}}}}";
    for _ in 0..2 {
        let result = query(
            &client,
            &server.endpoint,
            document,
            json!({"ref":work.invocation_ref().unwrap()}),
        );
        assert!(result.get("errors").is_none(), "{result}");
        let invocation = &result["data"]["runtime"]["invocation"];
        assert_eq!(invocation["__typename"], "StoppingInvocation");
        assert_eq!(invocation["stopSettlement"], "PENDING");
        assert!(invocation["finishedAt"].is_null());
        assert!(invocation["outcome"].is_null());
        assert_eq!(invocation["currentExecution"]["generation"], "1");
    }
    assert!(runtime
        .acknowledge_upstream_stop("another-owner", work.invocation_ref().unwrap())
        .is_err());
    runtime
        .acknowledge_upstream_stop("owner", work.invocation_ref().unwrap())
        .unwrap();
    let result = query(
        &client,
        &server.endpoint,
        document,
        json!({"ref":work.invocation_ref().unwrap()}),
    );
    assert!(result.get("errors").is_none(), "{result}");
    let invocation = &result["data"]["runtime"]["invocation"];
    assert_eq!(invocation["state"], "stopped");
    assert_eq!(invocation["stopSettlement"], "CONFIRMED");
    assert_eq!(invocation["outcome"]["status"], 499);
    assert!(invocation["finishedAt"].is_string());
    assert!(invocation["currentExecution"].is_null());
}

#[test]
fn terminal_acknowledgment_publishes_settlement_after_the_worker_exits() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = mutations::fixture(directory.path());
    let (_changes, observe) = tokio::sync::watch::channel(());
    let server = start(
        runtime.clone(),
        "cwp://example.test/runtime".into(),
        directory.path(),
        crate::idle::Activity::default(),
        Arc::new(|_| Ok(())),
        observe,
    )
    .unwrap();
    let client = client(&server.endpoint);
    let provider = session_endpoint(&client, &server.endpoint, None);
    let (mut terminal, _, _) = sessions::terminal(&provider, "stopping");
    let admitted = query_session(
        &client,
        &provider,
        r#"mutation{invokeAction(url:"cwp://example.test/runtime",activation:"@tui--run--task",input:{},requestId:"stop-publication"){...on ActionInvocation{id}}}"#,
        json!({}),
    );
    assert!(admitted.get("errors").is_none(), "{admitted}");
    let invocation = admitted["data"]["invokeAction"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut task = runtime.try_schedule_execution().unwrap().unwrap();
    let worker = std::thread::spawn(move || {
        task.execute_with_model_output(&cwr::adapter::AdapterModelOutputMailbox::discard(), |_| {})
    });
    let event = subscriptions::read(&mut terminal);
    assert_eq!(
        event["payload"]["data"]["terminalEvents"]["__typename"],
        "TerminalWork"
    );
    runtime.stop_invocation(&invocation, None).unwrap();
    let event = subscriptions::read(&mut terminal);
    assert_eq!(
        event["payload"]["data"]["terminalEvents"]["__typename"],
        "TerminalStop"
    );
    worker.join().unwrap().unwrap();
    let viewer = session_endpoint(&client, &server.endpoint, None);
    let mut socket = subscriptions::connect_session(&viewer);
    subscriptions::send(
        &mut socket,
        json!({"id":"state","type":"subscribe","payload":{
            "query":"subscription($ref:ID!){runtime{invocation(ref:$ref){state stopSettlement finishedAt}}}",
            "variables":{"ref":invocation}
        }}),
    );
    let first = subscriptions::read(&mut socket);
    assert_eq!(
        first["payload"]["data"]["runtime"]["invocation"]["state"],
        "stopping"
    );
    let result = query_session(
        &client,
        &provider,
        "mutation($ref:ID!){acknowledgeTerminalStop(invocationRef:$ref)}",
        json!({"ref":invocation}),
    );
    assert!(result.get("errors").is_none(), "{result}");
    let settled = subscriptions::read(&mut socket);
    assert_eq!(
        settled["payload"]["data"]["runtime"]["invocation"]["state"], "stopped",
        "{settled}"
    );
    assert_eq!(
        settled["payload"]["data"]["runtime"]["invocation"]["stopSettlement"],
        "CONFIRMED"
    );
}
