//! GraphQL exposes the resident's existing owner-scoped provider services.
use super::sessions::ClientSession;
use async_graphql::{Context, InputObject, Json, Object, Result, SimpleObject, Union, ID};
use cwr::{
    v2026_09::upstream::{UpstreamAction, UpstreamCoworker, UpstreamEvent, UpstreamWork},
    CwrResult,
};
use std::sync::Arc;

#[derive(InputObject)]
struct TerminalActionInput {
    action_name: String,
    title: String,
    description: String,
    input_schema: Json<serde_json::Value>,
    output_schema: Json<serde_json::Value>,
}
#[derive(InputObject)]
struct TerminalCoworkerInput {
    actor_ref: ID,
    coworker_ref: ID,
    handle: String,
    name: String,
    actions: Vec<TerminalActionInput>,
}
impl From<TerminalCoworkerInput> for UpstreamCoworker {
    fn from(c: TerminalCoworkerInput) -> Self {
        Self {
            actor_ref: c.actor_ref.0,
            coworker_ref: c.coworker_ref.0,
            handle: c.handle,
            name: c.name,
            actions: c
                .actions
                .into_iter()
                .map(|a| UpstreamAction {
                    action_name: a.action_name,
                    title: a.title,
                    description: a.description,
                    input_schema: a.input_schema.0,
                    output_schema: a.output_schema.0,
                })
                .collect(),
        }
    }
}
#[derive(Default)]
pub(super) struct TerminalMutations;
#[Object]
impl TerminalMutations {
    async fn register_terminal_provider(
        &self,
        ctx: &Context<'_>,
        coworkers: Vec<TerminalCoworkerInput>,
    ) -> Result<ID> {
        let session = ctx.data_unchecked::<Arc<ClientSession>>().clone();
        operation(session, move |s| {
            s.terminal_operation(|| {
                let weak = Arc::downgrade(s);
                let (stop_tx, stop_rx) = std::sync::mpsc::sync_channel(16);
                let stop_session = Arc::downgrade(s);
                std::thread::Builder::new()
                    .name("graphql-terminal-stop".into())
                    .spawn(move || forward_stops(stop_session, stop_rx))?;
                s.service
                    .runtime
                    .register_upstream_provider(
                        &s.id,
                        coworkers.into_iter().map(Into::into).collect(),
                        Arc::new(move |event| {
                            let s = weak.upgrade().ok_or_else(|| {
                                std::io::Error::other("terminal session disconnected")
                            })?;
                            s.ensure_open()?;
                            s.events.try_send(event).map_err(|_| {
                                std::io::Error::other(
                                    "terminal event capacity exhausted or receiver disconnected",
                                )
                            })?;
                            Ok(())
                        }),
                        stop_tx,
                    )
                    .map(ID)
            })
        })
        .await
    }
    async fn enable_terminal_coworker(
        &self,
        ctx: &Context<'_>,
        provider_session_ref: ID,
        actor_ref: ID,
        enabled: bool,
    ) -> Result<bool> {
        operation(
            ctx.data_unchecked::<Arc<ClientSession>>().clone(),
            move |s| {
                s.terminal_operation(|| {
                    s.service.runtime.enable_upstream_coworker(
                        &s.id,
                        &provider_session_ref,
                        &actor_ref,
                        &s.service.actor_ref,
                        enabled,
                    )?;
                    Ok(true)
                })
            },
        )
        .await
    }
    async fn complete_terminal_work(
        &self,
        ctx: &Context<'_>,
        invocation_ref: ID,
        terminal: Json<serde_json::Value>,
    ) -> Result<bool> {
        operation(
            ctx.data_unchecked::<Arc<ClientSession>>().clone(),
            move |s| {
                s.service.runtime.complete_upstream_work(
                    &s.id,
                    &invocation_ref,
                    serde_json::from_value(terminal.0)?,
                )?;
                Ok(true)
            },
        )
        .await
    }
    async fn acknowledge_terminal_stop(
        &self,
        ctx: &Context<'_>,
        invocation_ref: ID,
    ) -> Result<bool> {
        operation(
            ctx.data_unchecked::<Arc<ClientSession>>().clone(),
            move |s| {
                s.service
                    .runtime
                    .acknowledge_upstream_stop(&s.id, &invocation_ref)?;
                Ok(true)
            },
        )
        .await
    }
    async fn protect_terminal_result(&self, ctx: &Context<'_>, invocation_ref: ID) -> Result<bool> {
        operation(
            ctx.data_unchecked::<Arc<ClientSession>>().clone(),
            move |s| {
                s.service
                    .runtime
                    .protect_upstream_result(&s.id, &invocation_ref)?;
                Ok(true)
            },
        )
        .await
    }
    async fn validate_terminal_result(
        &self,
        ctx: &Context<'_>,
        validation_ref: ID,
        allowed: bool,
    ) -> Result<bool> {
        operation(
            ctx.data_unchecked::<Arc<ClientSession>>().clone(),
            move |s| {
                s.service
                    .runtime
                    .validate_upstream_result(&s.id, &validation_ref, allowed)?;
                Ok(true)
            },
        )
        .await
    }
    async fn revoke_terminal_result(&self, ctx: &Context<'_>, invocation_ref: ID) -> Result<bool> {
        operation(
            ctx.data_unchecked::<Arc<ClientSession>>().clone(),
            move |s| {
                s.service
                    .runtime
                    .revoke_upstream_result(&s.id, &invocation_ref)?;
                Ok(true)
            },
        )
        .await
    }
}
async fn operation<T: Send + 'static>(
    session: Arc<ClientSession>,
    operation: impl FnOnce(&Arc<ClientSession>) -> CwrResult<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(move || {
        session.ensure_open()?;
        let result = operation(&session);
        // Completion and cancellation may settle after the scheduler worker exits.
        // Wake every session even if the requesting connection has gone away.
        session.notify_changes();
        result
    })
    .await?
    .map_err(|_| {
        async_graphql::Error::new("Terminal operation failed or is unavailable to this session")
    })
}
#[derive(SimpleObject)]
pub(super) struct SessionReady {
    pub session_ref: ID,
}
#[derive(SimpleObject)]
pub(super) struct TerminalWork {
    invocation_ref: ID,
    root_invocation_ref: ID,
    conversation_ref: ID,
    requester_actor_ref: ID,
    provider_session_ref: ID,
    target_actor_ref: ID,
    coworker_ref: ID,
    action_name: String,
    input: Json<serde_json::Value>,
    addressed_message: Option<Json<serde_json::Value>>,
}
#[derive(SimpleObject)]
pub(super) struct TerminalStop {
    invocation_ref: ID,
}
#[derive(SimpleObject)]
pub(super) struct TerminalResultValidation {
    invocation_ref: ID,
    validation_ref: ID,
}
#[derive(Union)]
pub(super) enum TerminalEvent {
    Ready(SessionReady),
    Work(Box<TerminalWork>),
    Stop(TerminalStop),
    Validate(TerminalResultValidation),
}
impl From<UpstreamEvent> for TerminalEvent {
    fn from(event: UpstreamEvent) -> Self {
        match event {
            UpstreamEvent::WorkRequested { work } => {
                let UpstreamWork {
                    invocation_ref,
                    root_invocation_ref,
                    conversation_ref,
                    requester_actor_ref,
                    route,
                    action_name,
                    input,
                    addressed_message,
                } = *work;
                Self::Work(Box::new(TerminalWork {
                    invocation_ref: ID(invocation_ref),
                    root_invocation_ref: ID(root_invocation_ref),
                    conversation_ref: ID(conversation_ref),
                    requester_actor_ref: ID(requester_actor_ref),
                    provider_session_ref: ID(route.provider_session_ref),
                    target_actor_ref: ID(route.coworker.actor_ref),
                    coworker_ref: ID(route.coworker.coworker_ref),
                    action_name,
                    input: Json(input),
                    addressed_message: addressed_message
                        .map(|m| Json(serde_json::to_value(m).expect("serializable message"))),
                }))
            }
            UpstreamEvent::StopRequested { invocation_ref } => Self::Stop(TerminalStop {
                invocation_ref: ID(invocation_ref),
            }),
            UpstreamEvent::ValidateResult {
                invocation_ref,
                validation_ref,
            } => Self::Validate(TerminalResultValidation {
                invocation_ref: ID(invocation_ref),
                validation_ref: ID(validation_ref),
            }),
        }
    }
}

fn forward_stops(
    session: std::sync::Weak<ClientSession>,
    events: std::sync::mpsc::Receiver<UpstreamEvent>,
) {
    for event in events {
        let Some(session) = session.upgrade() else {
            break;
        };
        if !session.alive() || session.events.try_send(event).is_err() {
            session.close();
            break;
        }
    }
}
