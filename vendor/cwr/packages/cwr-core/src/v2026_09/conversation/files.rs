//! Files are immutable graph resources. Content is loaded only on an authorized read.
use super::*;

pub const MAX_FILE_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RetainedFile {
    pub file_ref: String,
    pub owner_actor_ref: String,
    /// Explicit readers derived by the runtime from invocation ancestry.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reader_actor_refs: Vec<String>,
    pub filename: String,
    pub media_type: String,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RetainedFileContent {
    pub metadata: RetainedFile,
    // Hex avoids large JSON arrays of integers and adds no binary transport assumptions.
    pub content_hex: String,
}

impl ConversationAdmissionStore {
    pub fn ingest_file(
        &mut self,
        owner_actor_ref: &str,
        filename: &str,
        media_type: &str,
        bytes: &[u8],
    ) -> CwrResult<RetainedFile> {
        self.ingest_file_for_readers(owner_actor_ref, Vec::new(), filename, media_type, bytes)
    }

    pub(crate) fn ingest_file_for_readers(
        &mut self,
        owner_actor_ref: &str,
        mut reader_actor_refs: Vec<String>,
        filename: &str,
        media_type: &str,
        bytes: &[u8],
    ) -> CwrResult<RetainedFile> {
        validate_ref("file owner", owner_actor_ref)?;
        for reader in &reader_actor_refs {
            validate_ref("file reader", reader)?;
        }
        reader_actor_refs.sort();
        reader_actor_refs.dedup();
        reader_actor_refs.retain(|reader| reader != owner_actor_ref);
        if bytes.len() > MAX_FILE_BYTES {
            return invalid("File exceeds the 10 MB limit");
        }
        if filename.trim().is_empty()
            || filename.len() > 255
            || filename
                .chars()
                .any(|c| c.is_control() || c == '/' || c == '\\')
        {
            return invalid("File name must be a nonempty name of at most 255 bytes");
        }
        if media_type.len() > 255
            || !media_type.contains('/')
            || !media_type.bytes().all(|b| b.is_ascii_graphic())
        {
            return invalid("File media type is invalid");
        }
        let metadata = RetainedFile {
            file_ref: new_ref(&self.state.runtime_root_ref, "files"),
            owner_actor_ref: owner_actor_ref.into(),
            reader_actor_refs,
            filename: filename.into(),
            media_type: media_type.into(),
            size_bytes: bytes.len() as u64,
            sha256: hex::encode(Sha256::digest(bytes)),
        };
        let content = RetainedFileContent {
            metadata: metadata.clone(),
            content_hex: hex::encode(bytes),
        };
        self.transact(|state| {
            state.files.insert(metadata.file_ref.clone(), content);
            Ok(())
        })?;
        Ok(metadata)
    }

    pub fn file_metadata(&self, actor_ref: &str, file_ref: &str) -> CwrResult<RetainedFile> {
        let record = self
            .backend
            .lock()
            .map_err(|_| invalid_error("conversation backend lock poisoned"))?
            .get(file_ref)?
            .filter(|record| record.kind == cwpdb_store::conversation::RecordKind::File)
            .ok_or_else(|| invalid_error("File unavailable"))?;
        let metadata: RetainedFile =
            serde_json::from_value(record.properties["record"]["metadata"].clone())?;
        if metadata.file_ref != file_ref {
            return invalid("File unavailable");
        }
        let loaded_participant = self.state.conversations.iter().any(|conversation| {
            conversation
                .participant_actor_refs
                .iter()
                .any(|actor| actor == actor_ref)
                && conversation.messages.iter().any(|message| {
                    message
                        .file_refs
                        .iter()
                        .any(|reference| reference == file_ref)
                })
        });
        let participant = if loaded_participant || self.state.cold_conversation_refs.is_empty() {
            loaded_participant
        } else {
            let mut participant = false;
            for conversation in self.state.conversations.iter().filter(|conversation| {
                conversation
                    .participant_actor_refs
                    .iter()
                    .any(|actor| actor == actor_ref)
            }) {
                if self
                    .conversation_history(&conversation.conversation_ref)?
                    .is_some_and(|conversation| {
                        conversation.messages.iter().any(|message| {
                            message
                                .file_refs
                                .iter()
                                .any(|reference| reference == file_ref)
                        })
                    })
                {
                    participant = true;
                    break;
                }
            }
            participant
        };
        if metadata.owner_actor_ref != actor_ref
            && !metadata
                .reader_actor_refs
                .iter()
                .any(|reader| reader == actor_ref)
            && !participant
        {
            return invalid("File unavailable");
        }
        Ok(metadata)
    }

    /// A reference alone never grants access to the bytes.
    pub fn read_file(&self, actor_ref: &str, file_ref: &str) -> CwrResult<RetainedFileContent> {
        self.file_metadata(actor_ref, file_ref)?;
        let record = self
            .backend
            .lock()
            .map_err(|_| invalid_error("conversation backend lock poisoned"))?
            .get(file_ref)?
            .filter(|record| record.kind == cwpdb_store::conversation::RecordKind::File)
            .ok_or_else(|| invalid_error("File unavailable"))?;
        let file: RetainedFileContent =
            serde_json::from_value(record.properties["record"].clone())?;
        let bytes = hex::decode(&file.content_hex)?;
        if bytes.len() as u64 != file.metadata.size_bytes
            || bytes.len() > MAX_FILE_BYTES
            || hex::encode(Sha256::digest(&bytes)) != file.metadata.sha256
            || file.metadata.file_ref != file_ref
        {
            return invalid("Retained file integrity check failed");
        }
        Ok(file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn custody_survives_restart_and_requires_owner() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("runtime.sqlite");
        let root = "cwp://example.test/runtime";
        let owner = "cwp://example.test/actors/owner";
        let mut store = ConversationAdmissionStore::open(&path, root).unwrap();
        let mut original = b"original file bytes".to_vec();
        let metadata = store
            .ingest_file(owner, "test.txt", "text/plain", &original)
            .unwrap();
        original.fill(0);
        drop(store);
        let store = ConversationAdmissionStore::open(&path, root).unwrap();
        let retained = store.read_file(owner, &metadata.file_ref).unwrap();
        assert_eq!(
            hex::decode(retained.content_hex).unwrap(),
            b"original file bytes"
        );
        assert!(store
            .read_file("cwp://example.test/actors/other", &metadata.file_ref)
            .is_err());
        assert!(store
            .read_file(owner, "cwp://example.test/files/missing")
            .is_err());
    }
    #[test]
    fn invalid_uploads_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let mut store = ConversationAdmissionStore::open(
            directory.path().join("db"),
            "cwp://example.test/runtime",
        )
        .unwrap();
        let owner = "cwp://example.test/actors/owner";
        assert!(store
            .ingest_file(owner, "../escape", "text/plain", b"x")
            .is_err());
        assert!(store.ingest_file(owner, "test", "invalid", b"x").is_err());
        assert!(store
            .ingest_file(owner, "test", "text/plain", &vec![0; MAX_FILE_BYTES + 1])
            .is_err());
    }
}
