//! Retained visualization metadata, with HTML left to explicit artifact reads.
use super::{messages::metadata, schema::Message};
use async_graphql::{Result, SimpleObject, ID};

#[derive(SimpleObject)]
pub(super) struct MessageVisualization {
    marker_index: i32,
    title: Option<String>,
    mode: Option<String>,
    error: Option<String>,
    artifact_ref: Option<ID>,
}
pub(super) async fn visualizations(message: &Message) -> Result<Vec<MessageVisualization>> {
    let Some(value) = metadata(message).await? else {
        return Ok(vec![]);
    };
    value
        .message
        .visualizations
        .into_iter()
        .map(|value| {
            Ok(MessageVisualization {
                marker_index: i32::try_from(value.marker_index)?,
                title: value.title,
                mode: value.mode,
                error: value.error,
                artifact_ref: value.artifact_ref.map(ID),
            })
        })
        .collect()
}
