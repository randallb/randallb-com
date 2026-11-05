//! Rootless retained history has position cursors, never canonical placement IDs.
use super::{
    publications::{resource_node, Cursor, Node},
    schema::{blocking, PageInfo, ReadService},
};
use async_graphql::{Result, SimpleObject};
use cwr::v2026_09::conversation::{
    ConversationPositionKey, ConversationRecordPageRequest, ConversationResource,
    ConversationTimelineAnchor,
};

#[derive(SimpleObject)]
pub(super) struct LegacyHistoryPage {
    revision: String,
    page_info: PageInfo,
    edges: Vec<LegacyHistoryEdge>,
}

#[derive(SimpleObject)]
struct LegacyHistoryEdge {
    cursor: Cursor,
    node: Option<Node>,
}

pub(super) async fn page(
    service: ReadService,
    conversation: String,
    last: i32,
    before: Option<Cursor>,
) -> Result<Option<LegacyHistoryPage>> {
    if !(1..=100).contains(&last) {
        return Err("Page size must be between 1 and 100".into());
    }
    blocking(service.reads.clone(), service.budget(), move || {
        service.catalog()?;
        service.publications.read(&service, |view| {
            if view.timeline_layout(&conversation)?.is_some() {
                return Ok(None);
            }
            let scope = (&service.actor_ref, &service.owner, &conversation);
            let before = before
                .map(|cursor| -> cwr::CwrResult<ConversationPositionKey> {
                    let (actor, owner, reference, key): (
                        String,
                        Option<String>,
                        String,
                        ConversationPositionKey,
                    ) = serde_json::from_str(&cursor.0)?;
                    if (actor, owner, reference)
                        != (scope.0.clone(), scope.1.clone(), scope.2.clone())
                    {
                        return Err("Invalid legacy cursor authorization scope".into());
                    }
                    Ok(key)
                })
                .transpose()?;
            let Some(page) = view.legacy_page(
                &conversation,
                &ConversationRecordPageRequest {
                    last: Some(last as usize),
                    before,
                    ..Default::default()
                },
            )?
            else {
                return Ok(None);
            };
            let mut edges = Vec::with_capacity(page.anchors.len());
            for anchor in page.anchors {
                let (key, resource) = match anchor {
                    ConversationTimelineAnchor::Message { key, message_ref } => {
                        let resource = view
                            .legacy_selected_message(&conversation, &key, &message_ref)?
                            .map(|message| ConversationResource::Message {
                                message: Box::new(message),
                                streaming: false,
                                occurred_at_unix_ms: None,
                            });
                        (key, resource)
                    }
                    ConversationTimelineAnchor::Turn {
                        key,
                        invocation_ref,
                    } => (
                        key,
                        view.resource(&invocation_ref)?.map(|value| value.resource),
                    ),
                };
                edges.push(LegacyHistoryEdge {
                    cursor: Cursor(serde_json::to_string(&(
                        &service.actor_ref,
                        &service.owner,
                        &conversation,
                        key,
                    ))?),
                    node: resource
                        .map(|resource| resource_node(resource, service.clone()))
                        .transpose()?,
                });
            }
            Ok(Some(LegacyHistoryPage {
                revision: page.revision,
                page_info: PageInfo::new(
                    page.has_next_page,
                    page.has_previous_page,
                    edges.first().map(|edge| edge.cursor.0.clone()),
                    edges.last().map(|edge| edge.cursor.0.clone()),
                ),
                edges,
            }))
        })
    })
    .await
}
