use super::*;
use crate::v2026_09::conversation::{
    ConversationActivityEvent, ConversationAdmissionOutcome, ConversationAdmissionStore,
    ConversationIngress, ConversationIngressAddressing, ConversationIngressConversation,
    ConversationInvocationClaimOutcome,
};
use std::path::Path;

const RUNTIME_REF: &str = "cwp://example.test/runtime";

fn legacy_admission(interrupted: bool) -> (tempfile::TempDir, ResolvedCwrGraph, String) {
    let (temporary, mut graph, caller) = native_graph();
    let target = graph
        .actors
        .iter()
        .find(|actor| actor.actor_ref != caller)
        .unwrap()
        .actor_ref
        .clone();
    for binding in graph
        .actor_integrations
        .iter_mut()
        .filter(|binding| binding.actor_ref == target)
    {
        binding.available_authority = vec!["workspace.read".into()];
    }
    for route in graph
        .routes
        .iter_mut()
        .filter(|route| route.actor_ref == target)
    {
        route.available_authority = vec!["workspace.read".into()];
    }
    retain_native_tool_definitions(&mut graph).unwrap();
    let binding_ref = graph
        .actor_integrations
        .iter()
        .find(|binding| binding.actor_ref == target)
        .unwrap()
        .actor_integration_ref
        .clone();
    let state_path = temporary.path().join("cwpdb.sqlite3");
    let mut store = ConversationAdmissionStore::open(&state_path, RUNTIME_REF).unwrap();
    let admitted = store
        .admit(
            &graph,
            &ConversationIngress {
                owner_context_ref: None,
                file_refs: Vec::new(),
                integration_ref: "cwp://example.test/ingress".into(),
                provider_event_ref: "legacy".into(),
                author_actor_ref: caller,
                conversation: ConversationIngressConversation::Create,
                addressing: ConversationIngressAddressing::Action {
                    actor_ref: target,
                    activation: "@worker--echo--message".into(),
                    action_name: "echo--message".into(),
                    input: json!({}),
                },
                text: "Echo a message".into(),
            },
            Some(&binding_ref),
        )
        .unwrap();
    let ConversationAdmissionOutcome::Started(admitted) = admitted else {
        panic!("new invocation")
    };
    let invocation_ref = admitted.record.invocation_ref.unwrap();
    if interrupted {
        claim(&mut store, &invocation_ref);
    }
    drop(store);
    remove_new_snapshot_fields(&state_path);
    (temporary, graph, invocation_ref)
}

// Write the actual pre-upgrade representation, with both new fields absent.
fn remove_new_snapshot_fields(path: &Path) {
    crate::cwpdb::test_support::legacy_invocation::remove_execution_snapshot_fields(path).unwrap();
}

fn claim(
    store: &mut ConversationAdmissionStore,
    invocation_ref: &str,
) -> crate::v2026_09::conversation::ClaimedConversationInvocation {
    let ConversationInvocationClaimOutcome::Claimed(claimed) =
        store.claim_invocation(invocation_ref, 100).unwrap()
    else {
        panic!("queued invocation should remain runnable")
    };
    *claimed
}

fn retained_invocation(
    store: &ConversationAdmissionStore,
    invocation_ref: &str,
) -> AdapterInvocation {
    store
        .execution_conversation(invocation_ref)
        .unwrap()
        .activity
        .iter()
        .find_map(|activity| match &activity.event {
            ConversationActivityEvent::TurnStarted {
                invocation: Some(snapshot),
                ..
            } => Some(serde_json::from_value(snapshot.clone()).unwrap()),
            _ => None,
        })
        .unwrap()
}

#[test]
fn reopened_legacy_work_backfills_the_unchanged_route_and_freezes_it_for_retry() {
    for (interrupted, continuation) in [(false, false), (true, false), (true, true)] {
        let (temporary, mut graph, invocation_ref) = legacy_admission(interrupted);
        let state_path = temporary.path().join("cwpdb.sqlite3");
        let mut store = ConversationAdmissionStore::open(&state_path, RUNTIME_REF).unwrap();
        store.recover_interrupted_invocations().unwrap();
        let claimed = claim(&mut store, &invocation_ref);
        let record = claimed.claim.invocation;
        let mut invocation = claimed.adapter_invocation;
        assert!(invocation.retained_route.is_none());
        assert!(invocation.provider_fingerprint.is_none());
        let route = selected_process_route(&graph, &record, &mut invocation).unwrap();
        assert_eq!(
            invocation.route_fingerprint.as_deref(),
            Some(route.fingerprint().as_str())
        );
        assert_eq!(invocation.retained_route.as_ref(), Some(&route));
        if continuation {
            invocation.continuation = Some(json!({"provider_checkpoint":"existing"}));
        }
        store
            .retain_execution_invocation(&invocation_ref, &invocation)
            .unwrap();
        store
            .recover_interrupted_invocation(&invocation_ref)
            .unwrap();
        drop(store);

        let mut reopened = ConversationAdmissionStore::open(&state_path, RUNTIME_REF).unwrap();
        assert_eq!(
            retained_invocation(&reopened, &invocation_ref).retained_route,
            Some(route.clone())
        );
        let retry = claim(&mut reopened, &invocation_ref);
        let mut retried = retry.adapter_invocation;
        graph
            .actor_integrations
            .iter_mut()
            .find(|binding| binding.actor_ref == record.target_actor_ref)
            .unwrap()
            .adapter_params["changed"] = json!(true);
        assert_eq!(
            selected_process_route(&graph, &retry.claim.invocation, &mut retried).unwrap(),
            route
        );
        graph
            .actor_integrations
            .iter_mut()
            .find(|binding| binding.actor_ref == record.target_actor_ref)
            .unwrap()
            .available_authority
            .clear();
        assert!(
            selected_process_route(&graph, &retry.claim.invocation, &mut retried)
                .unwrap_err()
                .to_string()
                .contains("admitted execution authority was revoked")
        );
    }
}

#[test]
fn legacy_fallback_rejects_changed_routes_missing_fingerprints_and_revoked_bindings() {
    let (temporary, graph, invocation_ref) = legacy_admission(false);
    let mut store =
        ConversationAdmissionStore::open(temporary.path().join("cwpdb.sqlite3"), RUNTIME_REF)
            .unwrap();
    let claimed = claim(&mut store, &invocation_ref);
    for change in 0..10 {
        let mut changed = graph.clone();
        let mut invocation = claimed.adapter_invocation.clone();
        let binding = changed
            .actor_integrations
            .iter_mut()
            .find(|binding| binding.actor_ref == claimed.claim.invocation.target_actor_ref)
            .unwrap();
        match change {
            0 => binding.adapter_ref = crate::adapter::SH_ADAPTER_REF.into(),
            1 => binding.working_path = temporary.path().join("different"),
            2 => binding.coworker_source = temporary.path().join("different/WORKER.toml"),
            3 => binding.adapter_params["different"] = json!(true),
            4 => binding.integration_params["different"] = json!(true),
            5 => binding.actor_integration_params["different"] = json!(true),
            6 => binding.available_authority.clear(),
            7 => binding.integration_ref = "cwp://example.test/different-provider".into(),
            8 => binding.actor_integration_ref = "cwp://example.test/different-binding".into(),
            _ => invocation.route_fingerprint = None,
        }
        assert!(
            selected_process_route(&changed, &claimed.claim.invocation, &mut invocation).is_err(),
            "change {change}"
        );
        assert!(
            invocation.retained_route.is_none(),
            "failed fallback must not freeze another route"
        );
    }
}
