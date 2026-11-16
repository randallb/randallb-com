use super::{
    messages::{actor, Actor},
    publications::Cursor,
    schema::{blocking, PageInfo, ReadService},
};
use async_graphql::{InputObject, Result, SimpleObject, ID};
#[derive(Default, InputObject)]
pub(super) struct ActorFilter {
    search: Option<String>,
    refs: Option<Vec<ID>>,
}
#[derive(SimpleObject)]
pub(super) struct ActorConnection {
    edges: Vec<ActorEdge>,
    page_info: PageInfo,
    total_count: i32,
}
#[derive(SimpleObject)]
struct ActorEdge {
    cursor: Cursor,
    node: Actor,
}
pub(super) async fn recipients(
    service: ReadService,
    message: String,
    filter: Option<ActorFilter>,
    first: Option<i32>,
    after: Option<Cursor>,
    last: Option<i32>,
    before: Option<Cursor>,
) -> Result<ActorConnection> {
    let filter = filter.unwrap_or_default();
    let filter = cwr::v2026_09::conversation::ConversationActorFilter {
        search: filter.search,
        refs: filter
            .refs
            .unwrap_or_default()
            .into_iter()
            .map(|id| id.to_string())
            .collect(),
    }
    .normalized()?;
    let scope = serde_json::to_string(&(
        &service.actor_ref,
        &service.owner,
        "recipients",
        &message,
        &filter,
    ))?;
    let window = super::windows::Window {
        first,
        after,
        last,
        before,
    };
    blocking(service.reads.clone(), service.budget(), move || {
        let values = service
            .publications
            .read(&service, |v| v.message_recipients(&message, &filter))?
            .ok_or_else(|| std::io::Error::other("Message unavailable"))?;
        let values = values
            .into_iter()
            .map(|(index, value)| (index, value.actor_ref.clone(), actor(value)))
            .collect();
        let page = window
            .select(&scope, values)
            .map_err(|e| std::io::Error::other(e.message))?;
        Ok(ActorConnection {
            edges: page
                .values
                .into_iter()
                .map(|(cursor, node)| ActorEdge { cursor, node })
                .collect(),
            page_info: page.info,
            total_count: page.total,
        })
    })
    .await
}
