use super::*;
use crate::{sequence_receipt, NativeStore};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::json;
use sha2::{Digest, Sha256};

#[derive(Serialize, Deserialize)]
struct SavedReceipt {
    request: Mutation,
    receipt: Receipt,
}

const KIND: &str = "invocation_execution_journal_v1";
const EVENT: &str = "invocation_execution_event_v1";
const MEMBERSHIP: &str = "invocation_execution_event";
const MAX_PAYLOAD_BYTES: usize = 524_288;

fn key(context: &Context, invocation_ref: &str) -> String {
    crate::derived_ref(KIND, &[&context.workspace_ref, invocation_ref])
}
fn revision(key: &str, counter: i64) -> String {
    hex::encode(Sha256::digest(json!([key, counter]).to_string().as_bytes()))
}
fn read(db: &Connection, key: &str) -> CwpdbResult<Option<Record>> {
    let result: Option<(String, i64)> = db
        .query_row(
            "SELECT json(n.props), n.revision FROM nodes n JOIN ref_bindings r ON r.ref_id=n.ref_id WHERE r.canonical_ref=?1 AND n.kind=?2",
            params![key, KIND], |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?;
    result
        .map(|(props, counter)| {
            let mut record: Record = serde_json::from_str(&props)?;
            record.revision = revision(key, counter);
            Ok(record)
        })
        .transpose()
}
fn check_ref(reference: &str) -> CwpdbResult<()> {
    if !reference.starts_with("cwp://") || reference.len() > 4096 || reference.contains('#') {
        return Err("invalid canonical storage reference".into());
    }
    Ok(())
}
fn check_context(context: &Context, invocation_ref: &str) -> CwpdbResult<()> {
    check_ref(&context.workspace_ref)?;
    check_ref(&context.caller_ref)?;
    check_ref(invocation_ref)
}
fn receipt(record: &Record) -> Receipt {
    Receipt {
        invocation_ref: record.invocation_ref.clone(),
        revision: record.revision.clone(),
        next_sequence: record.next_sequence,
        terminal: record.terminal,
    }
}

impl InvocationStorage for NativeStore {
    fn mutate(
        &mut self,
        request: &Mutation,
        authorizer: &dyn Authorizer,
        now_unix_ms: u64,
    ) -> CwpdbResult<Outcome> {
        self.graph_transaction(|tx| mutate_in_transaction(tx, request, authorizer, now_unix_ms))
    }

    fn receipt(
        &mut self,
        context: &Context,
        invocation_ref: &str,
        request_id: &str,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<ReceiptLookup> {
        check_context(context, invocation_ref)?;
        let scope = crate::derived_ref(
            "declared_mutation_scope",
            &[&context.workspace_ref, &context.caller_ref],
        );
        self.graph_read(|db| {
            authorizer.authorize(context, invocation_ref, false)?;
            let result: CwpdbResult<ReceiptLookup> =
                match sequence_receipt::retained_result(db, &scope, request_id)? {
                    sequence_receipt::Replay::NotCommitted => Ok(ReceiptLookup::NotCommitted),
                    sequence_receipt::Replay::CommittedResultExpired => {
                        Ok(ReceiptLookup::CommittedResultExpired)
                    }
                    sequence_receipt::Replay::Retained(value) => {
                        let saved: SavedReceipt = serde_json::from_value(value)?;
                        if saved.request.invocation_ref != invocation_ref {
                            return Err("mutation request conflict".into());
                        }
                        Ok(ReceiptLookup::Retained {
                            request: Box::new(saved.request),
                            receipt: saved.receipt,
                        })
                    }
                };
            let result = result?;
            authorizer.authorize(context, invocation_ref, false)?;
            Ok(result)
        })
    }

    fn invocation(
        &mut self,
        context: &Context,
        invocation_ref: &str,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<Option<Record>> {
        check_context(context, invocation_ref)?;
        self.graph_read(|db| {
            authorizer.authorize(context, invocation_ref, false)?;
            read(db, &key(context, invocation_ref))
        })
    }

    fn events(
        &mut self,
        context: &Context,
        invocation_ref: &str,
        after: Option<u64>,
        limit: usize,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<EventPage> {
        check_context(context, invocation_ref)?;
        if limit == 0 || limit > 1000 || after.is_some_and(|value| value > i64::MAX as u64) {
            return Err("invocation page requires limit 1..1000 and a valid sequence".into());
        }
        let key = key(context, invocation_ref);
        self.graph_read(|db| {
            authorizer.authorize(context, invocation_ref, false)?;
            if read(db, &key)?.is_none() { return Err("invocation not found".into()); }
            let mut statement = db.prepare("SELECT json(n.props) FROM edges e JOIN ref_bindings r ON r.ref_id=e.source_id JOIN nodes n ON n.ref_id=e.target_id WHERE r.canonical_ref=?1 AND e.kind=?2 AND e.position>?3 ORDER BY e.position LIMIT ?4")?;
            let mut items = statement.query_map(params![key,MEMBERSHIP,after.map(|v|v as i64).unwrap_or(-1),(limit+1) as i64], |row| row.get::<_,String>(0))?.map(|row| Ok(serde_json::from_str::<Event>(&row?)?)).collect::<CwpdbResult<Vec<_>>>()?;
            let complete = items.len() <= limit;
            items.truncate(limit);
            let continuation = if complete { None } else { items.last().map(|e|e.sequence) };
            Ok(EventPage {items,continuation,complete})
        })
    }
}

fn apply(
    request: &Mutation,
    previous: Option<Record>,
    now: u64,
) -> CwpdbResult<(Record, Option<Event>)> {
    if let Operation::Admit {
        conversation_ref,
        parent_invocation_ref,
        admitted,
    } = &request.operation
    {
        check_ref(conversation_ref)?;
        if let Some(parent) = parent_invocation_ref {
            check_ref(parent)?;
        }
        if request.expected_revision.is_some() {
            return Err("admission cannot name an expected revision".into());
        }
        if previous.is_some() {
            return Err("invocation already admitted".into());
        }
        return Ok((
            Record {
                workspace_ref: request.context.workspace_ref.clone(),
                invocation_ref: request.invocation_ref.clone(),
                conversation_ref: conversation_ref.clone(),
                parent_invocation_ref: parent_invocation_ref.clone(),
                admitted: admitted.clone(),
                assignment: None,
                revision: String::new(),
                next_sequence: 0,
                terminal: None,
            },
            None,
        ));
    }
    let mut record = previous.ok_or("invocation not found")?;
    if request.expected_revision.as_ref() != Some(&record.revision) {
        return Err("invocation revision conflict".into());
    }
    if record.terminal.is_some() {
        return Err("invocation is terminal".into());
    }
    let event = match &request.operation {
        Operation::Assign { assignment } => {
            check_ref(&assignment.executor_ref)?;
            check_ref(&assignment.assignment_ref)?;
            if record.assignment.is_some() {
                return Err("invocation assignment is immutable".into());
            }
            if assignment.expires_at_unix_ms <= now
                || assignment.route_fingerprint.is_empty()
                || assignment.generation == 0
            {
                return Err("invalid invocation assignment".into());
            }
            record.assignment = Some(assignment.clone());
            None
        }
        Operation::Append {
            assignment_ref,
            executor_ref,
            generation,
            event,
        } => {
            let assignment = record
                .assignment
                .as_ref()
                .ok_or("invocation has no assignment")?;
            if assignment.assignment_ref != *assignment_ref
                || assignment.executor_ref != *executor_ref
                || assignment.generation != *generation
                || assignment.expires_at_unix_ms <= now
            {
                return Err("stale or foreign invocation assignment".into());
            }
            if event.sequence != record.next_sequence {
                return Err("invocation event sequence conflict".into());
            }
            Some(event.clone())
        }
        Operation::Stop { kind, reason } => {
            if !matches!(kind, EventKind::Interrupted | EventKind::Cancelled) {
                return Err("coordinator stop requires interruption or cancellation".into());
            }
            Some(Event {
                sequence: record.next_sequence,
                kind: *kind,
                payload: reason.clone(),
            })
        }
        Operation::Admit { .. } => unreachable!(),
    };
    if let Some(event) = &event {
        record.next_sequence = record
            .next_sequence
            .checked_add(1)
            .filter(|v| *v <= i64::MAX as u64)
            .ok_or("invocation sequence exhausted")?;
        if event.kind != EventKind::Progress {
            record.terminal = Some(event.kind);
        }
    }
    Ok((record, event))
}

fn store_event(tx: &Transaction<'_>, root: &str, event: &Event) -> CwpdbResult<String> {
    let reference = crate::derived_ref(EVENT, &[root, &event.sequence.to_string()]);
    crate::put_node(tx, &reference, EVENT, &serde_json::to_value(event)?)?;
    crate::put_edge(
        tx,
        root,
        MEMBERSHIP,
        &reference,
        event.sequence as i64,
        &json!({}),
    )?;
    Ok(reference)
}

pub(crate) fn mutate_in_transaction(
    tx: &Transaction<'_>,
    request: &Mutation,
    authorizer: &dyn Authorizer,
    now_unix_ms: u64,
) -> CwpdbResult<Outcome> {
    check_context(&request.context, &request.invocation_ref)?;
    let arguments = serde_json::to_value(request)?;
    if serde_json::to_vec(&arguments)?.len() > MAX_PAYLOAD_BYTES {
        return Err("invocation mutation payload exceeds 512 KiB".into());
    }
    let key = key(&request.context, &request.invocation_ref);
    let scope = crate::derived_ref(
        "declared_mutation_scope",
        &[&request.context.workspace_ref, &request.context.caller_ref],
    );

    authorizer.authorize(&request.context, &request.invocation_ref, true)?;
    match sequence_receipt::check(tx, &scope, &request.request_id, &arguments)? {
        sequence_receipt::Replay::Retained(result) => {
            authorizer.authorize(&request.context, &request.invocation_ref, true)?;
            return Ok(Outcome::Committed(
                serde_json::from_value::<SavedReceipt>(result)?.receipt,
            ));
        }
        sequence_receipt::Replay::CommittedResultExpired => {
            authorizer.authorize(&request.context, &request.invocation_ref, true)?;
            return Ok(Outcome::CommittedResultExpired);
        }
        sequence_receipt::Replay::NotCommitted => {}
    }
    let previous = read(tx, &key)?;
    let (mut record, event) = apply(request, previous, now_unix_ms)?;
    crate::put_node(tx, &key, KIND, &serde_json::to_value(&record)?)?;
    record = read(tx, &key)?.ok_or("invocation journal write disappeared")?;
    let dependency = if let Some(event) = event {
        store_event(tx, &key, &event)?
    } else {
        key.clone()
    };
    let result = receipt(&record);
    sequence_receipt::commit(
        tx,
        &scope,
        &request.request_id,
        &arguments,
        &serde_json::to_value(SavedReceipt {
            request: request.clone(),
            receipt: result.clone(),
        })?,
        &[dependency],
    )?;
    // A stop/revocation during storage work must roll back record, event and receipt together.
    authorizer.authorize(&request.context, &request.invocation_ref, true)?;
    Ok(Outcome::Committed(result))
}
