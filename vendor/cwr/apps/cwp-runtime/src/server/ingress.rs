use super::*;
use cwr::v2026_09::{conversation::ConversationIngress, service::RuntimeIngressAttachment};

/// Provider events admitted by a Desktop session retain only that connection's
/// host-local routes. Direct background admissions remain unowned.
pub(super) fn admit(
    state: &SharedState,
    writer: &SharedWriter,
    request_id: String,
    host_actor_ref: Option<String>,
    ingress: Box<ConversationIngress>,
    actor_integration_ref: Option<String>,
    attachment: Option<RuntimeIngressAttachment>,
) -> CwrResult<()> {
    let owner = upstream_owner(writer);
    let runtime = {
        let state = lock_state(state)?;
        if let Some(actor) = host_actor_ref.as_ref() {
            if state.host_actors.get(&owner) != Some(actor) || actor != &ingress.author_actor_ref {
                return invalid("ingress requires the connection's selected author");
            }
        }
        state.runtime.clone()
    };
    let module = runtime
        .bundled_conversation_module_as(&ingress.integration_ref, &ingress.author_actor_ref)?;
    let admission = module.admit_provider_ingress(
        *ingress,
        actor_integration_ref.as_deref(),
        host_actor_ref.as_ref().map(|_| owner.as_str()),
    )?;
    let attachment_ref = attachment
        .map(|attachment| attachments::retain_ingress(state, &admission, attachment))
        .transpose()?;
    accept_admission(state, writer, request_id, admission, attachment_ref)
}
