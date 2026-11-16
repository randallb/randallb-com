//! Lifecycle projection uses retained CWR state, never inferred execution state.
use super::messages::{datetime, DateTime};
use super::mutations::ActionResult;
use async_graphql::{Enum, Interface, SimpleObject, ID};
use cwr::v2026_09::{conversation::ConversationInvocation, invocation::ActionInvocationState};

/// Whether executor release is confirmed, pending, or absent from legacy history.
#[derive(Clone, Copy, PartialEq, Eq, Enum)]
pub(super) enum StopSettlement {
    Pending,
    Confirmed,
    Unavailable,
}

#[derive(Clone, SimpleObject)]
pub(super) struct InvocationOutcome {
    status: i32,
    summary: String,
    error: Option<OutcomeDetail>,
    cause: Option<OutcomeDetail>,
}
#[derive(Clone, SimpleObject)]
struct OutcomeDetail {
    code: String,
    message: Option<String>,
}

#[derive(Clone)]
struct InvocationData {
    // Only pure lifecycle unit fixtures omit a service; public projections require it.
    service: Option<super::schema::ReadService>,
    id: ID,
    state: String,
    conversation_ref: ID,
    requester_actor_ref: ID,
    target_actor_ref: ID,
    source_message_ref: Option<ID>,
    parent_invocation_ref: Option<ID>,
    child_invocation_refs: Vec<ID>,
    outcome: Option<InvocationOutcome>,
    admitted_at: DateTime,
    started_at: Option<DateTime>,
    finished_at: Option<DateTime>,
    stop_requested_at: Option<DateTime>,
    stop_settlement: Option<StopSettlement>,
    current_execution: Option<ExecutionAttempt>,
}

#[derive(Clone, SimpleObject)]
pub(super) struct ExecutionAttempt {
    generation: String,
    phase: String,
    started_at: Option<DateTime>,
    latest_activity_at: Option<DateTime>,
    progress: Option<String>,
}

macro_rules! lifecycle {
    ($name:ident) => {
        #[derive(Clone)]
        pub(super) struct $name(InvocationData);
        #[async_graphql::Object]
        impl $name {
            pub(super) async fn id(&self) -> ID {
                self.0.id.clone()
            }
            async fn state(&self) -> String {
                self.0.state.clone()
            }
            async fn conversation_ref(&self) -> ID {
                self.0.conversation_ref.clone()
            }
            async fn requester_actor_ref(&self) -> ID {
                self.0.requester_actor_ref.clone()
            }
            async fn target_actor_ref(&self) -> ID {
                self.0.target_actor_ref.clone()
            }
            /// The retained requester identity under this invocation's session visibility.
            async fn requester(&self) -> async_graphql::Result<Option<super::messages::Actor>> {
                related_actor(&self.0, &self.0.requester_actor_ref).await
            }
            /// The retained target identity, including unresolved historical actors.
            async fn target(&self) -> async_graphql::Result<Option<super::messages::Actor>> {
                related_actor(&self.0, &self.0.target_actor_ref).await
            }
            async fn source_message_ref(&self) -> Option<ID> {
                self.0.source_message_ref.clone()
            }
            async fn parent_invocation_ref(&self) -> Option<ID> {
                self.0.parent_invocation_ref.clone()
            }
            async fn child_invocation_refs(&self) -> Vec<ID> {
                self.0.child_invocation_refs.clone()
            }
            async fn admitted_at(&self) -> DateTime {
                self.0.admitted_at.clone()
            }
            async fn started_at(&self) -> Option<DateTime> {
                self.0.started_at.clone()
            }
            async fn finished_at(&self) -> Option<DateTime> {
                self.0.finished_at.clone()
            }
            async fn stop_requested_at(&self) -> Option<DateTime> {
                self.0.stop_requested_at.clone()
            }
            async fn stop_settlement(&self) -> Option<StopSettlement> {
                self.0.stop_settlement
            }
            async fn current_execution(&self) -> Option<ExecutionAttempt> {
                self.0.current_execution.clone()
            }
            async fn activities(
                &self,
                first: Option<i32>,
                after: Option<super::publications::Cursor>,
                last: Option<i32>,
                before: Option<super::publications::Cursor>,
            ) -> async_graphql::Result<Option<super::inspector::WorkActivityConnection>> {
                let Some(service) = self.0.service.clone() else {
                    return Ok(None);
                };
                super::inspector::activities(
                    service,
                    self.0.id.to_string(),
                    super::windows::Window {
                        first,
                        after,
                        last,
                        before,
                    },
                )
                .await
            }
            async fn calls(
                &self,
                first: Option<i32>,
                after: Option<super::publications::Cursor>,
                last: Option<i32>,
                before: Option<super::publications::Cursor>,
            ) -> async_graphql::Result<Option<super::inspector::InvocationCallConnection>> {
                let Some(service) = self.0.service.clone() else {
                    return Ok(None);
                };
                super::inspector::calls(
                    service,
                    self.0.id.to_string(),
                    super::windows::Window {
                        first,
                        after,
                        last,
                        before,
                    },
                )
                .await
            }
            async fn waits(
                &self,
                first: Option<i32>,
                after: Option<super::publications::Cursor>,
                last: Option<i32>,
                before: Option<super::publications::Cursor>,
            ) -> async_graphql::Result<Option<super::inspector::InvocationWaitConnection>> {
                let Some(service) = self.0.service.clone() else {
                    return Ok(None);
                };
                super::inspector::waits(
                    service,
                    self.0.id.to_string(),
                    super::windows::Window {
                        first,
                        after,
                        last,
                        before,
                    },
                )
                .await
            }
            async fn stop_control(
                &self,
            ) -> async_graphql::Result<Option<super::controls::StopControl>> {
                let Some(service) = self.0.service.clone() else {
                    return Ok(None);
                };
                super::controls::project(service, self.0.id.to_string()).await
            }
            async fn forked_conversation_ref(&self) -> async_graphql::Result<Option<ID>> {
                let Some(service) = self.0.service.clone() else {
                    return Ok(None);
                };
                let reference = self.0.id.to_string();
                super::schema::blocking(service.reads.clone(), service.budget(), move || {
                    service
                        .publications
                        .read(&service, |view| {
                            view.invocation_fork_destination(&reference)
                        })
                        .map(|value| value.map(ID))
                })
                .await
            }
            async fn action_presentation(
                &self,
            ) -> async_graphql::Result<Option<super::invocation_action::InvocationAction>> {
                super::invocation_action::project(
                    self.0.service.clone(),
                    self.0.source_message_ref.clone(),
                    self.0.id.clone(),
                )
                .await
            }
            /// The authorized final published reply for named actions and ordinary chat.
            async fn result_message(
                &self,
            ) -> async_graphql::Result<Option<super::schema::Message>> {
                super::invocation_action::result_message(
                    self.0.service.clone(),
                    self.0.id.to_string(),
                )
                .await
            }
            async fn outcome(&self) -> Option<InvocationOutcome> {
                self.0.outcome.clone()
            }
        }
    };
}
lifecycle!(AdmittedInvocation);
lifecycle!(AwaitingPersonInvocation);
lifecycle!(RunningInvocation);
lifecycle!(CompletedInvocation);
lifecycle!(BlockedInvocation);
lifecycle!(FailedInvocation);
lifecycle!(StoppingInvocation);
lifecycle!(StoppedInvocation);

// Clippy treats repeated field types in async-graphql's nested field
// declarations as duplicate attributes; each declaration names a distinct field.
#[expect(
    clippy::duplicated_attributes,
    reason = "async-graphql repeats types for distinct interface fields"
)]
#[derive(Clone, Interface)]
#[graphql(
    field(name = "id", ty = "ID"),
    field(name = "state", ty = "String"),
    field(name = "conversation_ref", ty = "ID"),
    field(name = "requester_actor_ref", ty = "ID"),
    field(name = "target_actor_ref", ty = "ID"),
    field(name = "requester", ty = "Option<super::messages::Actor>"),
    field(name = "target", ty = "Option<super::messages::Actor>"),
    field(name = "source_message_ref", ty = "Option<ID>"),
    field(name = "parent_invocation_ref", ty = "Option<ID>"),
    field(name = "child_invocation_refs", ty = "Vec<ID>"),
    field(
        name = "activities",
        ty = "Option<super::inspector::WorkActivityConnection>",
        arg(name = "first", ty = "Option<i32>"),
        arg(name = "after", ty = "Option<super::publications::Cursor>"),
        arg(name = "last", ty = "Option<i32>"),
        arg(name = "before", ty = "Option<super::publications::Cursor>")
    ),
    field(
        name = "calls",
        ty = "Option<super::inspector::InvocationCallConnection>",
        arg(name = "first", ty = "Option<i32>"),
        arg(name = "after", ty = "Option<super::publications::Cursor>"),
        arg(name = "last", ty = "Option<i32>"),
        arg(name = "before", ty = "Option<super::publications::Cursor>")
    ),
    field(
        name = "waits",
        ty = "Option<super::inspector::InvocationWaitConnection>",
        arg(name = "first", ty = "Option<i32>"),
        arg(name = "after", ty = "Option<super::publications::Cursor>"),
        arg(name = "last", ty = "Option<i32>"),
        arg(name = "before", ty = "Option<super::publications::Cursor>")
    ),
    field(name = "forked_conversation_ref", ty = "Option<ID>"),
    field(name = "stop_control", ty = "Option<super::controls::StopControl>"),
    field(name = "outcome", ty = "Option<InvocationOutcome>"),
    field(name = "result_message", ty = "Option<super::schema::Message>"),
    field(
        name = "action_presentation",
        ty = "Option<super::invocation_action::InvocationAction>"
    ),
    field(name = "admitted_at", ty = "DateTime"),
    field(name = "started_at", ty = "Option<DateTime>"),
    field(name = "finished_at", ty = "Option<DateTime>"),
    field(name = "stop_requested_at", ty = "Option<DateTime>"),
    field(name = "stop_settlement", ty = "Option<StopSettlement>"),
    field(name = "current_execution", ty = "Option<ExecutionAttempt>")
)]
pub(super) enum ActionInvocation {
    Admitted(AdmittedInvocation),
    AwaitingPerson(AwaitingPersonInvocation),
    Running(RunningInvocation),
    Completed(CompletedInvocation),
    Blocked(BlockedInvocation),
    Failed(FailedInvocation),
    Stopping(StoppingInvocation),
    Stopped(StoppedInvocation),
}
async fn related_actor(
    data: &InvocationData,
    actor: &ID,
) -> async_graphql::Result<Option<super::messages::Actor>> {
    let Some(service) = data.service.clone() else {
        return Ok(None);
    };
    let reference = data.id.to_string();
    let actor = actor.to_string();
    super::schema::blocking(service.reads.clone(), service.budget(), move || {
        Ok(service
            .publications
            .read(&service, |view| view.related_actor(&reference, &actor))?
            .map(super::messages::actor))
    })
    .await
}
impl ActionInvocation {
    fn with_service(mut self, service: super::schema::ReadService) -> Self {
        match &mut self {
            Self::Admitted(v) => v.0.service = Some(service),
            Self::AwaitingPerson(v) => v.0.service = Some(service),
            Self::Running(v) => v.0.service = Some(service),
            Self::Completed(v) => v.0.service = Some(service),
            Self::Blocked(v) => v.0.service = Some(service),
            Self::Failed(v) => v.0.service = Some(service),
            Self::Stopping(v) => v.0.service = Some(service),
            Self::Stopped(v) => v.0.service = Some(service),
        }
        self
    }
    pub fn into_result(self) -> ActionResult {
        match self {
            Self::Admitted(value) => ActionResult::Admitted(value),
            Self::AwaitingPerson(value) => ActionResult::AwaitingPerson(value),
            Self::Running(value) => ActionResult::Running(value),
            Self::Completed(value) => ActionResult::Completed(value),
            Self::Blocked(value) => ActionResult::Blocked(value),
            Self::Failed(value) => ActionResult::Failed(value),
            Self::Stopping(value) => ActionResult::Stopping(value),
            Self::Stopped(value) => ActionResult::Stopped(value),
        }
    }
}
pub(super) fn project(
    invocation: Box<ConversationInvocation>,
    service: super::schema::ReadService,
) -> cwr::CwrResult<ActionInvocation> {
    Ok(project_lifecycle(invocation)?.with_service(service))
}
fn project_lifecycle(invocation: Box<ConversationInvocation>) -> cwr::CwrResult<ActionInvocation> {
    let state = invocation.state;
    // Startup recovery settles legacy suspended work. Seeing it here is a domain
    // invariant failure; do not silently expose a new lifecycle or fabricate 500.
    if state == ActionInvocationState::Waiting {
        return Err(std::io::Error::other("Unrecovered suspended invocation").into());
    }
    let terminal = matches!(
        state,
        ActionInvocationState::Completed
            | ActionInvocationState::Blocked
            | ActionInvocationState::Failed
            | ActionInvocationState::Stopped
    );
    let data = InvocationData {
        service: None,
        admitted_at: datetime(i128::from(invocation.admitted_at_unix_ms))?,
        started_at: invocation
            .started_at_unix_ms
            .map(|ms| datetime(i128::from(ms)))
            .transpose()?,
        finished_at: invocation
            .finished_at_unix_ms
            .map(|ms| datetime(i128::from(ms)))
            .transpose()?,
        stop_requested_at: invocation
            .stop_requested_at_unix_ms
            .map(|ms| datetime(i128::from(ms)))
            .transpose()?,
        stop_settlement: match state {
            ActionInvocationState::Stopping => Some(StopSettlement::Pending),
            ActionInvocationState::Stopped if invocation.stop_settlement_tracked => {
                Some(StopSettlement::Confirmed)
            }
            ActionInvocationState::Stopped => Some(StopSettlement::Unavailable),
            _ => None,
        },
        current_execution: if terminal {
            None
        } else {
            invocation
                .current_execution
                .filter(|execution| {
                    execution.attempt_active || state == ActionInvocationState::Stopping
                })
                .map(|execution| -> cwr::CwrResult<ExecutionAttempt> {
                    Ok(ExecutionAttempt {
                        generation: execution.generation.to_string(),
                        phase: serde_json::to_value(execution.phase)?
                            .as_str()
                            .unwrap_or_default()
                            .into(),
                        started_at: execution
                            .started_at_unix_ms
                            .map(|ms| datetime(i128::from(ms)))
                            .transpose()?,
                        latest_activity_at: execution
                            .latest_activity_at_unix_ms
                            .map(|ms| datetime(i128::from(ms)))
                            .transpose()?,
                        progress: execution.progress,
                    })
                })
                .transpose()?
        },
        id: ID(invocation.invocation_ref),
        state: serde_json::to_value(state)?
            .as_str()
            .ok_or_else(|| std::io::Error::other("Invalid invocation state"))?
            .to_owned(),
        conversation_ref: ID(invocation.conversation_ref),
        requester_actor_ref: ID(invocation.requester_actor_ref),
        target_actor_ref: ID(invocation.target_actor_ref),
        source_message_ref: invocation.source_message_ref.map(ID),
        parent_invocation_ref: invocation.parent_invocation_ref.map(ID),
        child_invocation_refs: invocation
            .child_invocation_refs
            .into_iter()
            .map(ID)
            .collect(),
        outcome: if terminal {
            invocation.outcome.map(|outcome| InvocationOutcome {
                status: i32::from(outcome.status.as_u16()),
                summary: outcome.summary,
                error: outcome.error.map(|detail| OutcomeDetail {
                    code: detail.code,
                    message: detail.message,
                }),
                cause: outcome.cause.map(|detail| OutcomeDetail {
                    code: detail.code,
                    message: detail.message,
                }),
            })
        } else {
            None
        },
    };
    Ok(match state {
        ActionInvocationState::Admitted => ActionInvocation::Admitted(AdmittedInvocation(data)),
        ActionInvocationState::AwaitingPerson => {
            ActionInvocation::AwaitingPerson(AwaitingPersonInvocation(data))
        }
        ActionInvocationState::Running => ActionInvocation::Running(RunningInvocation(data)),
        ActionInvocationState::Completed => ActionInvocation::Completed(CompletedInvocation(data)),
        ActionInvocationState::Blocked => ActionInvocation::Blocked(BlockedInvocation(data)),
        ActionInvocationState::Failed => ActionInvocation::Failed(FailedInvocation(data)),
        ActionInvocationState::Stopping => ActionInvocation::Stopping(StoppingInvocation(data)),
        ActionInvocationState::Stopped => ActionInvocation::Stopped(StoppedInvocation(data)),
        ActionInvocationState::Waiting => unreachable!(),
    })
}

#[cfg(test)]
mod tests;
