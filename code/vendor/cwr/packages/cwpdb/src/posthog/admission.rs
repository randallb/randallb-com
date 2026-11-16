//! Enqueue, quota and retention invariants are complete owning transactions.
use super::*;
use crate::{
    sequence_ledger::{AppendValue, SequenceLedger},
    sequence_read::{Direction, ReadBounds},
};
use rusqlite::{Transaction, TransactionBehavior};

// Stop serialization at the per-event admission limit instead of allocating
// an unbounded serialized copy of a rejected value.
fn serialized_event(value: &Value) -> Option<String> {
    struct Capped(Vec<u8>);
    impl std::io::Write for Capped {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > 65_536 - self.0.len() {
                return Err(std::io::ErrorKind::InvalidData.into());
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut output = Capped(Vec::new());
    serde_json::to_writer(&mut output, value).ok()?;
    String::from_utf8(output.0).ok()
}
pub(super) fn add_dropped(
    tx: &Transaction<'_>,
    declarations: &Declarations,
    count: usize,
) -> CwpdbResult<()> {
    if count == 0 {
        return Ok(());
    }
    let state = declarations
        .state
        .get_in_connection(tx, STATE_DEFINITION, STATE_REF)?;
    let mut value = state.value.properties;
    let dropped = value["record"]["counters"]["dropped"]
        .as_i64()
        .ok_or("invalid drop counter")?;
    value["record"]["counters"]["dropped"] = json!(dropped
        .checked_add(i64::try_from(count)?)
        .ok_or("drop counter exhausted")?);
    declarations.state.replace_in_transaction(
        tx,
        STATE_DEFINITION,
        STATE_REF,
        &state.revision,
        &value,
    )?;
    Ok(())
}
fn expire_in_transaction(
    tx: &Transaction<'_>,
    declarations: &Declarations,
    cutoff: i64,
) -> CwpdbResult<usize> {
    let mut after = None;
    let mut examined = 0;
    let mut removed = 0;
    loop {
        let page = reads::admission_index().read(
            tx,
            &[],
            Direction::Ascending,
            after,
            Some(cutoff),
            ReadBounds {
                rows: 250,
                bytes: 1_048_576,
                work: MAX_QUEUE_EVENTS - examined,
            },
        )?;
        examined += page.examined;
        let selected: Vec<_> = page
            .items
            .iter()
            .map(|r| (r.reference.as_str(), &r.revision))
            .collect();
        declarations
            .event
            .purge_batch_in_transaction(tx, EVENT_DEFINITION, &selected)?;
        accounting::remove(
            tx,
            declarations,
            &page
                .items
                .iter()
                .map(|r| r.value.clone())
                .collect::<Vec<_>>(),
        )?;
        removed += selected.len();
        if page.complete {
            break;
        }
        if examined >= MAX_QUEUE_EVENTS {
            return Err("expiry exhausted queue work budget".into());
        }
        // Cutoff misses consume bytes too: an empty page is valid when its
        // cursor advances. Never confuse that with end-of-scan or retry an
        // unchanged boundary inside the owning transaction.
        let next = page.next.ok_or("expiry read cannot make progress")?;
        if page.examined == 0 || next <= after.unwrap_or(0) {
            return Err("expiry read cannot make progress".into());
        }
        after = Some(next);
    }
    add_dropped(tx, declarations, removed)?;
    Ok(removed)
}
impl PosthogOutbox {
    pub fn expire(&mut self, now: u64) -> CwpdbResult<usize> {
        let cutoff = i64::try_from(now.saturating_sub(14 * 24 * 60 * 60 * 1000))?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let removed = expire_in_transaction(&tx, &self.declarations, cutoff)?;
        tx.commit()?;
        Ok(removed)
    }
    /// Up to 250 filtered metric events per call. Event size validation precedes
    /// quota eviction; oversized events count as drops without evicting work.
    /// Persistent graph totals include quarantine and change with each mutation.
    pub fn enqueue(&mut self, destination: &str, now: u64, events: &[Value]) -> CwpdbResult<()> {
        if destination.is_empty() || destination.len() > 512 || events.len() > 250 {
            return Err("invalid outbox enqueue bounds".into());
        }
        for event in events {
            if !event.is_object()
                || event["uuid"]
                    .as_str()
                    .and_then(|s| uuid::Uuid::parse_str(s).ok())
                    .is_none()
                || !event["properties"].is_object()
                || !event["event"].is_string()
                || !event["timestamp"].is_string()
            {
                return Err("invalid telemetry event".into());
            }
        }
        let received = i64::try_from(now)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        // No event is admitted before validation; expiry and its counter changes
        // are in the same owning transaction as this enqueue batch.
        expire_in_transaction(
            &tx,
            &self.declarations,
            i64::try_from(now.saturating_sub(14 * 24 * 60 * 60 * 1000))?,
        )?;
        let mut accounting = accounting::ledger(&self.declarations).read(&tx)?;
        for event in events {
            if serialized_event(event).is_none() {
                add_dropped(&tx, &self.declarations, 1)?;
                continue;
            }
            let state =
                self.declarations
                    .state
                    .get_in_connection(&tx, STATE_DEFINITION, STATE_REF)?;
            let mut event = event.clone();
            event["properties"]["export_dropped_total"] =
                state.value.properties["record"]["counters"]["dropped"].clone();
            let Some(original) = serialized_event(&event) else {
                add_dropped(&tx, &self.declarations, 1)?;
                continue;
            };
            while accounting.rows >= MAX_QUEUE_EVENTS as u64
                || accounting.bytes + original.len() as u64 > MAX_QUEUE_BYTES as u64
            {
                let page = reads::admission_index().read(
                    &tx,
                    &[],
                    Direction::Ascending,
                    None,
                    None,
                    ReadBounds {
                        rows: 1,
                        bytes: 1_048_576,
                        work: 1,
                    },
                )?;
                let oldest = page
                    .items
                    .first()
                    .ok_or("outbox capacity accounting mismatch")?;
                self.declarations.event.purge_batch_in_transaction(
                    &tx,
                    EVENT_DEFINITION,
                    &[(&oldest.reference, &oldest.revision)],
                )?;
                accounting::remove(&tx, &self.declarations, std::slice::from_ref(&oldest.value))?;
                accounting.rows -= 1;
                accounting.bytes = accounting
                    .bytes
                    .checked_sub(
                        oldest.value["record"]["bytes"]
                            .as_u64()
                            .ok_or("invalid retained byte count")?,
                    )
                    .ok_or("outbox byte accounting mismatch")?;
                add_dropped(&tx, &self.declarations, 1)?;
            }
            let previous = state.value.properties["record"]["high_water"]
                .as_i64()
                .ok_or("invalid high-water")?;
            let next = previous
                .checked_add(1)
                .ok_or("admission sequence exhausted")?;
            let ledger = SequenceLedger {
                events: &self.declarations.event,
                event_definition: EVENT_DEFINITION,
                state: &self.declarations.state,
                state_definition: STATE_DEFINITION,
                state_ref: STATE_REF,
                state_sequence: "high_water",
                record_envelope: true,
                sequence: "sequence",
                payload: "event",
                bytes: "bytes",
                max_sequence: i64::MAX,
            };
            let appended = ledger.append_attempt_batch_in_transaction(
                &tx,
                &[AppendValue {
                    reference: event_ref(next),
                    value: event_value(next, received, destination.into(), original.clone(), false),
                }],
            )?;
            if appended.high_water != next {
                return Err("outbox sequence advance mismatch".into());
            }
            for outcome in appended.outcomes {
                if let crate::declared::InsertOutcome::Created(record) = outcome {
                    let bytes = record.value.properties["record"]["event"]
                        .as_str()
                        .ok_or("invalid original event")?
                        .len();
                    accounting = accounting::ledger(&self.declarations).adjust(
                        &tx,
                        1,
                        i64::try_from(bytes)?,
                    )?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }
}
