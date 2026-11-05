use super::*;
use rusqlite::{params, OptionalExtension};
pub(super) mod invocation;
const LIMIT: i64 = 50000;
pub(super) fn one(db: &Connection, source: &str, kind: &str) -> CwpdbResult<Option<String>> {
    let mut query=db.prepare_cached("SELECT target.canonical_ref FROM edges e INDEXED BY edges_out JOIN ref_bindings target ON target.ref_id=e.target_id WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind=?2 ORDER BY e.position LIMIT 2")?;
    let found = query
        .query_map(params![source, kind], |r| r.get(0))?
        .collect::<Result<Vec<String>, _>>()?;
    if found.len() > 1 {
        return Err(format!("singleton relationship {kind} has multiple members").into());
    }
    Ok(found.into_iter().next())
}
pub(super) fn visible(
    db: &Connection,
    root: Option<&str>,
    reference: &str,
    kind: &str,
) -> CwpdbResult<bool> {
    let Some(root) = root else { return Ok(true) };
    // A direct placement is already a complete root-membership proof. Avoid
    // enumerating every repeated publication of the same canonical message.
    let direct: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM edges content INDEXED BY edges_in JOIN edges placement INDEXED BY edges_in ON placement.target_id=content.source_id AND placement.kind='conversation_timeline_item' WHERE content.target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2) AND content.kind=?3 AND placement.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1))", params![root, reference, kind], |row| row.get(0))?;
    if direct {
        return Ok(true);
    }

    let (count,depth,found):(i64,Option<i64>,bool)=db.query_row("WITH RECURSIVE ancestors(connection_id,depth) AS (
       SELECT placement.source_id,0 FROM edges content INDEXED BY edges_in
       JOIN edges placement ON placement.target_id=content.source_id AND placement.kind='conversation_timeline_item'
       WHERE content.target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2) AND content.kind=?3
       UNION SELECT placement.source_id,current.depth+1 FROM ancestors current
       JOIN edges child ON child.target_id=current.connection_id AND child.kind='conversation_timeline_child'
       JOIN edges placement ON placement.target_id=child.source_id AND placement.kind='conversation_timeline_item'
       WHERE current.depth<128 LIMIT 50001)
       SELECT COUNT(*),MAX(depth),COALESCE(MAX(connection_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)),0) FROM ancestors",params![root,reference,kind],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    if count > LIMIT || depth.is_some_and(|depth| depth >= 128) {
        return Err("resource visibility work limit exceeded".into());
    }
    Ok(found)
}
pub(super) fn message(
    db: &Connection,
    conversation: &str,
    reference: &str,
) -> CwpdbResult<Option<Value>> {
    let Some(node) = read::get(db, reference)?.filter(|n| n.kind == RecordKind::Message) else {
        return Ok(None);
    };
    if node.schema_ref != crate::schema_ref(RecordKind::Message.as_str()) {
        return Err("unsupported message schema".into());
    }
    let properties:Option<String>=db.query_row("SELECT json(props) FROM edges WHERE source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND kind='message' AND target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2)",params![conversation,reference],|r|r.get(0)).optional()?;
    let Some(properties) = properties else {
        return Ok(None);
    };
    let mut record = node.properties["record"].clone();
    let properties: Value = serde_json::from_str(&properties)?;
    if let Some(value) = properties.get("visualizations") {
        record
            .as_object_mut()
            .ok_or("invalid message record")?
            .insert("visualizations".into(), value.clone());
    }
    Ok(Some(record))
}
fn association(db: &Connection, node: &Record) -> CwpdbResult<Option<(String, bool, Value)>> {
    if node.kind == RecordKind::Invocation {
        return Ok(node.properties["record"]["conversation_ref"]
            .as_str()
            .map(|s| (s.into(), false, node.properties["record"].clone())));
    }
    if node.kind != RecordKind::Message {
        return Ok(None);
    };
    if let Some(stream) = node.properties.get("streaming").filter(|v| v.is_object()) {
        let reference = stream["conversation_ref"]
            .as_str()
            .ok_or("streaming conversation ref required")?;
        if stream["message"]["message_ref"] != node.reference {
            return Err("streaming message identity mismatch".into());
        }
        return Ok(Some((reference.into(), true, stream["message"].clone())));
    }
    let mut query=db.prepare_cached("SELECT source.canonical_ref FROM edges e INDEXED BY edges_in JOIN ref_bindings source ON source.ref_id=e.source_id WHERE e.target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind='message' ORDER BY source.canonical_ref LIMIT 2")?;
    let refs = query
        .query_map([&node.reference], |r| r.get(0))?
        .collect::<Result<Vec<String>, _>>()?;
    if refs.len() > 1 {
        return Err("canonical message ref belongs to multiple conversations".into());
    }
    Ok(refs
        .into_iter()
        .next()
        .map(|reference| (reference, false, Value::Null)))
}
pub(super) fn conversation_facts(
    db: &Connection,
    scope: &Scope,
    reference: &str,
) -> CwpdbResult<Option<ResourceConversationFacts>> {
    let owned: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM edges WHERE source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND kind='conversation' AND target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2))",params![root_ref(scope),reference],|row|row.get(0))?;
    if !owned {
        return Ok(None);
    }
    let Some(node) = read::get(db, reference)?.filter(|node| node.kind == RecordKind::Conversation)
    else {
        return Ok(None);
    };
    if node.schema_ref != crate::schema_ref(RecordKind::Conversation.as_str()) {
        return Err("unsupported conversation schema".into());
    }
    let parent_ref = one(db, reference, "parent")?;
    let root=db.query_row("SELECT EXISTS(SELECT 1 FROM edges WHERE source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND kind='root_conversation' AND target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2))",params![root_ref(scope),reference],|r|r.get(0))?;
    let mut query=db.prepare_cached("SELECT target.canonical_ref FROM edges e INDEXED BY edges_out JOIN ref_bindings target ON target.ref_id=e.target_id WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind='participant' ORDER BY e.position LIMIT 50001")?;
    let participants = query
        .query_map([reference], |r| r.get(0))?
        .collect::<Result<Vec<String>, _>>()?;
    if participants.len() > LIMIT as usize {
        return Err("resource participant limit exceeded".into());
    }
    Ok(Some(ResourceConversationFacts {
        owner_context_ref: node.properties["owner_context_ref"]
            .as_str()
            .map(str::to_owned),
        parent_ref,
        sidebar_eligible: node.properties["sidebar_eligible"]
            .as_bool()
            .unwrap_or(true),
        root,
        participant_actor_refs: participants,
    }))
}
impl NativeConversationStore {
    pub(super) fn resource_facts(
        &mut self,
        scope: &Scope,
        reference: &str,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<Option<ResourceFacts>> {
        self.check_scope(scope)?;
        valid_ref(reference)?;
        let tx = self.connection.transaction()?;
        let result = snapshot_resource(&tx, scope, reference, authorizer)?;
        tx.commit()?;
        Ok(result)
    }
}

pub(super) fn snapshot_resource(
    db: &Connection,
    scope: &Scope,
    reference: &str,
    authorizer: &dyn Authorizer,
) -> CwpdbResult<Option<ResourceFacts>> {
    authorizer.authorize(&scope.context, reference, false)?;
    let kind: Option<String> = db.query_row("SELECT kind FROM nodes WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)",[reference],|row|row.get(0)).optional()?;
    if !matches!(kind.as_deref(), Some("message" | "invocation")) {
        return Ok(None);
    }
    let Some(node) = read::get(db, reference)? else {
        return Ok(None);
    };
    if node.schema_ref != crate::schema_ref(node.kind.as_str()) {
        return Err("unsupported resource schema".into());
    }
    let Some((conversation_ref, streaming, stream_record)) = association(db, &node)? else {
        return Ok(None);
    };
    authorizer.authorize(&scope.context, &conversation_ref, false)?;
    let Some(conversation) = conversation_facts(db, scope, &conversation_ref)? else {
        return Ok(None);
    };
    let root = one(db, &conversation_ref, "conversation_timeline_root")?;
    if streaming && root.is_none() {
        return Ok(None);
    }
    if let Some(root) = &root {
        let retained = read::get(db, root)?.ok_or("resource timeline root is unavailable")?;
        if retained.kind != RecordKind::ConversationTimelineConnection
            || retained.schema_ref
                != crate::schema_ref(RecordKind::ConversationTimelineConnection.as_str())
        {
            return Err("resource timeline root has invalid kind or schema".into());
        }
    }
    let edge = if node.kind == RecordKind::Invocation {
        "conversation_timeline_invocation"
    } else {
        "conversation_timeline_message"
    };
    if !visible(db, root.as_deref(), reference, edge)? {
        return Ok(None);
    };
    let resource = if node.kind == RecordKind::Invocation {
        invocation::facts(db, &conversation_ref, &node, root.as_deref())?
    } else {
        let Some(record) = (if streaming {
            Some(stream_record)
        } else {
            message(db, &conversation_ref, reference)?
        }) else {
            return Ok(None);
        };
        let occurred=db.query_row("SELECT json_extract(n.props,'$.record.occurred_at_unix_ms') FROM edges link INDEXED BY edges_in CROSS JOIN nodes n ON n.ref_id=link.source_id AND n.kind='activity' CROSS JOIN edges membership INDEXED BY edges_in ON membership.target_id=n.ref_id AND membership.kind='activity' WHERE link.target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND link.kind='field:event.message_ref' AND membership.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2) AND json_extract(n.props,'$.record.event.kind')='message_recorded' AND json_extract(n.props,'$.record.event.message_ref')=?1 ORDER BY membership.position LIMIT 1",params![reference,conversation_ref],|r|r.get(0)).optional()?.flatten();
        ResourceRecordFacts::Message {
            record,
            streaming,
            occurred_at_unix_ms: if streaming { None } else { occurred },
        }
    };
    let metadata = runtime_metadata(db, scope)?;
    let result = ResourceFacts {
        runtime_root_ref: scope.runtime_root_ref.clone(),
        conversation_ref,
        commit_revision: metadata["commit_revision"]
            .as_i64()
            .ok_or("missing commit revision")?
            .to_string(),
        conversation,
        resource,
    };
    if serde_json::to_vec(&result)?.len() > MAX_WIRE_BYTES {
        return Err("resource facts exceed wire limit".into());
    }
    authorizer.authorize(&scope.context, reference, false)?;
    Ok(Some(result))
}
