use super::*;
use rusqlite::OptionalExtension;
pub(super) fn put(
    tx: &Transaction<'_>,
    scope: &Scope,
    input: &WriteRecord,
    next_revision: i64,
) -> CwpdbResult<bool> {
    valid_ref(&input.reference)?;
    if input.kind == RecordKind::Runtime {
        return Err("runtime metadata uses its named update fields".into());
    }
    if !input.properties.is_object() {
        return Err("CWR record properties must be an object".into());
    }
    // Concretizing a retained actor binding must recheck its unchanged role.
    // All typed CWR RecordKind variants are internal records, not actor nodes.
    let (scanned, actor_binding): (i64, bool) = tx.query_row(
        "WITH incident(source_id) AS (
           SELECT source_id FROM edges INDEXED BY edges_in
           WHERE target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)
             AND kind IN ('participant','participant_version') LIMIT 50001
         ) SELECT COUNT(*),COALESCE(MAX(EXISTS(SELECT 1 FROM edges own INDEXED BY edges_out
           WHERE own.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2)
             AND own.kind='conversation' AND own.target_id=incident.source_id)),0) FROM incident",
        params![input.reference, root_ref(scope)],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if scanned > 50000 {
        return Err("external actor binding work limit exceeded".into());
    }
    if actor_binding {
        return Err("CWR record conflicts with retained external actor binding".into());
    }
    let previous = read::get(tx, &input.reference)?;
    if previous.as_ref().is_some_and(|previous| {
        previous.kind != input.kind || previous.schema_ref != crate::schema_ref(input.kind.as_str())
    }) {
        return Err("CWR record schema or kind conflict".into());
    }
    if matches!(
        input.kind,
        RecordKind::Conversation
            | RecordKind::ConversationTimelineConnection
            | RecordKind::ConversationTimelineItem
    ) {
        let owner = if input.kind == RecordKind::Conversation {
            input.reference.as_str()
        } else {
            input
                .owner_ref
                .as_deref()
                .ok_or("timeline record requires conversation owner")?
        };
        valid_ref(owner)?;
        crate::claim_identity(
            tx,
            &input.reference,
            &format!("node:{}", input.kind.as_str()),
            owner,
        )?;
    }
    let mut patch = input.properties.clone();
    if input.kind == RecordKind::Admission {
        let record = patch
            .get_mut("record")
            .and_then(Value::as_object_mut)
            .ok_or("admission record missing")?;
        let integration = record
            .get("integration_ref")
            .and_then(Value::as_str)
            .ok_or("admission integration missing")?;
        let provider = record
            .get("provider_event_ref")
            .and_then(Value::as_str)
            .ok_or("admission event missing")?;
        if input.reference != crate::derived_ref("admission", &[integration, provider]) {
            return Err("admission identity mismatch".into());
        }
        if let Some(old) = &previous {
            for field in [
                "integration_ref",
                "provider_event_ref",
                "conversation_ref",
                "source_message_ref",
            ] {
                if old.properties["record"][field]
                    != record.get(field).cloned().unwrap_or(Value::Null)
                {
                    return Err("immutable admission identity changed".into());
                }
            }
        }
        if let Some(fingerprint) = record.remove("source_input_fingerprint") {
            patch["source_input_fingerprint"] = fingerprint;
        }
    }
    if input.kind == RecordKind::UpstreamDeclaration
        && previous
            .as_ref()
            .is_some_and(|previous| previous.properties["record"] != patch["record"])
    {
        return Err("upstream declaration changed without a new coworker version".into());
    }
    let mut merged = previous
        .as_ref()
        .map(|record| record.properties.clone())
        .unwrap_or_else(|| json!({}));
    if patch.get("record").is_some() {
        merged.as_object_mut().unwrap().remove("record");
    }
    merge(&mut merged, &patch);
    // These payloads are complete opaque CWR records, not merge patches.
    // RFC 7396 would delete explicit null fields (including required input:null)
    // and change the record the protocol layer actually validated.
    for field in ["record", "streaming"] {
        if let Some(payload) = patch.get(field) {
            merged[field] = payload.clone();
        }
    }
    if input.kind == RecordKind::Conversation {
        for forbidden in [
            "creation_order",
            "commit_revision",
            "timeline_revision",
            "completed_work_refs",
        ] {
            if input.properties.get(forbidden).is_some() {
                return Err(
                    "caller cannot choose storage-owned conversation clocks or summaries".into(),
                );
            }
        }
        if previous.is_none() {
            let creation:i64=tx.query_row("UPDATE nodes SET props=jsonb_set(props,'$.next_creation_order',COALESCE(json_extract(props,'$.next_creation_order'),0)+1) WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) RETURNING json_extract(props,'$.next_creation_order')-1",[root_ref(scope)],|row|row.get(0))?;
            merged["creation_order"] = json!(creation);
        }
    }
    if input.kind == RecordKind::Message && patch.get("record").is_some() {
        merged.as_object_mut().unwrap().remove("streaming");
    }
    if input.kind == RecordKind::File
        && previous
            .as_ref()
            .is_some_and(|record| record.properties["record"] != merged["record"])
    {
        return Err("retained files are immutable".into());
    }
    if previous
        .as_ref()
        .is_some_and(|record| record.properties == merged)
    {
        return Ok(false);
    }
    if input.kind == RecordKind::Conversation {
        merged["commit_revision"] = json!(next_revision);
    }
    if serde_json::to_vec(&merged)?.len() > MAX_RECORD_BYTES {
        return Err("CWR record exceeds24MiB wire budget".into());
    }
    // Full resulting outer properties are selected here; domain `record` replaces
    // as a whole and extension properties survive unless explicitly removed.
    let id = crate::put_node_with_schema(
        tx,
        &input.reference,
        input.kind.as_str(),
        &crate::schema_ref(input.kind.as_str()),
        &json!({}),
    )?;
    tx.execute(
        "UPDATE nodes SET props=jsonb(?2) WHERE ref_id=?1",
        params![id, serde_json::to_string(&merged)?],
    )?;
    if merged.get("record").is_some() {
        references(tx, &input.reference, &merged["record"])?;
    }
    if input.kind == RecordKind::Activity {
        commentary(tx, &input.reference, &merged["record"])?;
    }
    if input.kind == RecordKind::ConversationTimelineItem {
        let owner = input.owner_ref.as_deref().ok_or("timeline owner missing")?;
        let publication = merged["publication_ref"]
            .as_str()
            .ok_or("timeline publication missing")?;
        crate::claim_identity(tx, publication, "conversation_timeline_publication", owner)?;
    }
    if input.kind == RecordKind::Invocation {
        completion(tx, scope, &input.reference, &merged["record"])?;
    }
    if input.kind == RecordKind::Conversation {
        advance_summary(tx, scope)?;
    }
    Ok(true)
}
fn merge(target: &mut Value, patch: &Value) {
    let Some(patch) = patch.as_object() else {
        *target = patch.clone();
        return;
    };
    if !target.is_object() {
        *target = json!({});
    }
    let target = target.as_object_mut().unwrap();
    for (key, value) in patch {
        if value.is_null() {
            target.remove(key);
        } else {
            merge(target.entry(key).or_insert(Value::Null), value);
        }
    }
}
pub(crate) fn references(tx: &Transaction<'_>, reference: &str, record: &Value) -> CwpdbResult<()> {
    let mut wanted = BTreeMap::<String, Vec<String>>::new();
    for (prefix, value) in [
        ("", record),
        ("event.", &record["event"]),
        ("progress.", &record["progress"]),
    ] {
        if let Some(object) = value.as_object() {
            for (name, value) in object {
                let references = if name.ends_with("_ref") {
                    value.as_str().into_iter().collect::<Vec<_>>()
                } else if name.ends_with("_refs") {
                    value
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .collect()
                } else {
                    continue;
                };
                wanted.insert(
                    format!("field:{prefix}{name}"),
                    references
                        .into_iter()
                        .filter(|value| value.starts_with("cwp://") && *value != reference)
                        .map(str::to_owned)
                        .collect(),
                );
            }
        }
    }
    let previous=tx.prepare_cached("SELECT DISTINCT kind FROM edges WHERE source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND kind GLOB 'field:*'")?.query_map([reference],|row|row.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
    for kind in previous {
        wanted.entry(kind).or_default();
    }
    for (kind, targets) in wanted {
        crate::replace_targets(tx, reference, &kind, &targets)?;
    }
    Ok(())
}
fn commentary(tx: &Transaction<'_>, reference: &str, record: &Value) -> CwpdbResult<()> {
    let event = &record["event"];
    if event["kind"] == "commentary_progress" {
        let provider = event["provider_integration_ref"]
            .as_str()
            .ok_or("commentary provider missing")?;
        let turn = event["response_or_turn_id"]
            .as_str()
            .ok_or("commentary turn missing")?;
        let item = event["item_id"].as_str().ok_or("commentary item missing")?;
        let key = crate::derived_ref("commentary", &[provider, turn, item]);
        let other:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM edges WHERE source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND kind='commentary' AND target_id!=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2))",params![key,reference],|row|row.get(0))?;
        if other {
            return Err("provider commentary item already retained".into());
        }
        crate::put_edge(tx, &key, "commentary", reference, 0, &json!({}))?;
    }
    Ok(())
}
pub(crate) fn completion(
    tx: &Transaction<'_>,
    scope: &Scope,
    reference: &str,
    record: &Value,
) -> CwpdbResult<()> {
    if let (Some(conversation), Some(publications)) = (
        record["conversation_ref"].as_str(),
        record["timeline_publications"].as_object(),
    ) {
        for publication in publications.keys() {
            crate::claim_identity(
                tx,
                publication,
                "conversation_timeline_publication",
                conversation,
            )?;
        }
    }
    if record["state"] == "completed"
        && record["parent_invocation_ref"].is_null()
        && record["forked_from_invocation_ref"].is_null()
    {
        let previous:Option<i64>=tx.query_row("SELECT json_extract(props,'$.completion_order') FROM nodes WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)",[reference],|row|row.get(0)).optional()?.flatten();
        let order = if let Some(previous) = previous {
            previous
        } else {
            tx.query_row("UPDATE nodes SET props=jsonb_set(props,'$.completion_order',COALESCE(json_extract(props,'$.completion_order'),0)+1) WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) RETURNING json_extract(props,'$.completion_order')",[root_ref(scope)],|row|row.get::<_,i64>(0))?
        };
        tx.execute("UPDATE nodes SET props=jsonb_set(props,'$.completion_order',?2) WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)",params![reference,order])?;
        if let Some(conversation) = record["conversation_ref"].as_str() {
            crate::put_edge(
                tx,
                conversation,
                "completed_invocation",
                reference,
                order,
                &json!({}),
            )?;
        }
    } else {
        tx.execute("DELETE FROM edges WHERE kind='completed_invocation' AND target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)",[reference])?;
    }
    Ok(())
}
pub(super) fn advance_summary(tx: &Transaction<'_>, scope: &Scope) -> CwpdbResult<()> {
    tx.execute("UPDATE nodes SET props=jsonb_set(props,'$.summary_membership_revision',COALESCE(json_extract(props,'$.summary_membership_revision'),0)+1) WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)",[root_ref(scope)])?;
    Ok(())
}

/// Completion order and compact root attention facts are storage-owned. Seed
/// imported live completion activities once, after their canonical memberships
/// exist; activity before a fork boundary is historical rather than new work.
pub(crate) fn refresh_completions(
    tx: &Transaction<'_>,
    scope: &Scope,
    conversation: &str,
) -> CwpdbResult<bool> {
    let is_root: bool = tx.query_row("SELECT NOT EXISTS(SELECT 1 FROM edges WHERE source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND kind='parent')",[conversation],|row|row.get(0))?;
    if !is_root {
        return Ok(false);
    }
    let lineage: i64 = tx.query_row("SELECT COALESCE(MAX(e.position),-1) FROM edges e JOIN nodes n ON n.ref_id=e.target_id WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind='transfer_metadata' AND json_extract(n.props,'$.record.event.kind')='conversation_forked_from'",[conversation],|row|row.get(0))?;
    let imported = tx.prepare("SELECT json_extract(n.props,'$.record.event.invocation_ref') FROM edges e JOIN nodes n ON n.ref_id=e.target_id WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind='activity' AND e.position>?2 AND json_extract(n.props,'$.record.event.kind')='work_lifecycle' AND json_extract(n.props,'$.record.event.state')='completed' AND json_extract(n.props,'$.record.event.parent_invocation_ref') IS NULL ORDER BY e.position")?.query_map(params![conversation,lineage],|row|row.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
    for invocation in imported {
        let exists:bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM edges WHERE source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND kind='completed_invocation' AND target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2))",params![conversation,invocation],|row|row.get(0))?;
        if !exists {
            let order:i64 = tx.query_row("UPDATE nodes SET props=jsonb_set(props,'$.completion_order',COALESCE(json_extract(props,'$.completion_order'),0)+1) WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) RETURNING json_extract(props,'$.completion_order')",[root_ref(scope)],|row|row.get(0))?;
            crate::put_edge(
                tx,
                conversation,
                "completed_invocation",
                &invocation,
                order,
                &json!({}),
            )?;
        }
    }
    let mut completed = tx.prepare("SELECT r.canonical_ref FROM edges e JOIN ref_bindings r ON r.ref_id=e.target_id WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind='completed_invocation' ORDER BY e.position DESC LIMIT 500")?.query_map([conversation],|row|row.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
    completed.reverse();
    Ok(tx.execute("UPDATE nodes SET props=jsonb_set(props,'$.completed_work_refs',jsonb(?2)) WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND COALESCE(json_extract(props,'$.completed_work_refs'),json('[]')) IS NOT json(?2)",params![conversation,serde_json::to_string(&completed)?])?>0)
}
