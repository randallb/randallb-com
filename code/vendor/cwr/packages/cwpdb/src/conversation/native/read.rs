use super::*;
use rusqlite::{params, OptionalExtension};
use sha2::{Digest, Sha256};
mod selectors;
#[derive(Clone)]
pub(super) struct Cursor {
    fingerprint: String,
    revision: String,
    phase: u8,
    group: usize,
    source: i64,
    target: i64,
}
const BOOT_RECORDS: &[RecordKind] = &[
    RecordKind::Runtime,
    RecordKind::Conversation,
    RecordKind::Admission,
    RecordKind::Invocation,
    RecordKind::Wait,
    RecordKind::ToolAttempt,
    RecordKind::ToolOperation,
    RecordKind::ToolFile,
    RecordKind::ConversationTimelineConnection,
    RecordKind::ConversationTimelineItem,
    RecordKind::UpstreamDeclaration,
    RecordKind::LocalActorIdentity,
    RecordKind::Message,
];
const COLD_RECORDS: &[RecordKind] = &[
    RecordKind::Conversation,
    RecordKind::Message,
    RecordKind::Activity,
    RecordKind::ConversationTransferMetadata,
];
const BOOT_RELATIONS: &[(&str, &str)] = &[
    ("runtime", "conversation"),
    ("runtime", "root_conversation"),
    ("runtime", "admission"),
    ("runtime", "wait"),
    ("external", "upstream_declaration"),
    ("conversation", "parent"),
    ("conversation", "participant"),
    ("conversation", "archived"),
    ("conversation", "participant_version"),
    ("conversation", "conversation_timeline_root"),
    ("invocation", "execution"),
    ("invocation", "tool_attempt"),
    ("invocation", "tool_operation"),
    ("tool_operation", "tool_file"),
    ("tool_operation", "child_invocation"),
    (
        "conversation_timeline_connection",
        "conversation_timeline_item",
    ),
    ("conversation_timeline_item", "conversation_timeline_child"),
    (
        "conversation_timeline_item",
        "conversation_timeline_message",
    ),
    (
        "conversation_timeline_item",
        "conversation_timeline_invocation",
    ),
];
const COLD_RELATIONS: &[(&str, &str)] = &[
    ("conversation", "parent"),
    ("conversation", "participant"),
    ("conversation", "archived"),
    ("conversation", "message"),
    ("conversation", "activity"),
    ("conversation", "transfer_metadata"),
    ("conversation", "conversation_timeline_root"),
];
fn fingerprint(request: &ReadRequest) -> CwpdbResult<String> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(&json!([
        request.scope,
        request.selection,
        request.page_size,
        request.max_bytes
    ]))?)))
}
pub(super) fn get(db: &Connection, reference: &str) -> CwpdbResult<Option<Record>> {
    let row:Option<(String,String,String,i64)>=db.query_row("SELECT n.kind,s.canonical_ref,json(n.props),n.revision FROM nodes n JOIN ref_bindings r ON r.ref_id=n.ref_id JOIN ref_bindings s ON s.ref_id=n.schema_id WHERE r.canonical_ref=?1",[reference],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).optional()?;
    row.map(|(kind, schema_ref, props, counter)| {
        Ok(Record {
            reference: reference.into(),
            kind: parse_kind(&kind)?,
            schema_ref,
            properties: serde_json::from_str(&props)?,
            revision: ref_revision(reference, counter),
        })
    })
    .transpose()
}
impl NativeConversationStore {
    pub(super) fn read_page(
        &mut self,
        request: &ReadRequest,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<SnapshotPage> {
        if serde_json::to_vec(request)?.len() > MAX_WIRE_BYTES {
            return Err("CWR read request exceeds24MiB wire budget".into());
        }
        self.check_scope(&request.scope)?;
        if request.page_size == 0
            || request.page_size > 1000
            || request.max_bytes == 0
            || request.max_bytes > MAX_WIRE_BYTES
        {
            return Err("invalid CWR page limits".into());
        }
        let fingerprint = fingerprint(request)?;
        let mut cursor = match &request.continuation {
            Some(token) => self
                .cursors
                .get(token)
                .filter(|cursor| cursor.fingerprint == fingerprint)
                .cloned()
                .ok_or("invalid CWR read continuation")?,
            None => Cursor {
                fingerprint,
                revision: String::new(),
                phase: 0,
                group: 0,
                source: 0,
                target: 0,
            },
        };
        let tx = self.connection.transaction()?;
        let target = match &request.selection {
            Selection::Conversation { conversation_ref } => conversation_ref.as_str(),
            Selection::ConversationMetadata {
                conversation_ref: Some(reference),
            } => reference.as_str(),
            _ => request.scope.runtime_root_ref.as_str(),
        };
        authorizer.authorize(&request.scope.context, target, false)?;
        let (revision, _) = current_revision(&tx, &request.scope)?;
        if request
            .expected_revision
            .as_ref()
            .is_some_and(|expected| expected != &revision)
            || !cursor.revision.is_empty() && cursor.revision != revision
        {
            return Err("conversation revision_conflict".into());
        }
        cursor.revision = revision.clone();
        let mut metadata = runtime_metadata(&tx, &request.scope)?;
        metadata["storage_maintenance"] =
            serde_json::to_value(maintenance::needed(&tx, &request.scope)?)?;
        let mut page = SnapshotPage {
            revision,
            metadata,
            records: Vec::new(),
            relationships: Vec::new(),
            continuation: None,
            complete: false,
        };
        let mut bytes = serde_json::to_vec(&page)?.len() + 128;
        if bytes > request.max_bytes {
            return Err("CWR metadata exceeds page byte budget".into());
        }
        let records = match &request.selection {
            Selection::Bootstrap => BOOT_RECORDS.to_vec(),
            Selection::Conversation { .. } => COLD_RECORDS.to_vec(),
            Selection::Records { kind } => vec![*kind],
            Selection::ConversationMetadata { .. } => vec![
                RecordKind::Conversation,
                RecordKind::Message,
                RecordKind::ConversationTransferMetadata,
            ],
            Selection::Commentary { .. } => vec![RecordKind::Activity],
        };
        while cursor.phase == 0 && cursor.group < records.len() {
            let kind = records[cursor.group];
            let filter=match &request.selection{
    Selection::Bootstrap if kind==RecordKind::Conversation=>"AND (EXISTS(SELECT 1 FROM edges own WHERE own.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?3) AND own.kind='conversation' AND own.target_id=n.ref_id) OR r.claim_owner_ref=r.canonical_ref AND r.claim_kind='node:conversation')",
    Selection::Bootstrap if kind==RecordKind::Message=>"AND json_type(n.props,'$.streaming')='object'",
    Selection::Conversation{..} if kind==RecordKind::Conversation=>"AND r.canonical_ref=?4",
    Selection::Conversation{..}=>"AND EXISTS(SELECT 1 FROM edges own WHERE own.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?4) AND own.target_id=n.ref_id AND own.kind IN ('message','activity','transfer_metadata'))",
    Selection::ConversationMetadata{..}=> selectors::metadata_record_filter(kind),
    Selection::Commentary{..}=>"AND EXISTS(SELECT 1 FROM edges lookup WHERE lookup.target_id=n.ref_id AND lookup.kind='commentary' AND lookup.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?4))",
    _=>""
   };
            let selection_reference = selectors::reference(&request.selection);
            let conversation = selection_reference.as_str();
            let sql=format!("SELECT n.ref_id,r.canonical_ref,s.canonical_ref,json(n.props),n.revision FROM nodes n INDEXED BY nodes_by_kind JOIN ref_bindings r ON r.ref_id=n.ref_id JOIN ref_bindings s ON s.ref_id=n.schema_id WHERE n.kind=?1 AND n.ref_id>?2 {filter} AND (?3 IS NOT NULL OR ?4 IS NOT NULL) ORDER BY n.ref_id LIMIT ?5");
            let mut statement = tx.prepare(&sql)?;
            let mut rows = statement.query(params![
                kind.as_str(),
                cursor.target,
                root_ref(&request.scope),
                conversation,
                (request.page_size + 1) as i64
            ])?;
            while let Some(row) = rows.next()? {
                let id: i64 = row.get(0)?;
                let reference: String = row.get(1)?;
                let counter: i64 = row.get(4)?;
                let record = Record {
                    revision: ref_revision(&reference, counter),
                    reference,
                    kind,
                    schema_ref: row.get(2)?,
                    properties: serde_json::from_str(&row.get::<_, String>(3)?)?,
                };
                let size = serde_json::to_vec(&record)?.len();
                if !fits(&page, request, bytes, size)? {
                    drop(rows);
                    drop(statement);
                    tx.commit()?;
                    return self.finish_page(page, cursor);
                }
                bytes += size + 1;
                page.records.push(record);
                cursor.target = id;
            }
            cursor.group += 1;
            cursor.target = 0;
        }
        if cursor.phase == 0 {
            cursor.phase = 1;
            cursor.group = 0;
            cursor.source = 0;
            cursor.target = 0;
        }
        let relations = match &request.selection {
            Selection::Bootstrap => BOOT_RELATIONS,
            Selection::Conversation { .. } => COLD_RELATIONS,
            Selection::Records { .. } => &[],
            Selection::ConversationMetadata { .. } => selectors::METADATA_RELATIONS,
            Selection::Commentary { .. } => &[("conversation", "activity")],
        };
        while cursor.group < relations.len() {
            let (source_kind, kind) = relations[cursor.group];
            let selection_reference = selectors::reference(&request.selection);
            let conversation = selection_reference.as_str();
            let source_filter =
                selectors::relationship_filter(&request.selection, source_kind, kind);
            let sql=format!("SELECT e.source_id,e.target_id,r.canonical_ref,source.canonical_ref,target.canonical_ref,e.position,json(e.props),e.revision,schema.canonical_ref FROM nodes owner INDEXED BY nodes_by_kind JOIN ref_bindings source ON source.ref_id=owner.ref_id JOIN edges e INDEXED BY edges_out ON e.source_id=owner.ref_id AND e.kind=?2 JOIN ref_bindings r ON r.ref_id=e.ref_id JOIN ref_bindings schema ON schema.ref_id=e.schema_id JOIN ref_bindings target ON target.ref_id=e.target_id WHERE owner.kind=?1 AND owner.ref_id>=?3 AND (owner.ref_id>?3 OR e.target_id>?4) {source_filter} AND (?5 IS NOT NULL OR ?6 IS NOT NULL) ORDER BY owner.ref_id,e.target_id LIMIT ?7");
            let sql = if source_kind == "external" {
                "SELECT e.source_id,e.target_id,r.canonical_ref,source.canonical_ref,target.canonical_ref,e.position,json(e.props),e.revision,schema.canonical_ref FROM nodes owner INDEXED BY nodes_by_kind JOIN edges e INDEXED BY edges_in ON e.target_id=owner.ref_id AND e.kind=?2 JOIN ref_bindings source ON source.ref_id=e.source_id JOIN ref_bindings r ON r.ref_id=e.ref_id JOIN ref_bindings schema ON schema.ref_id=e.schema_id JOIN ref_bindings target ON target.ref_id=e.target_id WHERE owner.kind='upstream_declaration' AND e.source_id>=?3 AND (e.source_id>?3 OR e.target_id>?4) AND (?1 IS NOT NULL OR ?5 IS NOT NULL OR ?6 IS NOT NULL) ORDER BY e.source_id,e.target_id LIMIT ?7".to_owned()
            } else {
                sql
            };
            let mut statement = tx.prepare(&sql)?;
            let mut rows = statement.query(params![
                source_kind,
                kind,
                cursor.source,
                cursor.target,
                conversation,
                root_ref(&request.scope),
                (request.page_size + 1) as i64
            ])?;
            while let Some(row) = rows.next()? {
                let schema_ref: String = row.get(8)?;
                if schema_ref != crate::EDGE_SCHEMA_REF {
                    return Err(
                        format!("Unsupported retained relationship schema: {schema_ref}").into(),
                    );
                }
                let source: i64 = row.get(0)?;
                let target: i64 = row.get(1)?;
                let reference: String = row.get(2)?;
                let counter: i64 = row.get(7)?;
                let relation = Relationship {
                    revision: ref_revision(&reference, counter),
                    reference,
                    source_ref: row.get(3)?,
                    kind: kind.into(),
                    target_ref: row.get(4)?,
                    position: row.get(5)?,
                    properties: serde_json::from_str(&row.get::<_, String>(6)?)?,
                };
                let size = serde_json::to_vec(&relation)?.len();
                if !fits(&page, request, bytes, size)? {
                    drop(rows);
                    drop(statement);
                    tx.commit()?;
                    return self.finish_page(page, cursor);
                }
                bytes += size + 1;
                page.relationships.push(relation);
                cursor.source = source;
                cursor.target = target;
            }
            cursor.group += 1;
            cursor.source = 0;
            cursor.target = 0;
        }
        tx.commit()?;
        page.complete = true;
        Ok(page)
    }
    fn finish_page(&mut self, mut page: SnapshotPage, cursor: Cursor) -> CwpdbResult<SnapshotPage> {
        self.next_cursor = self
            .next_cursor
            .checked_add(1)
            .ok_or("CWR cursor sequence exhausted")?;
        let token = hex::encode(Sha256::digest(serde_json::to_vec(&json!([
            cursor.fingerprint,
            cursor.revision,
            self.next_cursor,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
                .to_string(),
            std::process::id()
        ]))?));
        if self.cursors.len() >= 4096 {
            self.cursors.clear();
        }
        self.cursors.insert(token.clone(), cursor);
        page.continuation = Some(token);
        Ok(page)
    }
}
fn fits(
    page: &SnapshotPage,
    request: &ReadRequest,
    bytes: usize,
    size: usize,
) -> CwpdbResult<bool> {
    let count = page.records.len() + page.relationships.len();
    if count == 0 && bytes + size + 1 > request.max_bytes {
        return Err(format!("CWR record requires at least {} page bytes", bytes + size).into());
    }
    Ok(count < request.page_size && bytes + size < request.max_bytes)
}
