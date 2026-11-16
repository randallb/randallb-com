use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::*;

fn root_request(invocation_ref: &str) -> StartActionInvocation {
    StartActionInvocation {
        invocation_ref: invocation_ref.into(),
        origin: InvocationOrigin::Ingress {
            integration_ref: "~@/integrations/slack".into(),
            provider_event_ref: format!("event-{invocation_ref}"),
        },
        requester_actor_ref: "~@/actors/requester".into(),
        target_actor_ref: "~@/actors/parent".into(),
        conversation_ref: Some("~@/conversations/one".into()),
        actor_integration_ref: Some("~@/actor_integrations/parent_slack".into()),
        action_name: "respond_to--message".into(),
        input: json!({"message": "hello"}),
        parent_invocation_ref: None,
        admitted_at_unix_ms: 10,
    }
}

fn child_request(
    invocation_ref: &str,
    parent_invocation_ref: &str,
    call_id: &str,
) -> StartActionInvocation {
    StartActionInvocation {
        invocation_ref: invocation_ref.into(),
        origin: InvocationOrigin::HostCall {
            caller_invocation_ref: parent_invocation_ref.into(),
            call_id: call_id.into(),
        },
        requester_actor_ref: "~@/actors/parent".into(),
        target_actor_ref: "~@/actors/child".into(),
        conversation_ref: Some("~@/conversations/one".into()),
        actor_integration_ref: Some("~@/actor_integrations/child_slack".into()),
        action_name: "draft--brief".into(),
        input: json!({"topic": "launch"}),
        parent_invocation_ref: Some(parent_invocation_ref.into()),
        admitted_at_unix_ms: 20,
    }
}

fn start_running_root(ledger: &mut ActionInvocationLedger, invocation_ref: &str) {
    assert!(matches!(
        ledger.start(root_request(invocation_ref)).unwrap(),
        StartInvocationOutcome::Started(_)
    ));
    assert!(matches!(
        ledger.claim(invocation_ref, 11).unwrap(),
        ClaimInvocationOutcome::Claimed(_)
    ));
}
include!("tests/status.rs");
include!("tests/start_claim.rs");
include!("tests/terminal_stop.rs");
include!("tests/waits.rs");
