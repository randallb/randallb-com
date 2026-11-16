//! File ingestion and reads use the session actor; filenames never select host paths.
use super::schema::{blocking, ReadService};
use async_graphql::{Context, Object, Result, SimpleObject, ID};
use base64::{engine::general_purpose::STANDARD, Engine};
use cwr::v2026_09::conversation::{RetainedFile, MAX_FILE_BYTES};

#[derive(Clone, SimpleObject)]
pub(super) struct FileMetadata {
    id: ID,
    filename: String,
    media_type: String,
    size_bytes: u64,
    sha256: String,
}
impl From<RetainedFile> for FileMetadata {
    fn from(file: RetainedFile) -> Self {
        Self {
            id: ID(file.file_ref),
            filename: file.filename,
            media_type: file.media_type,
            size_bytes: file.size_bytes,
            sha256: file.sha256,
        }
    }
}
#[derive(SimpleObject)]
pub(super) struct FileContent {
    file: FileMetadata,
    content_base64: String,
}
#[derive(Default)]
pub(super) struct FileMutations;
#[Object]
impl FileMutations {
    async fn ingest_file(
        &self,
        ctx: &Context<'_>,
        filename: String,
        media_type: String,
        content_base64: String,
    ) -> Result<FileMetadata> {
        if content_base64.len() > MAX_FILE_BYTES.div_ceil(3) * 4 {
            return Err("File exceeds the 10 MB limit".into());
        }
        let bytes = STANDARD.decode(content_base64)?;
        let service = ctx.data_unchecked::<ReadService>().clone();
        blocking(service.reads.clone(), service.budget(), move || {
            Ok(service
                .runtime
                .ingest_file_as(&service.actor_ref, &filename, &media_type, &bytes)?
                .into())
        })
        .await
    }
}
pub(super) async fn read(service: ReadService, reference: String) -> Result<FileContent> {
    blocking(service.reads.clone(), service.budget(), move || {
        let content = service
            .runtime
            .read_file_as(&service.actor_ref, &reference)?;
        let bytes = hex::decode(content.content_hex)?;
        Ok(FileContent {
            file: content.metadata.into(),
            content_base64: STANDARD.encode(bytes),
        })
    })
    .await
}

pub(super) async fn metadata(service: ReadService, reference: String) -> Result<FileMetadata> {
    blocking(service.reads.clone(), service.budget(), move || {
        Ok(service
            .runtime
            .file_metadata_as(&service.actor_ref, &reference)?
            .into())
    })
    .await
}
