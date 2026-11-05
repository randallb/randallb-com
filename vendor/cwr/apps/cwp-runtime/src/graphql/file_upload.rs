//! Raw uploads share the GraphQL actor session and never accept host paths.
use super::{authorize, HttpState};
use axum::{
    extract::{Query, Request, State},
    http::StatusCode,
    Json,
};
use cwr::v2026_09::conversation::MAX_FILE_BYTES;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Metadata {
    filename: String,
    media_type: String,
    invocation_ref: Option<String>,
}

pub(super) async fn upload(
    State(state): State<HttpState>,
    Query(metadata): Query<Metadata>,
    request: Request,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let session = authorize(&state, request.headers())?;
    let _permit = state
        .permits
        .clone()
        .try_acquire_owned()
        .map_err(|_| StatusCode::TOO_MANY_REQUESTS)?;
    let _consumer = state
        .activity
        .connect()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let bytes = axum::body::to_bytes(request.into_body(), MAX_FILE_BYTES)
        .await
        .map_err(|_| StatusCode::PAYLOAD_TOO_LARGE)?;
    session
        .ensure_open()
        .map_err(|_| StatusCode::UNAUTHORIZED)?;
    let service = session.service.fresh_selection();
    let file =
        super::schema::blocking(
            service.reads.clone(),
            service.budget(),
            move || match metadata.invocation_ref {
                Some(invocation) => service.runtime.ingest_invocation_file_as(
                    &service.actor_ref,
                    &invocation,
                    &metadata.filename,
                    &metadata.media_type,
                    &bytes,
                ),
                None => service.runtime.ingest_file_as(
                    &service.actor_ref,
                    &metadata.filename,
                    &metadata.media_type,
                    &bytes,
                ),
            },
        )
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    Ok(Json(
        serde_json::json!({"id":file.file_ref,"filename":file.filename,
        "mediaType":file.media_type,"sizeBytes":file.size_bytes,"sha256":file.sha256}),
    ))
}
