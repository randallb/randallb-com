//! Mutations use runtime admission fingerprints and the resident's execution owner.
use super::schema::ReadService;
use async_graphql::{
    Context, Error, ErrorExtensions, Json, MergedObject, Object, Result, SimpleObject, Union, ID,
};
use cwr::v2026_09::{conversation::ConversationResource, local::LocalAction};

#[derive(MergedObject, Default)]
pub(super) struct Mutation(
    ActionMutations,
    super::files::FileMutations,
    super::terminals::TerminalMutations,
    super::messaging::MessageMutations,
);
#[derive(Default)]
struct ActionMutations;
#[Object]
impl ActionMutations {
    async fn invoke_action(
        &self,
        ctx: &Context<'_>,
        url: ID,
        activation: String,
        input: Json<serde_json::Value>,
        request_id: ID,
    ) -> Result<ActionResult> {
        if request_id.is_empty() || request_id.len() > 256 {
            return Ok(rejected(
                "invalid_request_id",
                "requestId must contain 1 to 256 bytes",
            ));
        }
        let mut service = ctx.data_unchecked::<ReadService>().clone();
        // Earlier mutation fields may have opened a pre-admission snapshot.
        // Preserve this operation's deadline while checking the current target.
        service.publications = Default::default();
        let budget = service.budget();
        let permit = budget.acquire(service.reads.clone()).await?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            // Pool delays must not start admission after the selection expires.
            // Once admission starts, preserve its durable outcome and recovery.
            budget.check().map_err(|_| {
                Error::new("Action admission did not start before its deadline or cancellation")
            })?;
            invoke(
                &service,
                url.as_str(),
                &activation,
                input.0,
                request_id.as_str(),
            )
        })
        .await?
    }
}

fn invoke(
    service: &ReadService,
    url: &str,
    activation: &str,
    input: serde_json::Value,
    request_id: &str,
) -> Result<ActionResult> {
    let schedule_activation = matches!(
        activation,
        "@cwr--list--schedules"
            | "@cwr--get--schedule"
            | "@cwr--pause--schedule"
            | "@cwr--resume--schedule"
            | "@cwr--list--schedule_runs"
    );
    // A schedule action requires the selected installed owner or an existing
    // conversation. The runtime root cannot recover an owner-destination
    // receipt after that owner is removed.
    if schedule_activation && url == service.runtime_ref {
        return Ok(rejected(
            "schedule_owner_required",
            "Select the installed schedule owner or an existing conversation",
        ));
    }
    let visible_conversation = url != service.runtime_ref
        && service
            .publications
            .read(service, |view| view.conversation(url))
            .map_err(|_| Error::new("Runtime resource read failed"))?
            .is_some();
    // Ordinary native controls select their installed owner explicitly. Actor
    // refs and unknown resources never imply an owner's conversation context.
    let ordinary_owner = !schedule_activation
        && !activation.starts_with("@cwr--")
        && url != service.runtime_ref
        && !visible_conversation
        && service
            .runtime
            .conversation_context_targets(Some(url))
            .map_err(|_| Error::new("Runtime context read failed"))?
            .is_some_and(|targets| !targets.is_empty());
    let owner_context_ref = if (schedule_activation && !visible_conversation) || ordinary_owner {
        Some(url)
    } else {
        None
    };
    let conversation_ref = if owner_context_ref.is_some() || url == service.runtime_ref {
        None
    } else {
        if !visible_conversation {
            return Ok(rejected(
                "resource_unavailable",
                "Resource is unavailable to this actor",
            ));
        }
        Some(url)
    };
    // CWR's durable admission key is (integration, provider event). Include actor
    // identity so request IDs are independent across credentials without changing
    // the persisted ingress fingerprint's target, activation or normalized input.
    let key = serde_json::to_string(&(&service.actor_ref, request_id))
        .map_err(|_| Error::new("Unable to encode request identity"))?;
    let admission = match owner_context_ref.map_or_else(
        || {
            service.runtime.admit_action_in_upstream_scope(
                &service.ingress,
                &key,
                &service.actor_ref,
                conversation_ref,
                activation,
                input.clone(),
                service.owner.as_deref(),
            )
        },
        |owner| {
            if ordinary_owner {
                service.runtime.admit_owner_action_in_upstream_scope(
                    &service.ingress,
                    &key,
                    &service.actor_ref,
                    owner,
                    activation,
                    input.clone(),
                    service.owner.as_deref(),
                )
            } else {
                service
                    .runtime
                    .admit_owner_schedule_action_in_upstream_scope(
                        &service.ingress,
                        &key,
                        &service.actor_ref,
                        owner,
                        activation,
                        input.clone(),
                        service.owner.as_deref(),
                    )
            }
        },
    ) {
        Ok(admission) => admission,
        Err(error) if error.is::<cwr::v2026_09::conversation::ConversationContextError>() => {
            return Err(Error::new(error.to_string()).extend_with(|_, extensions| {
                extensions.set("code", "CONVERSATION_CONTEXT_REJECTED")
            }));
        }
        Err(error)
            if error.is::<cwr::ConfigurationError>()
                && service
                    .runtime
                    .admission_record(&service.ingress, &key)
                    .is_ok_and(|record| record.is_none()) =>
        {
            return Ok(rejected("admission_rejected", &error.to_string()))
        }
        Err(_) => return Err(unknown_outcome()),
    };
    let id = admission.invocation_ref().ok_or_else(unknown_outcome)?;
    // Admission is already persisted. Publication/scheduling/read failures must
    // never become a safe rejection that encourages dispatch under a new ID.
    (service.publisher)(&admission).map_err(|_| unknown_outcome())?;
    let mut response_service = service.clone();
    response_service.publications = Default::default();
    let invocation = read_invocation(&response_service, id)
        .map_err(|_| unknown_outcome())?
        .ok_or_else(unknown_outcome)?;
    Ok(invocation.into_result())
}

fn rejected(code: &str, message: &str) -> ActionResult {
    ActionResult::Error(ActionError {
        code: code.into(),
        message: message.into(),
        retryable: false,
    })
}
fn unknown_outcome() -> Error {
    Error::new("Action outcome could not be confirmed; retry with the same requestId and input")
        .extend_with(|_, extensions| extensions.set("code", "ACTION_OUTCOME_UNKNOWN"))
}

pub(super) fn read_invocation(
    service: &ReadService,
    id: &str,
) -> cwr::CwrResult<Option<ActionInvocation>> {
    service.catalog()?;
    let Some(snapshot) = service
        .publications
        .read(service, |view| view.resource(id))?
    else {
        return Ok(None);
    };
    let ConversationResource::Invocation { invocation } = snapshot.resource else {
        return Ok(None);
    };
    super::invocations::project(invocation, service.clone()).map(Some)
}

#[derive(SimpleObject)]
pub(super) struct Action {
    actor_ref: ID,
    action_name: String,
    activation: String,
    title: String,
    description: String,
    input_schema: Json<serde_json::Value>,
    output_schema: Json<serde_json::Value>,
}
impl From<LocalAction> for Action {
    fn from(action: LocalAction) -> Self {
        Self {
            actor_ref: ID(action.actor_ref),
            action_name: action.action_name,
            activation: action.activation,
            title: action.title,
            description: action.description,
            input_schema: Json(action.input_schema),
            output_schema: Json(action.output_schema),
        }
    }
}
pub(super) use super::invocations::ActionInvocation;

#[derive(SimpleObject)]
pub(super) struct ActionError {
    code: String,
    message: String,
    retryable: bool,
}
#[derive(Union)]
pub(super) enum ActionResult {
    Admitted(super::invocations::AdmittedInvocation),
    AwaitingPerson(super::invocations::AwaitingPersonInvocation),
    Running(super::invocations::RunningInvocation),
    Completed(super::invocations::CompletedInvocation),
    Blocked(super::invocations::BlockedInvocation),
    Failed(super::invocations::FailedInvocation),
    Stopping(super::invocations::StoppingInvocation),
    Stopped(super::invocations::StoppedInvocation),
    Error(ActionError),
}
