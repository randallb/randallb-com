use super::*;

#[test]
fn coworker_configuration_is_queryable_on_targets_and_session_actor() {
    let directory = tempfile::tempdir().unwrap();
    let server = start(
        runtime(directory.path()),
        "cwp://example.test/runtime".into(),
        directory.path(),
        crate::idle::Activity::default(),
        Arc::new(|_| Ok(())),
        tokio::sync::watch::channel(()).1,
    )
    .unwrap();
    let endpoint = &server.endpoint;
    let response = query(
        &client(endpoint),
        endpoint,
        r#"{
      runtime {
        actor { ... on Coworker { category bindings { id } defaultBinding { id } } }
        messageTargets {
          handle category
          bindings { id adapter { id name } model { provider model reasoningEffort } }
          defaultBinding { id adapter { id name } model { provider model reasoningEffort } }
        }
      }
    }"#,
        json!({}),
    );
    assert!(response.get("errors").is_none(), "{response}");
    let targets = response["data"]["runtime"]["messageTargets"]
        .as_array()
        .unwrap();
    let helper = targets
        .iter()
        .find(|value| value["handle"] == "@helper")
        .unwrap();
    assert_eq!(helper["category"], "AI");
    assert_eq!(helper["defaultBinding"]["adapter"]["name"], "Codex");
    assert!(helper["defaultBinding"]["model"]["model"].is_null());
    assert!(helper["defaultBinding"]["model"]["reasoningEffort"].is_null());
    assert!(helper["bindings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|binding| binding["id"] == helper["defaultBinding"]["id"]));
    assert_eq!(response["data"]["runtime"]["actor"]["category"], "HUMAN");
}

#[test]
fn selection_shares_configuration_across_distinct_coworkers() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = runtime(directory.path());
    let identity = runtime
        .catalog()
        .unwrap()
        .known_actors
        .into_iter()
        .find(|actor| actor.handle == "@helper")
        .unwrap();
    let coworker = super::super::messages::Coworker::from(identity.clone());
    let mut service = ReadService {
        configurations: Default::default(),
        action_titles: Default::default(),
        publications: Default::default(),
        actor_ref: runtime.catalog().unwrap().acting_actor.actor_ref,
        runtime,
        runtime_ref: "cwp://example.test/runtime".into(),
        owner: None,
        ingress: "cwp://example.test/ingress".into(),
        budget: Some(super::super::budget::ReadBudget::new(Duration::from_secs(
            5,
        ))),
        reads: Arc::new(Semaphore::new(8)),
        publisher: Arc::new(|_| Ok(())),
    };
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let first = super::super::coworkers::read_configuration(&coworker, service.clone())
            .await
            .unwrap()
            .unwrap();
        let action = service
            .runtime
            .catalog()
            .unwrap()
            .actions
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(
            service.action_title(&action.activation).await.unwrap(),
            Some(action.title.clone())
        );
        service.budget = Some(super::super::budget::ReadBudget::new(Duration::ZERO));
        assert_eq!(
            service
                .clone()
                .action_title(&action.activation)
                .await
                .unwrap(),
            Some(action.title)
        );
        assert!(service.fresh_selection().action_titles.get().is_none());

        let second =
            super::super::coworkers::read_configuration(&coworker.clone(), service.clone())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(first.default_binding_ref, second.default_binding_ref);
        assert_eq!(first.bindings.len(), second.bindings.len());
        let fresh = super::super::messages::Coworker::from(identity.clone());
        assert!(
            super::super::coworkers::read_configuration(&fresh, service.clone())
                .await
                .unwrap()
                .is_some()
        );
        let other =
            super::super::messages::Coworker::from(service.runtime.catalog().unwrap().acting_actor);
        assert!(
            super::super::coworkers::read_configuration(&other, service.clone())
                .await
                .unwrap()
                .is_some()
        );
        service.configurations = Default::default();
        let fresh = super::super::messages::Coworker::from(identity.clone());
        assert!(super::super::coworkers::read_configuration(&fresh, service)
            .await
            .is_err());
    });
}
