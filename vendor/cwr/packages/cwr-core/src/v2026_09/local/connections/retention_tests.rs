use super::*;
use crate::v2026_09::conversation::*;

#[test]
fn pre_input_plan_invocations_keep_resolved_parameters_without_reading_stores() {
    for (adapter, params) in [
        (
            SH_ADAPTER_REF,
            json!({"env":{"PROFILE":"local-json","TOKEN":"supplied"},
                "command":"managed-input:status", "runner":["cwr-local-key:tool"],
                "api_key":"managed-input:ordinary-field"}),
        ),
        (
            OPEN_RESPONSES_ADAPTER_REF,
            json!({"api_key":"supplied",
            "env":{"EXAMPLE":"managed-input:status"}, "prompt":"cwr-local-key:literal"}),
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let mut route = ResolvedProcessAdapterRoute::new(
            adapter,
            root.path(),
            root.path(),
            json!({"runner":["managed-input:tool"]}),
            params.clone(),
            json!({"nested":{"literal":"cwr-local-key:actor"}}),
        );
        let invocation =
            route.project_invocation(INVOCATION, "run--command", json!({}), json!({}), json!({}));
        let mut old = serde_json::to_value(invocation).unwrap();
        old.as_object_mut().unwrap().remove("local_inputs");
        let mut restored: AdapterInvocation = serde_json::from_value(old).unwrap();
        assert!(restored.local_inputs.is_none());
        // Old work must not acquire new dependencies from today's graph.
        fs::write(root.path().join("CWR.toml"), "unavailable source").unwrap();
        deliver(&root.path().join("CWR.toml"), &mut route, &mut restored).unwrap();
        assert!(
            handoff::prepare_with(&mut route, &mut restored, &Default::default(), |_| panic!(
                "resolved legacy work must not read credentials"
            ),)
            .unwrap()
            .is_none()
        );
        assert_eq!(restored.integration_params, params);
    }
}

#[test]
fn pre_input_plan_invocations_with_unresolved_slots_fail_at_both_checks() {
    for marker in ["managed-input:token", "cwr-local-key:old-generation"] {
        for adapter in [SH_ADAPTER_REF, OPEN_RESPONSES_ADAPTER_REF] {
            for route_only in [false, true] {
                let params = |value| {
                    if adapter == SH_ADAPTER_REF {
                        json!({"env":{"TOKEN":value}})
                    } else {
                        json!({"api_key":value})
                    }
                };
                let root = tempfile::tempdir().unwrap();
                let mut route = ResolvedProcessAdapterRoute::new(
                    adapter,
                    root.path(),
                    root.path(),
                    json!({}),
                    params(if route_only { marker } else { "resolved" }),
                    json!({}),
                );
                let mut invocation = route.project_invocation(
                    INVOCATION,
                    "run--command",
                    json!({}),
                    json!({}),
                    json!({}),
                );
                invocation.integration_params =
                    params(if route_only { "resolved" } else { marker });
                let mut old = serde_json::to_value(invocation).unwrap();
                old.as_object_mut().unwrap().remove("local_inputs");
                let mut restored: AdapterInvocation = serde_json::from_value(old).unwrap();
                assert!(deliver(&root.path().join("CWR.toml"), &mut route, &mut restored).is_err());
                assert!(handoff::prepare_with(
                    &mut route,
                    &mut restored,
                    &Default::default(),
                    |_| panic!("unapproved work must not read credentials"),
                )
                .is_err());
            }
        }
    }
}

#[test]
fn database_restart_retains_dependency_identity_without_managed_values() {
    let _serial = environment::TEST_ENVIRONMENT_LOCK.lock().unwrap();
    let root = fixture();
    let _env = EnvironmentGuard::isolate(root.path());
    let store = setup(root.path());
    let graph = resolve_local_graph(&root.path().join("CWR.toml")).unwrap();
    let binding = &graph.actor_integrations[0];
    let person = graph
        .actors
        .iter()
        .find(|actor| actor.coworker_slug == "person")
        .unwrap();
    let path = root.path().join("retained.sqlite");
    let runtime = "cwp://example.test/runtimes/retention";
    let ingress = ConversationIngress {
        owner_context_ref: None,
        file_refs: Vec::new(),
        integration_ref: binding.integration_ref.clone(),
        provider_event_ref: "retention".into(),
        author_actor_ref: person.actor_ref.clone(),
        conversation: ConversationIngressConversation::Create,
        addressing: ConversationIngressAddressing::Addressed {
            actor_refs: vec![binding.actor_ref.clone()],
        },
        text: "Use the configured inputs".into(),
    };
    let mut database = ConversationAdmissionStore::open(&path, runtime).unwrap();
    let ConversationAdmissionOutcome::Started(admitted) = database
        .admit(&graph, &ingress, Some(&binding.actor_integration_ref))
        .unwrap()
    else {
        panic!("expected admission")
    };
    let reference = admitted.record.invocation_ref.unwrap();
    let expected = admitted.invocation.as_ref().unwrap().local_inputs.clone();
    assert_eq!(expected.as_ref().unwrap().dependencies.len(), 2);
    let assert_no_values = || {
        for suffix in ["", "-wal", "-shm"] {
            let bytes = fs::read(format!("{}{suffix}", path.display())).unwrap_or_default();
            for value in [SECRET, VARIABLE] {
                assert!(
                    !bytes
                        .windows(value.len())
                        .any(|bytes| bytes == value.as_bytes()),
                    "managed value in {suffix}"
                );
            }
        }
    };
    assert_no_values();
    drop(database);
    let mut database = ConversationAdmissionStore::open(&path, runtime).unwrap();
    let ConversationInvocationClaimOutcome::Claimed(claimed) =
        database.claim_invocation(&reference, 100).unwrap()
    else {
        panic!("expected claim")
    };
    let mut invocation = claimed.adapter_invocation;
    assert_eq!(invocation.local_inputs, expected);
    let mut route = invocation.retained_route.clone().unwrap();
    drop(
        handoff::prepare_with(&mut route, &mut invocation, &Default::default(), |id| {
            store.get(id)?.ok_or_else(|| "missing".into())
        })
        .unwrap(),
    );
    assert_eq!(invocation.integration_params["env"]["RESEARCH_KEY"], SECRET);
    assert_no_values();
}
