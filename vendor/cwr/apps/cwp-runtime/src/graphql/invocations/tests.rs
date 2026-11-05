use super::*;
use async_graphql::{EmptyMutation, EmptySubscription, Object, Schema};
use cwr::v2026_09::conversation::{ConversationInvocationOutcome, ConversationOutcomeDetail};
use cwr::v2026_09::invocation::ActionResponseStatus;

struct Projection(ConversationInvocation);
#[Object]
impl Projection {
    async fn invocation(&self) -> ActionInvocation {
        project_lifecycle(Box::new(self.0.clone())).unwrap()
    }
    async fn result(&self) -> ActionResult {
        project_lifecycle(Box::new(self.0.clone()))
            .unwrap()
            .into_result()
    }
}
fn record(state: ActionInvocationState) -> ConversationInvocation {
    ConversationInvocation {
        invocation_ref: "cwp://example.test/invocations/one".into(),
        conversation_ref: "cwp://example.test/conversations/one".into(),
        requester_actor_ref: "cwp://example.test/actors/person".into(),
        target_actor_ref: "cwp://example.test/actors/worker".into(),
        source_message_ref: None,
        parent_invocation_ref: None,
        child_invocation_refs: vec![],
        waits: vec![],
        state,
        outcome: None,
        admitted_at_unix_ms: 1,
        started_at_unix_ms: None,
        finished_at_unix_ms: None,
        stop_requested_at_unix_ms: None,
        stop_settlement_tracked: false,
        latest_reply: None,
        current_execution: None,
    }
}
#[tokio::test]
async fn every_lifecycle_resolves_through_interface_and_mutation_union() {
    for (state, typename, public_state) in [
        (
            ActionInvocationState::Admitted,
            "AdmittedInvocation",
            "admitted",
        ),
        (
            ActionInvocationState::AwaitingPerson,
            "AwaitingPersonInvocation",
            "awaiting_person",
        ),
        (
            ActionInvocationState::Running,
            "RunningInvocation",
            "running",
        ),
        (
            ActionInvocationState::Completed,
            "CompletedInvocation",
            "completed",
        ),
        (
            ActionInvocationState::Blocked,
            "BlockedInvocation",
            "blocked",
        ),
        (ActionInvocationState::Failed, "FailedInvocation", "failed"),
        (
            ActionInvocationState::Stopping,
            "StoppingInvocation",
            "stopping",
        ),
        (
            ActionInvocationState::Stopped,
            "StoppedInvocation",
            "stopped",
        ),
    ] {
        let schema = Schema::new(Projection(record(state)), EmptyMutation, EmptySubscription);
        let response = schema.execute("{invocation{__typename id state outcome{status} activities{coverage} calls{coverage} waits{coverage}} result{__typename ... on ActionInvocation{id state outcome{status} activities{coverage} calls{coverage} waits{coverage}}}}").await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let data = response.data.into_json().unwrap();
        assert_eq!(data["invocation"], data["result"]);
        assert_eq!(data["invocation"]["__typename"], typename);
        assert_eq!(data["invocation"]["state"], public_state);
        assert_eq!(
            data["invocation"]["id"],
            "cwp://example.test/invocations/one"
        );
        // Historical terminal records are permitted to lack outcome metadata.
        assert!(data["invocation"]["outcome"].is_null());
    }
}
#[tokio::test]
async fn stopping_has_no_outcome_and_distinguishes_legacy_settlement() {
    for (state, tracked, expected) in [
        (ActionInvocationState::Stopping, true, "PENDING"),
        (ActionInvocationState::Stopped, true, "CONFIRMED"),
        (ActionInvocationState::Stopped, false, "UNAVAILABLE"),
    ] {
        let mut invocation = record(state);
        invocation.stop_settlement_tracked = tracked;
        invocation.stop_requested_at_unix_ms = Some(2);
        let schema = Schema::new(Projection(invocation), EmptyMutation, EmptySubscription);
        let response = schema
            .execute("{invocation{state stopSettlement finishedAt outcome{status}}}")
            .await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let data = response.data.into_json().unwrap();
        assert_eq!(data["invocation"]["stopSettlement"], expected);
        assert!(data["invocation"]["outcome"].is_null());
        assert!(data["invocation"]["finishedAt"].is_null());
    }
}
#[tokio::test]
async fn stopped_cause_is_public_and_nonterminal_outcome_is_absent() {
    for state in [
        ActionInvocationState::Running,
        ActionInvocationState::Stopped,
    ] {
        let mut invocation = record(state);
        invocation.outcome = Some(ConversationInvocationOutcome {
            status: ActionResponseStatus::STOPPED,
            summary: "Stopped".into(),
            error: None,
            cause: Some(ConversationOutcomeDetail {
                code: "person_requested_stop".into(),
                message: Some("Stopped by person".into()),
            }),
        });
        let schema = Schema::new(Projection(invocation), EmptyMutation, EmptySubscription);
        let response = schema
            .execute("{invocation{outcome{status summary cause{code message}}}}")
            .await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let data = response.data.into_json().unwrap();
        if state == ActionInvocationState::Running {
            assert!(data["invocation"]["outcome"].is_null());
        } else {
            assert_eq!(data["invocation"]["outcome"]["status"], 499);
            assert_eq!(
                data["invocation"]["outcome"]["cause"]["code"],
                "person_requested_stop"
            );
        }
    }
}
#[tokio::test]
async fn inactive_claims_do_not_expose_stale_execution_progress() {
    for active in [false, true] {
        let mut value = record(ActionInvocationState::Running);
        value.current_execution = Some(cwr::v2026_09::conversation::ConversationExecution {
            generation: 2,
            attempt_active: active,
            phase: cwr::adapter::ExecutionPhase::Working,
            started_at_unix_ms: Some(100),
            latest_activity_at_unix_ms: Some(200),
            progress: Some("Old progress".into()),
            live_output: None,
        });
        let schema = Schema::new(Projection(value), EmptyMutation, EmptySubscription);
        let response = schema
            .execute("{invocation{currentExecution{generation progress}}}")
            .await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let value = response.data.into_json().unwrap();
        if active {
            assert_eq!(value["invocation"]["currentExecution"]["generation"], "2");
        } else {
            assert!(value["invocation"]["currentExecution"].is_null());
        }
    }
}
#[test]
fn unrecovered_waiting_is_not_a_public_lifecycle() {
    assert!(project_lifecycle(Box::new(record(ActionInvocationState::Waiting))).is_err());
}
