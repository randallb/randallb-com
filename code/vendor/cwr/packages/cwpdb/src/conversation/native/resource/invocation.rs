use super::*;
fn references(db: &Connection, sql: &str, args: &[&str]) -> CwpdbResult<Vec<String>> {
    let mut statement = db.prepare_cached(sql)?;
    let result = statement
        .query_map(rusqlite::params_from_iter(args), |row| row.get(0))?
        .collect::<Result<Vec<String>, _>>()?;
    if result.len() > LIMIT as usize {
        return Err("resource relationship limit exceeded".into());
    }
    Ok(result)
}
fn reply(
    db: &Connection,
    conversation: &str,
    node: &Record,
    source: Option<&str>,
) -> CwpdbResult<Option<Value>> {
    let record = &node.properties["record"];
    if record["parent_invocation_ref"].is_string()
        && matches!(
            record["state"].as_str(),
            Some("completed" | "blocked" | "failed" | "stopped")
        )
    {
        if let Some(reference) = record["reply_message_ref"].as_str() {
            return message(db, conversation, reference);
        }
        let candidates=references(db,"SELECT reference.canonical_ref FROM edges membership INDEXED BY edges_out JOIN nodes n ON n.ref_id=membership.target_id AND n.kind='message' JOIN ref_bindings reference ON reference.ref_id=n.ref_id WHERE membership.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND membership.kind='message' AND json_extract(n.props,'$.record.reply_to_message_ref')=?2 AND json_extract(n.props,'$.record.author_actor_ref')=?3 AND json_type(n.props,'$.record.progress') IS NULL AND (json_extract(n.props,'$.record.action.kind')='result' OR ?4='respond_to--message') LIMIT 2",&[conversation,source.unwrap_or(""),record["target_actor_ref"].as_str().unwrap_or(""),record["action_name"].as_str().unwrap_or("")])?;
        let shared:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM edges target INDEXED BY edges_in JOIN nodes other ON other.ref_id=target.source_id AND other.kind='invocation' WHERE target.target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?3) AND target.kind='field:target_actor_ref' AND other.ref_id!=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND json_extract(other.props,'$.record.conversation_ref')=?2 AND json_extract(other.props,'$.record.target_actor_ref')=?3 AND (json_extract(other.props,'$.record.source_message_ref')=?4 OR EXISTS(SELECT 1 FROM edges execution JOIN edges activity ON activity.source_id=execution.target_id AND activity.kind='activity' JOIN nodes start ON start.ref_id=activity.target_id WHERE execution.source_id=other.ref_id AND execution.kind='execution' AND json_extract(start.props,'$.record.event.kind')='turn_started' AND json_extract(start.props,'$.record.event.source_message_ref')=?4)))",params![node.reference,conversation,record["target_actor_ref"].as_str(),source],|row|row.get(0))?;
        if candidates.len() == 1 && !shared {
            return message(db, conversation, &candidates[0]);
        }
    }
    let reference:Option<String>=db.query_row("SELECT reference.canonical_ref FROM edges membership INDEXED BY edges_out JOIN nodes message ON message.ref_id=membership.target_id AND message.kind='message' JOIN ref_bindings reference ON reference.ref_id=message.ref_id WHERE membership.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND membership.kind='message' AND (json_extract(message.props,'$.record.progress.invocation_ref')=?2 OR reference.canonical_ref IN (SELECT json_extract(admission.props,'$.record.reply_message_ref') FROM edges lookup INDEXED BY edges_in JOIN nodes admission ON admission.ref_id=lookup.source_id AND admission.kind='admission' WHERE lookup.target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2) AND lookup.kind='field:invocation_ref')) ORDER BY membership.position DESC LIMIT 1",params![conversation,node.reference],|row|row.get(0)).optional()?;
    reference
        .map(|reference| message(db, conversation, &reference))
        .transpose()
        .map(Option::flatten)
}
fn execution(
    db: &Connection,
    conversation: &str,
    node: &Record,
    execution: &str,
) -> CwpdbResult<Option<ResourceExecutionFacts>> {
    let record = &node.properties["record"];
    if matches!(
        record["state"].as_str(),
        Some("completed" | "blocked" | "failed" | "stopped")
    ) || (record["state"] == "stopping"
        && !record["stop_execution_pending"].as_bool().unwrap_or(false))
    {
        return Ok(None);
    }
    let generation = record["attempt_count"].as_u64().unwrap_or(0);
    if generation == 0 {
        return Ok(None);
    }
    let lower = record["attempt_started_after_activity_sequence"]
        .as_i64()
        .unwrap_or(if generation == 1 { -1 } else { i64::MAX });
    let latest:Option<i64>=db.query_row("SELECT MAX(json_extract(n.props,'$.record.occurred_at_unix_ms')) FROM edges e INDEXED BY edges_out JOIN nodes n ON n.ref_id=e.target_id AND n.kind='activity' WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind='activity' AND json_extract(n.props,'$.record.sequence')>?2",params![execution,lower],|row|row.get(0))?;
    let phase:Option<String>=db.query_row("SELECT json_object('state',json_extract(n.props,'$.record.event.activity.state'),'phase',json_extract(n.props,'$.record.event.activity.phase')) FROM edges e INDEXED BY edges_out JOIN nodes n ON n.ref_id=e.target_id AND n.kind='activity' WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind='activity' AND json_extract(n.props,'$.record.sequence')>?2 AND json_extract(n.props,'$.record.event.kind')='execution_activity' ORDER BY e.position DESC LIMIT 1",params![execution,lower],|row|row.get(0)).optional()?;
    let progress:Option<String>=db.query_row("SELECT json_extract(n.props,'$.record.event.summary') FROM edges e INDEXED BY edges_out JOIN nodes n ON n.ref_id=e.target_id AND n.kind='activity' WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind='activity' AND json_extract(n.props,'$.record.sequence')>?2 AND json_extract(n.props,'$.record.event.kind')='progress' AND NOT EXISTS(SELECT 1 FROM edges membership JOIN nodes message ON message.ref_id=membership.target_id AND message.kind='message' WHERE membership.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?3) AND membership.kind='message' AND json_extract(message.props,'$.record.progress.source_activity_ref')=(SELECT canonical_ref FROM ref_bindings WHERE ref_id=n.ref_id)) ORDER BY e.position DESC LIMIT 1",params![execution,lower,conversation],|row|row.get(0)).optional()?.flatten();
    Ok(Some(ResourceExecutionFacts {
        generation,
        attempt_active: record["attempt_active"].as_bool().unwrap_or(false),
        started_at_unix_ms: record["attempt_started_at_unix_ms"].as_u64(),
        latest_activity_at_unix_ms: latest.map(u64::try_from).transpose()?,
        latest_phase_event: phase
            .map(|value| serde_json::from_str(&value))
            .transpose()?,
        unpublished_progress: progress,
    }))
}
pub(in crate::conversation::native) fn facts(
    db: &Connection,
    conversation: &str,
    node: &Record,
    root: Option<&str>,
) -> CwpdbResult<ResourceRecordFacts> {
    let child_invocation_refs=references(db,"SELECT reference.canonical_ref FROM edges e INDEXED BY edges_in JOIN nodes n ON n.ref_id=e.source_id AND n.kind='invocation' JOIN ref_bindings reference ON reference.ref_id=n.ref_id WHERE e.target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2) AND e.kind='field:parent_invocation_ref' AND json_extract(n.props,'$.record.conversation_ref')=?1 ORDER BY reference.canonical_ref LIMIT 50001",&[conversation,&node.reference])?;
    let wait_records=references(db,"SELECT json_object('waiter_invocation_ref',json_extract(n.props,'$.record.waiter_invocation_ref'),'awaited_invocation_ref',json_extract(n.props,'$.record.awaited_invocation_ref'),'call_id',json_extract(n.props,'$.record.call_id'),'state',json_extract(n.props,'$.record.state'),'resume_after_activity_sequence',json_extract(n.props,'$.record.resume_after_activity_sequence')) FROM edges e INDEXED BY edges_in JOIN nodes n ON n.ref_id=e.source_id AND n.kind='wait' WHERE e.target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind='field:waiter_invocation_ref' ORDER BY json_extract(n.props,'$.record.call_id') LIMIT 50001",&[&node.reference])?.into_iter().map(|value|Ok(serde_json::from_str(&value)?)).collect::<CwpdbResult<Vec<Value>>>()?;
    let mut record = super::super::super::resource::invocation_fields(&node.properties["record"]);
    let execution_ref = one(db, &node.reference, "execution")?;
    let execution = if let Some(execution) = execution_ref {
        record["source_message_ref"]=db.query_row("SELECT json_extract(n.props,'$.record.event.source_message_ref') FROM edges e INDEXED BY edges_out JOIN nodes n ON n.ref_id=e.target_id AND n.kind='activity' WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind='activity' AND json_extract(n.props,'$.record.event.kind')='turn_started' ORDER BY e.position LIMIT 1",[&execution],|row|row.get::<_,Option<String>>(0)).optional()?.flatten().map(Value::String).unwrap_or(Value::Null);
        self::execution(db, conversation, node, &execution)?
    } else {
        None
    };
    let latest_reply = reply(
        db,
        conversation,
        node,
        record["source_message_ref"].as_str(),
    )?;
    let latest_reply = match latest_reply {
        Some(reply)
            if visible(
                db,
                root,
                reply["message_ref"]
                    .as_str()
                    .ok_or("reply message ref required")?,
                "conversation_timeline_message",
            )? =>
        {
            Some(reply)
        }
        _ => None,
    };
    Ok(ResourceRecordFacts::Invocation {
        record,
        child_invocation_refs,
        wait_records,
        latest_reply,
        execution,
    })
}
