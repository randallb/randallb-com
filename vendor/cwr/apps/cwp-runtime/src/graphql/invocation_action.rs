//! Action presentation comes from retained typed calls, never message text.
use super::schema::{blocking, ReadService};
use async_graphql::{Enum, SimpleObject, ID};
use cwr::v2026_09::conversation::ConversationActionEvent;

#[derive(Clone, SimpleObject)]
pub(super) struct InvocationAction {
    selector: String,
    title: String,
    /// Already-public source text; private invocation inputs stay private.
    request: String,
    links: Vec<ActionResultLink>,
    result_message: Option<super::schema::Message>,
}

#[derive(Clone, Copy, PartialEq, Eq, Enum)]
pub(super) enum ActionResultLinkKind {
    Conversation,
}

#[derive(Clone, SimpleObject)]
pub(super) struct ActionResultLink {
    label: String,
    kind: ActionResultLinkKind,
    reference: ID,
}

pub(super) async fn project(
    service: Option<ReadService>,
    source: Option<ID>,
    invocation: ID,
) -> async_graphql::Result<Option<InvocationAction>> {
    let (Some(service), Some(source)) = (service, source) else {
        return Ok(None);
    };
    let title_service = service.clone();
    let mut projected = blocking(service.reads.clone(), service.budget(), move || {
        let metadata = service
            .publications
            .read(&service, |view| view.message_metadata(source.as_str()))?;
        let Some(metadata) = metadata else {
            return Ok(None);
        };
        let Some(ConversationActionEvent::Call { activation, .. }) = metadata.message.action else {
            return Ok(None);
        };
        let title = action_title(&activation);
        let destination = service.publications.read(&service, |view| {
            view.invocation_fork_destination(invocation.as_str())
        })?;
        let links = destination
            .into_iter()
            .map(|reference| ActionResultLink {
                label: "Open fork".into(),
                kind: ActionResultLinkKind::Conversation,
                reference: ID(reference),
            })
            .collect();
        let result_message = service
            .publications
            .read(&service, |view| {
                view.invocation_result_message(invocation.as_str())
            })?
            .map(|message| super::schema::Message {
                service: service.clone(),
                id: ID(message.message_ref),
                author_actor_ref: ID(message.author_actor_ref),
                addressed_actor_refs: message.addressed_actor_refs.into_iter().map(ID).collect(),
                reply_to_message_ref: message.reply_to_message_ref.map(ID),
                text: message.text,
            });
        Ok(Some(InvocationAction {
            selector: activation,
            title,
            request: metadata.message.text,
            links,
            result_message,
        }))
    })
    .await?;
    if let Some(action) = projected.as_mut() {
        action.title = title_service
            .action_title(&action.selector)
            .await?
            .unwrap_or_else(|| action_title(&action.selector));
    }
    Ok(projected)
}

fn action_title(selector: &str) -> String {
    let name = selector.split_once("--").map_or(selector, |(_, name)| name);
    let words = name.replace(['-', '_'], " ");
    let words = words.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = words.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => "Action".into(),
    }
}

/// Resolve through the same frozen participant view as the invocation itself.
pub(super) async fn result_message(
    service: Option<ReadService>,
    reference: String,
) -> async_graphql::Result<Option<super::schema::Message>> {
    let Some(service) = service else {
        return Ok(None);
    };
    super::schema::blocking(service.reads.clone(), service.budget(), move || {
        Ok(service
            .publications
            .read(&service, |view| view.invocation_result_message(&reference))?
            .map(|message| super::schema::Message {
                service: service.clone(),
                id: ID(message.message_ref),
                author_actor_ref: ID(message.author_actor_ref),
                addressed_actor_refs: message.addressed_actor_refs.into_iter().map(ID).collect(),
                reply_to_message_ref: message.reply_to_message_ref.map(ID),
                text: message.text,
            }))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn labels_cover_intrinsic_and_external_actions() {
        assert_eq!(
            action_title("@cwr--fork--conversation"),
            "Fork conversation"
        );
        assert_eq!(action_title("@ops--update--stack"), "Update stack");
    }
}
