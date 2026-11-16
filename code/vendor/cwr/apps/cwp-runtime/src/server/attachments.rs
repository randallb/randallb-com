use std::{collections::BTreeSet, path::PathBuf};

use cwr::v2026_09::service::{
    RuntimeAttachmentRequest, RuntimeDeliveryRequest, RuntimeIngressAttachment,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RuntimeAttachment {
    pub(super) conversation_ref: String,
    pub(super) integration_ref: String,
    pub(super) destination_ref: String,
    pub(super) binding_key: Option<String>,
    pub(super) source_message_refs: BTreeSet<String>,
    pub(super) attached: bool,
    #[serde(skip)]
    pub(super) attachment_request_in_flight: bool,
    #[serde(skip)]
    pub(super) requested_delivery_refs: BTreeSet<String>,
    #[serde(default)]
    pub(super) terminal_delivery_refs: BTreeSet<String>,
}

pub(super) struct RuntimeAttachmentState {
    pub(super) attachments: BTreeMap<String, RuntimeAttachment>,
    pub(super) pending_default_conversation_refs: BTreeSet<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeAttachmentStateFile {
    version: u32,
    attachments: BTreeMap<String, RuntimeAttachment>,
    #[serde(default)]
    pending_default_conversation_refs: BTreeSet<String>,
}

pub(super) fn load(path: &PathBuf) -> CwrResult<RuntimeAttachmentState> {
    match fs::read_to_string(path) {
        Ok(contents) => {
            let retained: RuntimeAttachmentStateFile = serde_json::from_str(&contents)?;
            if retained.version != 1 {
                return invalid(format!(
                    "unsupported runtime attachment state version {}",
                    retained.version
                ));
            }
            Ok(RuntimeAttachmentState {
                attachments: retained.attachments,
                pending_default_conversation_refs: retained.pending_default_conversation_refs,
            })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(RuntimeAttachmentState {
            attachments: BTreeMap::new(),
            pending_default_conversation_refs: BTreeSet::new(),
        }),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn retain_ingress(
    state: &SharedState,
    admission: &LocalAdmission,
    attachment: RuntimeIngressAttachment,
) -> CwrResult<String> {
    let mut state = lock_state(state)?;
    let RuntimeIngressAttachment {
        integration_ref,
        destination_ref,
        binding_key,
    } = attachment;
    let bound_refs = state
        .attachments
        .iter()
        .filter(|(_, retained)| {
            retained.integration_ref == integration_ref
                && retained.binding_key.as_deref() == Some(binding_key.as_str())
        })
        .map(|(attachment_ref, _)| attachment_ref.clone())
        .collect::<Vec<_>>();
    for attachment_ref in &bound_refs {
        let retained = &state.attachments[attachment_ref];
        if retained.conversation_ref != admission.record.conversation_ref
            || retained.destination_ref != destination_ref
        {
            return invalid("a provider binding cannot move between conversations or destinations");
        }
    }
    let unbound_refs = state
        .attachments
        .iter()
        .filter(|(_, retained)| {
            retained.integration_ref == integration_ref
                && retained.destination_ref == destination_ref
                && retained.conversation_ref == admission.record.conversation_ref
                && retained.binding_key.is_none()
        })
        .map(|(attachment_ref, _)| attachment_ref.clone())
        .collect::<Vec<_>>();
    if let Some(attachment_ref) = bound_refs.first().or_else(|| unbound_refs.first()) {
        let attachment_ref = attachment_ref.clone();
        let mut retained = state
            .attachments
            .remove(&attachment_ref)
            .expect("a selected attachment ref remains present");
        for duplicate_ref in bound_refs
            .into_iter()
            .chain(unbound_refs)
            .filter(|duplicate_ref| duplicate_ref != &attachment_ref)
        {
            let duplicate = state
                .attachments
                .remove(&duplicate_ref)
                .expect("a duplicate attachment ref remains present");
            merge_attachment(&mut retained, duplicate);
        }
        retained.binding_key = Some(binding_key);
        retained
            .source_message_refs
            .insert(admission.record.source_message_ref.clone());
        state.attachments.insert(attachment_ref.clone(), retained);
        persist(&state)?;
        return Ok(attachment_ref);
    }
    let attachment_ref = format!(
        "{}/attachments/{}",
        state.runtime_ref,
        Uuid::new_v4().simple()
    );
    state.attachments.insert(
        attachment_ref.clone(),
        RuntimeAttachment {
            conversation_ref: admission.record.conversation_ref.clone(),
            integration_ref,
            destination_ref,
            binding_key: Some(binding_key),
            source_message_refs: BTreeSet::from([admission.record.source_message_ref.clone()]),
            attached: false,
            attachment_request_in_flight: false,
            requested_delivery_refs: BTreeSet::new(),
            terminal_delivery_refs: BTreeSet::new(),
        },
    );
    persist(&state)?;
    Ok(attachment_ref)
}

fn merge_attachment(retained: &mut RuntimeAttachment, duplicate: RuntimeAttachment) {
    retained
        .source_message_refs
        .extend(duplicate.source_message_refs);
    retained
        .requested_delivery_refs
        .extend(duplicate.requested_delivery_refs);
    retained
        .terminal_delivery_refs
        .extend(duplicate.terminal_delivery_refs);
    retained.attached |= duplicate.attached;
    retained.attachment_request_in_flight |= duplicate.attachment_request_in_flight;
}

pub(super) fn request_defaults(state: &SharedState, admission: &LocalAdmission) -> CwrResult<()> {
    let root_message = admission
        .conversation
        .messages
        .iter()
        .find(|message| message.message_ref == admission.record.source_message_ref)
        .cloned()
        .ok_or_else(|| std::io::Error::other("new conversation root message is unavailable"))?;
    {
        let mut state = lock_state(state)?;
        if state.expected_communication_adapters > state.communication_adapters.len() {
            state
                .pending_default_conversation_refs
                .insert(admission.record.conversation_ref.clone());
            persist(&state)?;
        }
    }
    request_default_conversation(state, &admission.record.conversation_ref, &root_message)?;
    let mut state = lock_state(state)?;
    if state.communication_adapters.len() >= state.expected_communication_adapters
        && state
            .pending_default_conversation_refs
            .remove(&admission.record.conversation_ref)
    {
        persist(&state)?;
    }
    Ok(())
}

fn request_default_conversation(
    state: &SharedState,
    conversation_ref: &str,
    root_message: &cwr::v2026_09::conversation::ConversationMessage,
) -> CwrResult<()> {
    let runtime_ref = lock_state(state)?.runtime_ref.clone();
    let adapters = lock_state(state)?
        .communication_adapters
        .iter()
        .flat_map(|(integration_ref, adapter)| {
            adapter
                .destinations
                .iter()
                .filter(|destination| destination.default_for_new_conversations)
                .map(|destination| (integration_ref.clone(), destination.clone()))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    for (integration_ref, destination) in adapters {
        let already_retained = lock_state(state)?.attachments.values().any(|attachment| {
            attachment.conversation_ref == conversation_ref
                && attachment.integration_ref == integration_ref
                && attachment.destination_ref == destination.destination_ref
        });
        if already_retained {
            continue;
        }
        let request = RuntimeAttachmentRequest {
            attachment_ref: format!("{runtime_ref}/attachments/{}", Uuid::new_v4().simple()),
            conversation_ref: conversation_ref.into(),
            integration_ref,
            destination_ref: destination.destination_ref,
            root_message: root_message.clone_for_external_delivery(),
        };
        {
            let mut state = lock_state(state)?;
            state.attachments.insert(
                request.attachment_ref.clone(),
                RuntimeAttachment {
                    conversation_ref: request.conversation_ref.clone(),
                    integration_ref: request.integration_ref.clone(),
                    destination_ref: request.destination_ref.clone(),
                    binding_key: None,
                    source_message_refs: BTreeSet::from([request.root_message.message_ref.clone()]),
                    attached: false,
                    attachment_request_in_flight: false,
                    requested_delivery_refs: BTreeSet::new(),
                    terminal_delivery_refs: BTreeSet::new(),
                },
            );
            persist(&state)?;
        }
        request_attachment(state, &request.attachment_ref)?;
    }
    Ok(())
}

pub(super) fn backfill_pending_defaults(state: &SharedState) -> CwrResult<()> {
    let (runtime, pending) = {
        let state = lock_state(state)?;
        (
            state.runtime.clone(),
            state
                .pending_default_conversation_refs
                .iter()
                .cloned()
                .collect::<Vec<_>>(),
        )
    };
    for conversation_ref in &pending {
        let conversation = runtime
            .conversation(conversation_ref)?
            .ok_or_else(|| std::io::Error::other("pending default conversation is unavailable"))?;
        let root_message = conversation
            .messages
            .first()
            .ok_or_else(|| std::io::Error::other("pending default root message is unavailable"))?;
        request_default_conversation(state, conversation_ref, root_message)?;
    }
    let mut state = lock_state(state)?;
    if state.communication_adapters.len() >= state.expected_communication_adapters {
        for conversation_ref in pending {
            state
                .pending_default_conversation_refs
                .remove(&conversation_ref);
        }
        persist(&state)?;
    }
    Ok(())
}

fn request_attachment(state: &SharedState, attachment_ref: &str) -> CwrResult<()> {
    let (writer, request) = {
        let mut state = lock_state(state)?;
        let runtime = state.runtime.clone();
        let attachment = state
            .attachments
            .get(attachment_ref)
            .ok_or_else(|| std::io::Error::other(format!("unknown attachment {attachment_ref}")))?;
        if attachment.attached || attachment.attachment_request_in_flight {
            return Ok(());
        }
        let integration_ref = attachment.integration_ref.clone();
        let conversation_ref = attachment.conversation_ref.clone();
        let destination_ref = attachment.destination_ref.clone();
        let source_message_refs = attachment.source_message_refs.clone();
        let Some(writer) = state
            .communication_adapters
            .get(&integration_ref)
            .map(|adapter| Arc::clone(&adapter.writer))
        else {
            return Ok(());
        };
        let conversation = runtime
            .conversation(&conversation_ref)?
            .ok_or_else(|| std::io::Error::other("attachment conversation is unavailable"))?;
        let root_message = conversation
            .messages
            .iter()
            .find(|message| source_message_refs.contains(&message.message_ref))
            .cloned()
            .ok_or_else(|| std::io::Error::other("attachment root message is unavailable"))?;
        state
            .attachments
            .get_mut(attachment_ref)
            .expect("attachment remains present while runtime state is locked")
            .attachment_request_in_flight = true;
        (
            writer,
            RuntimeAttachmentRequest {
                attachment_ref: attachment_ref.into(),
                conversation_ref,
                integration_ref,
                destination_ref,
                root_message,
            },
        )
    };
    if let Err(error) = write_event(&writer, &RuntimeServiceEvent::CreateAttachment { request }) {
        if let Ok(mut state) = lock_state(state) {
            if let Some(attachment) = state.attachments.get_mut(attachment_ref) {
                attachment.attachment_request_in_flight = false;
            }
        }
        return Err(error);
    }
    Ok(())
}

pub(super) fn request_deliveries(state: &SharedState, conversation_ref: &str) -> CwrResult<()> {
    let deliveries = {
        let mut state = lock_state(state)?;
        let Some(conversation) = state.runtime.conversation(conversation_ref)? else {
            return Ok(());
        };
        let runtime = state.runtime.clone();
        let adapters = state
            .communication_adapters
            .iter()
            .map(|(integration_ref, adapter)| {
                (integration_ref.clone(), Arc::clone(&adapter.writer))
            })
            .collect::<BTreeMap<_, _>>();
        let mut deliveries = Vec::new();
        for (attachment_ref, attachment) in &mut state.attachments {
            if !attachment.attached || attachment.conversation_ref != conversation_ref {
                continue;
            }
            let Some(writer) = adapters.get(&attachment.integration_ref) else {
                continue;
            };
            for message in &conversation.messages {
                if attachment
                    .source_message_refs
                    .contains(&message.message_ref)
                    || attachment
                        .terminal_delivery_refs
                        .contains(&message.message_ref)
                    || !attachment
                        .requested_delivery_refs
                        .insert(message.message_ref.clone())
                {
                    continue;
                }
                deliveries.push((
                    Arc::clone(writer),
                    RuntimeDeliveryRequest {
                        attachment_ref: attachment_ref.clone(),
                        delivery_ref: message.message_ref.clone(),
                        conversation_ref: conversation_ref.into(),
                        invocation_ref: runtime.delivery_invocation_ref(message)?,
                        message: message.clone_for_external_delivery(),
                        explicit_recipient_actor_refs: runtime
                            .explicit_reply_recipient_actor_refs(&message.message_ref)?,
                    },
                ));
            }
        }
        deliveries
    };
    for (writer, request) in deliveries {
        if let Err(error) = write_event(
            &writer,
            &RuntimeServiceEvent::DeliverMessage {
                request: request.clone(),
            },
        ) {
            clear_requested_delivery(state, &request.attachment_ref, &request.delivery_ref)?;
            return Err(error);
        }
    }
    Ok(())
}

pub(super) fn mark_attached(state: &SharedState, attachment_ref: &str) -> CwrResult<String> {
    let mut state = lock_state(state)?;
    let attachment = state
        .attachments
        .get_mut(attachment_ref)
        .ok_or_else(|| std::io::Error::other(format!("unknown attachment {attachment_ref}")))?;
    attachment.attached = true;
    attachment.attachment_request_in_flight = false;
    let conversation_ref = attachment.conversation_ref.clone();
    persist(&state)?;
    Ok(conversation_ref)
}

pub(super) fn replay_for_adapter(state: &SharedState, integration_ref: &str) -> CwrResult<()> {
    let (attachment_refs, conversation_refs) = {
        let mut state = lock_state(state)?;
        if !state.communication_adapters.contains_key(integration_ref) {
            return Err(std::io::Error::other("registered adapter is unavailable").into());
        }
        let mut attachment_refs = Vec::new();
        let mut conversations = BTreeSet::new();
        for (attachment_ref, attachment) in &mut state.attachments {
            if attachment.integration_ref != integration_ref {
                continue;
            }
            if attachment.attached {
                attachment.requested_delivery_refs.clear();
                conversations.insert(attachment.conversation_ref.clone());
                continue;
            }
            attachment.attachment_request_in_flight = false;
            attachment_refs.push(attachment_ref.clone());
        }
        (attachment_refs, conversations)
    };
    for attachment_ref in attachment_refs {
        request_attachment(state, &attachment_ref)?;
    }
    for conversation_ref in conversation_refs {
        request_deliveries(state, &conversation_ref)?;
    }
    Ok(())
}

pub(super) fn retry_attachment_after_failure(
    state: &SharedState,
    attachment_ref: &str,
) -> CwrResult<()> {
    {
        let mut state = lock_state(state)?;
        let attachment = state
            .attachments
            .get_mut(attachment_ref)
            .ok_or_else(|| std::io::Error::other(format!("unknown attachment {attachment_ref}")))?;
        if attachment.attached {
            return Ok(());
        }
        attachment.attachment_request_in_flight = false;
    }
    schedule_retry(Arc::clone(state), None, {
        let attachment_ref = attachment_ref.to_owned();
        move |state| request_attachment(state, &attachment_ref)
    })
}

pub(super) fn retry_delivery_after_failure(
    state: &SharedState,
    attachment_ref: &str,
    delivery_ref: &str,
    delay: Option<std::time::Duration>,
) -> CwrResult<()> {
    let conversation_ref = {
        let mut state = lock_state(state)?;
        let attachment = state
            .attachments
            .get_mut(attachment_ref)
            .ok_or_else(|| std::io::Error::other(format!("unknown attachment {attachment_ref}")))?;
        if !attachment.requested_delivery_refs.remove(delivery_ref) {
            return Ok(());
        }
        attachment.conversation_ref.clone()
    };
    schedule_retry(Arc::clone(state), delay, move |state| {
        request_deliveries(state, &conversation_ref)
    })
}

pub(super) fn retain_permanent_delivery_failure(
    state: &SharedState,
    attachment_ref: &str,
    delivery_ref: &str,
) -> CwrResult<()> {
    let mut state = lock_state(state)?;
    let attachment = state
        .attachments
        .get_mut(attachment_ref)
        .ok_or_else(|| std::io::Error::other(format!("unknown attachment {attachment_ref}")))?;
    if attachment.requested_delivery_refs.remove(delivery_ref) {
        attachment
            .terminal_delivery_refs
            .insert(delivery_ref.into());
        persist(&state)?;
    }
    Ok(())
}

fn clear_requested_delivery(
    state: &SharedState,
    attachment_ref: &str,
    delivery_ref: &str,
) -> CwrResult<()> {
    let mut state = lock_state(state)?;
    let attachment = state
        .attachments
        .get_mut(attachment_ref)
        .ok_or_else(|| std::io::Error::other(format!("unknown attachment {attachment_ref}")))?;
    attachment.requested_delivery_refs.remove(delivery_ref);
    Ok(())
}

fn schedule_retry(
    state: SharedState,
    delay: Option<std::time::Duration>,
    retry: impl FnOnce(&SharedState) -> CwrResult<()> + Send + 'static,
) -> CwrResult<()> {
    thread::Builder::new()
        .name("cwp-runtime-delivery-retry".into())
        .spawn(move || {
            let delay = delay.unwrap_or_else(|| {
                if cfg!(test) {
                    std::time::Duration::ZERO
                } else {
                    std::time::Duration::from_secs(1)
                }
            });
            thread::sleep(delay);
            let _ = retry(&state);
        })?;
    Ok(())
}

pub(super) fn retain_successful_delivery(
    state: &SharedState,
    attachment_ref: &str,
    delivery_ref: &str,
) -> CwrResult<()> {
    let mut state = lock_state(state)?;
    let attachment = state
        .attachments
        .get_mut(attachment_ref)
        .ok_or_else(|| std::io::Error::other(format!("unknown attachment {attachment_ref}")))?;
    if !attachment.requested_delivery_refs.remove(delivery_ref) {
        return invalid(format!(
            "attachment {attachment_ref} has no requested delivery {delivery_ref}"
        ));
    }
    attachment
        .terminal_delivery_refs
        .insert(delivery_ref.into());
    persist(&state)
}

fn persist(state: &RuntimeServerState) -> CwrResult<()> {
    if let Some(parent) = state.attachment_state_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = state.attachment_state_path.with_extension("json.tmp");
    fs::write(
        &temporary,
        serde_json::to_vec_pretty(&RuntimeAttachmentStateFile {
            version: 1,
            attachments: state.attachments.clone(),
            pending_default_conversation_refs: state.pending_default_conversation_refs.clone(),
        })?,
    )?;
    fs::rename(temporary, &state.attachment_state_path)?;
    Ok(())
}
