use super::*;
use rusqlite::{params, OptionalExtension};
impl NativeConversationStore {
    pub(super) fn history_read_facts(
        &mut self,
        request: &HistoryReadRequest,
        control: &ReadControl,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<Option<HistoryReadFacts>> {
        self.check_scope(&request.scope)?;
        valid_ref(&request.conversation_ref)?;
        validate(request)?;
        let mut bounded = control.clone();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis();
        let remaining = u64::try_from(i128::from(request.deadline_unix_ms) - i128::try_from(now)?)
            .map_err(|_| "conversation read operation exceeded its deadline")?;
        bounded.deadline = bounded
            .deadline
            .min(std::time::Instant::now() + std::time::Duration::from_millis(remaining));
        let control = &bounded;
        control.check()?;
        let monitor = control.clone();
        let enabled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let monitoring = enabled.clone();
        self.connection
            .busy_timeout(std::time::Duration::from_millis(5))?;
        self.connection.progress_handler(
            if monitor.vm_steps.is_some() { 1 } else { 1000 },
            Some(move || {
                if let Some(counter) = &monitor.vm_steps {
                    counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                monitoring.load(std::sync::atomic::Ordering::Acquire) && monitor.check().is_err()
            }),
        )?;
        let result = (|| loop {
            control.check()?;
            enabled.store(true, std::sync::atomic::Ordering::Release);
            let result: CwpdbResult<Option<HistoryReadFacts>> = (|| {
                let tx = self.connection.transaction()?;
                let selected = (|| {
                    authorizer.authorize(
                        &request.scope.context,
                        &request.conversation_ref,
                        false,
                    )?;
                    let Some(header) = resource::conversation_facts(
                        &tx,
                        &request.scope,
                        &request.conversation_ref,
                    )?
                    else {
                        return Ok(None);
                    };
                    if (!request.include_execution && header.parent_ref.is_some())
                        || request
                            .participant_actor_ref
                            .as_ref()
                            .is_some_and(|actor| !header.participant_actor_refs.contains(actor))
                    {
                        return Err(
                            "conversation is unavailable to the requesting participant".into()
                        );
                    }
                    let result = page(&tx, request, header, control)?;
                    authorizer.authorize(
                        &request.scope.context,
                        &request.conversation_ref,
                        false,
                    )?;
                    control.check()?;
                    Ok(Some(result))
                })();
                enabled.store(false, std::sync::atomic::Ordering::Release);
                match selected {
                    Ok(value) => {
                        tx.commit()?;
                        Ok(value)
                    }
                    Err(error) => {
                        tx.rollback()?;
                        Err(error)
                    }
                }
            })();
            if result.as_ref().err().and_then(|e|e.downcast_ref::<rusqlite::Error>()).is_some_and(|e| matches!(e,rusqlite::Error::SqliteFailure(code,_) if matches!(code.code,rusqlite::ErrorCode::DatabaseBusy|rusqlite::ErrorCode::DatabaseLocked))) { std::thread::sleep(std::time::Duration::from_millis(1));continue; }
            return result;
        })();
        self.connection.progress_handler(0, None::<fn() -> bool>)?;
        self.connection
            .busy_timeout(std::time::Duration::from_secs(5))?;
        control.check()?;
        result
    }
}
pub(super) fn validate(request: &HistoryReadRequest) -> CwpdbResult<()> {
    if request.max_bytes == 0 || request.max_bytes > MAX_WIRE_BYTES {
        return Err("invalid history response byte budget".into());
    }
    match &request.selection {
        HistorySelection::Page {
            after,
            before,
            first,
            last,
        }
        | HistorySelection::ArchivePage {
            after,
            before,
            first,
            last,
        }
        | HistorySelection::PagePositions {
            after,
            before,
            first,
            last,
        }
        | HistorySelection::ContentPositions {
            after,
            before,
            first,
            last,
        } => pages::content::validate(&ContentPageRequest {
            scope: request.scope.clone(),
            conversation_ref: request.conversation_ref.clone(),
            after: after.clone(),
            before: before.clone(),
            first: *first,
            last: *last,
            max_bytes: request.max_bytes,
        })?,
        HistorySelection::Replies { first, .. }
        | HistorySelection::Scan { first, .. }
        | HistorySelection::ScanPositions { first, .. }
            if *first == 0 || *first > 500 =>
        {
            return Err("invalid history page size".into())
        }
        _ => {}
    }
    Ok(())
}
pub(super) fn page(
    db: &Connection,
    request: &HistoryReadRequest,
    header: ResourceConversationFacts,
    control: &ReadControl,
) -> CwpdbResult<HistoryReadFacts> {
    let context = pages::context(db, &request.scope, &request.conversation_ref)?
        .ok_or("conversation disappeared from snapshot")?;
    if request
        .expected_commit_revision
        .as_ref()
        .is_some_and(|r| r != &context.commit_revision)
    {
        return Err("conversation revision_conflict".into());
    }
    let canonical = context.root.is_some();
    let root = context.root.as_deref().unwrap_or("");
    if canonical {
        pages::visibility::validate_walk(db, root)?;
    }
    // History includes commentary/progress; content pages intentionally omit it.
    let relation = if matches!(request.selection, HistorySelection::ContentPositions { .. }) {
        pages::visibility::message_relation(canonical)
    } else {
        pages::visibility::message_relation(canonical)
            .replace("AND json_type(node.props,'$.record.progress') IS NULL ", "")
    };
    let mut facts = HistoryReadFacts {
        runtime_root_ref: request.scope.runtime_root_ref.clone(),
        conversation_ref: request.conversation_ref.clone(),
        commit_revision: context.commit_revision,
        conversation: header,
        canonical,
        revision: context.revision,
        items: Vec::new(),
        has_next_page: false,
        has_previous_page: false,
        total_count: 0,
        reply_message_refs: Vec::new(),
    };
    let keys = select_keys(db, request, &relation, root, &mut facts)?;
    if matches!(
        request.selection,
        HistorySelection::PagePositions { .. }
            | HistorySelection::ContentPositions { .. }
            | HistorySelection::ScanPositions { .. }
    ) {
        facts.items = keys
            .into_iter()
            .map(|(position, message_ref)| HistoryItemFacts {
                position,
                message_ref,
                record: None,
                payload_bytes: 0,
                origin_invocation_ref: None,
            })
            .collect();
        control.check()?;
        if serde_json::to_vec(&facts)?.len() > request.max_bytes {
            return Err("history response exceeds byte budget".into());
        }
        return Ok(facts);
    }
    let mut used = serde_json::to_vec(&facts)?.len();
    for (position, reference) in keys {
        control.check()?;
        let (schema,bytes): (String,i64)=db.query_row(&format!("{relation} SELECT schema_ref,length(CAST(record AS BLOB)) FROM messages WHERE message_ref=?3"),params![request.conversation_ref,root,reference],|row|Ok((row.get(0)?,row.get(1)?)))?;
        if schema != crate::schema_ref("message") {
            return Err("unsupported message schema".into());
        }
        let payload_bytes = usize::try_from(bytes)?;
        let limit = if matches!(
            request.selection,
            HistorySelection::ArchivePage { .. } | HistorySelection::ArchiveMessage { .. }
        ) {
            MAX_RECORD_BYTES
        } else {
            MAX_HISTORY_MESSAGE_BYTES
        };
        let record = if payload_bytes > limit {
            None
        } else {
            Some(
                resource::message(db, &request.conversation_ref, &reference)?
                    .ok_or("selected history message disappeared from snapshot")?,
            )
        };
        if record
            .as_ref()
            .is_some_and(|r| r["message_ref"] != reference)
        {
            return Err("selected history message identity mismatch".into());
        }
        let origin = origin(db, &request.conversation_ref, &reference, record.as_ref())?;
        let item = HistoryItemFacts {
            position,
            message_ref: reference,
            record,
            payload_bytes,
            origin_invocation_ref: origin,
        };
        used += serde_json::to_vec(&item)?.len() + usize::from(!facts.items.is_empty());
        if used > request.max_bytes {
            return Err("history response exceeds byte budget".into());
        }
        facts.items.push(item);
    }
    control.check()?;
    if serde_json::to_vec(&facts)?.len() > request.max_bytes {
        return Err("history response exceeds byte budget".into());
    }
    Ok(facts)
}
fn origin(
    db: &Connection,
    conversation: &str,
    message: &str,
    record: Option<&Value>,
) -> CwpdbResult<Option<String>> {
    if let Some(progress) = record.and_then(|r| r["progress"]["invocation_ref"].as_str()) {
        return Ok(Some(progress.into()));
    }
    let mut statement=db.prepare_cached("SELECT DISTINCT CASE WHEN n.kind='invocation' THEN r.canonical_ref ELSE json_extract(n.props,'$.record.invocation_ref') END AS origin FROM edges e JOIN nodes n ON n.ref_id=e.source_id JOIN ref_bindings r ON r.ref_id=n.ref_id WHERE e.target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2) AND e.kind='field:reply_message_ref' AND n.kind IN ('invocation','admission') AND json_extract(n.props,'$.record.conversation_ref')=?1 AND json_extract(n.props,'$.record.reply_message_ref')=?2 AND origin IS NOT NULL LIMIT 2")?;
    let origins = statement
        .query_map(params![conversation, message], |row| {
            row.get::<_, String>(0)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(if origins.len() == 1 {
        origins.into_iter().next()
    } else {
        None
    })
}

fn select_keys(
    db: &Connection,
    request: &HistoryReadRequest,
    relation: &str,
    root: &str,
    facts: &mut HistoryReadFacts,
) -> CwpdbResult<Vec<(i64, String)>> {
    let canonical = facts.canonical;
    Ok(match &request.selection {
        HistorySelection::Page {
            after,
            before,
            first,
            last,
        }
        | HistorySelection::ArchivePage {
            after,
            before,
            first,
            last,
        }
        | HistorySelection::PagePositions {
            after,
            before,
            first,
            last,
        }
        | HistorySelection::ContentPositions {
            after,
            before,
            first,
            last,
        } => {
            if canonical {
                for c in [after, before].into_iter().flatten() {
                    if c.revision.as_deref() != Some(&facts.revision) {
                        return Err(
                            "conversation record cursor belongs to an earlier revision".into()
                        );
                    }
                }
            }
            let (keys, next, previous, total) = pages::content::window(
                db,
                relation,
                &ContentPageRequest {
                    scope: request.scope.clone(),
                    conversation_ref: request.conversation_ref.clone(),
                    after: after.clone(),
                    before: before.clone(),
                    first: *first,
                    last: *last,
                    max_bytes: request.max_bytes,
                },
                root,
            )?;
            facts.has_next_page = next;
            facts.has_previous_page = previous;
            facts.total_count = total;
            keys.into_iter()
                .map(|k| (k.position, k.reference))
                .collect()
        }
        HistorySelection::Message { message_ref }
        | HistorySelection::RetainedMessage { message_ref }
        | HistorySelection::ArchiveMessage { message_ref }
        | HistorySelection::Replies { message_ref, .. } => {
            if !canonical
                && matches!(
                    request.selection,
                    HistorySelection::Message { .. } | HistorySelection::Replies { .. }
                )
            {
                let published:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM edges e WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2) AND e.kind='timeline_item' AND json_extract(e.props,'$.item_kind')='message')",params![request.conversation_ref,message_ref],|row|row.get(0))?;
                if !published {
                    return Err("message is unavailable in the requested conversation".into());
                }
            }
            let key = db
                .query_row(
                    &format!(
                        "{relation} SELECT position,message_ref FROM messages WHERE message_ref=?3"
                    ),
                    params![request.conversation_ref, root, message_ref],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()?;
            if key.is_none() {
                return Err("message is unavailable in the requested conversation".into());
            }
            if let HistorySelection::Replies {
                after_message_ref,
                after_revision,
                first,
                ..
            } = &request.selection
            {
                if canonical
                    && after_message_ref.is_some()
                    && after_revision.as_deref() != Some(&facts.revision)
                {
                    return Err(
                        "conversation cursor belongs to an earlier canonical timeline revision"
                            .into(),
                    );
                }
                let after = if let Some(reference) = after_message_ref {
                    db.query_row(&format!("{relation} SELECT position FROM messages WHERE message_ref=?3 AND json_extract(record,'$.reply_to_message_ref')=?4"),params![request.conversation_ref,root,reference,message_ref],|row|row.get::<_,i64>(0)).optional()?.ok_or("reply cursor is not a reply to the requested message")?
                } else {
                    i64::MIN
                };
                let mut statement=db.prepare(&format!("{relation} SELECT message_ref FROM messages WHERE json_extract(record,'$.reply_to_message_ref')=?3 AND position>?4 ORDER BY position LIMIT ?5"))?;
                let mut refs = statement
                    .query_map(
                        params![
                            request.conversation_ref,
                            root,
                            message_ref,
                            after,
                            i64::try_from(first + 1)?
                        ],
                        |row| row.get::<_, String>(0),
                    )?
                    .collect::<Result<Vec<_>, _>>()?;
                facts.has_next_page = refs.len() > *first;
                refs.truncate(*first);
                facts.reply_message_refs = refs;
            }
            key.into_iter().collect()
        }
        HistorySelection::Scan {
            after_position,
            after_revision,
            first,
        }
        | HistorySelection::ScanPositions {
            after_position,
            after_revision,
            first,
        } => {
            if canonical
                && after_position.is_some()
                && after_revision.as_deref() != Some(&facts.revision)
            {
                return Err(
                    "conversation cursor belongs to an earlier canonical timeline revision".into(),
                );
            }
            let mut statement=db.prepare(&format!("{relation} SELECT position,message_ref FROM messages WHERE position>?3 ORDER BY position LIMIT ?4"))?;
            let mut keys = statement
                .query_map(
                    params![
                        request.conversation_ref,
                        root,
                        after_position.unwrap_or(i64::MIN),
                        i64::try_from(first + 1)?
                    ],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
                )?
                .collect::<Result<Vec<_>, _>>()?;
            facts.has_next_page = keys.len() > *first;
            keys.truncate(*first);
            keys
        }
    })
}
