//! Finite shared read paths and persistent declared outbox accounting.
use super::*;
use crate::sequence_read::{Direction, ReadBounds, SequenceAccounting, SequenceIndex};
use rusqlite::{types::Value as SqlValue, Transaction, TransactionBehavior};

pub type PendingEvent = (Selection, Value);
pub(super) fn admission_index() -> SequenceIndex {
    SequenceIndex {
        name: "posthog_admission_v1".into(),
        kind: "posthog_event".into(),
        schema: EVENT_DEFINITION.into(),
        sequence: "sequence".into(),
        receipt: "received".into(),
        record_envelope: true,
        equalities: vec![],
    }
}
pub(super) fn pending_index() -> SequenceIndex {
    SequenceIndex {
        name: "posthog_pending_v1".into(),
        equalities: vec!["destination".into(), "quarantined".into()],
        ..admission_index()
    }
}
pub(super) fn install(tx: &Transaction<'_>) -> CwpdbResult<()> {
    admission_index().install(tx)?;
    pending_index().install(tx)?;
    Ok(())
}
pub(super) fn accounting(
    db: &Connection,
    declarations: &Declarations,
) -> CwpdbResult<SequenceAccounting> {
    let state = declarations
        .state
        .get_in_connection(db, STATE_DEFINITION, STATE_REF)?;
    let record = &state.value.properties["record"];
    let rows = usize::try_from(
        record["live_rows"]
            .as_u64()
            .ok_or("missing row accounting")?,
    )?;
    let bytes = record["live_bytes"]
        .as_u64()
        .ok_or("missing byte accounting")?;
    let page = admission_index().read(
        db,
        &[],
        Direction::Descending,
        None,
        None,
        ReadBounds {
            rows: 1,
            bytes: 1_048_576,
            work: 1,
        },
    )?;
    let max_live_sequence = page
        .items
        .first()
        .map(|r| {
            r.value["record"]["sequence"]
                .as_i64()
                .ok_or("invalid live sequence")
        })
        .transpose()?
        .unwrap_or(0);
    Ok(SequenceAccounting {
        rows,
        bytes,
        max_live_sequence,
        complete: true,
    })
}
impl PosthogOutbox {
    pub fn accounting(&self) -> CwpdbResult<SequenceAccounting> {
        let tx = self.db.unchecked_transaction()?;
        let result = accounting(&tx, &self.declarations)?;
        tx.commit()?;
        Ok(result)
    }
    pub fn dropped(&self) -> CwpdbResult<i64> {
        let state =
            self.declarations
                .state
                .get_in_connection(&self.db, STATE_DEFINITION, STATE_REF)?;
        state.value.properties["record"]["counters"]["dropped"]
            .as_i64()
            .ok_or_else(|| "invalid drop counter".into())
    }
    /// Preserve the exporter's original millisecond timestamp formatting.
    pub fn timestamp(&self, unix_ms: u64) -> CwpdbResult<String> {
        let millis = i64::try_from(unix_ms)?;
        Ok(self.db.query_row(
            "SELECT strftime('%Y-%m-%dT%H:%M:%fZ', ?1 / 1000.0, 'unixepoch')",
            [millis],
            |r| r.get(0),
        )?)
    }
    /// A selected batch owns its values and comparison tokens. All transactions
    /// end before the exporter performs HTTP. Invalid retained JSON is kept but
    /// quarantined, so it cannot block unrelated valid events indefinitely.
    pub fn pending(
        &mut self,
        destination: &str,
        body_overhead: usize,
    ) -> CwpdbResult<Vec<PendingEvent>> {
        if destination.is_empty() || destination.len() > 512 || body_overhead >= 1_000_000 {
            return Err("invalid pending destination/body budget".into());
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut result = Vec::new();
        let mut after = None;
        let mut examined = 0;
        let mut bytes = body_overhead;
        let mut envelope_bytes = 0usize;
        'pages: loop {
            let page = pending_index().read(
                &tx,
                &[SqlValue::Text(destination.into()), SqlValue::Integer(0)],
                Direction::Ascending,
                after,
                None,
                ReadBounds {
                    rows: 250 - result.len(),
                    bytes: 1_048_576,
                    work: MAX_QUEUE_EVENTS - examined,
                },
            )?;
            examined += page.examined;
            for row in page.items {
                self.declarations
                    .event
                    .validate(EVENT_DEFINITION, &row.value)?;
                let original = row.value["record"]["event"]
                    .as_str()
                    .ok_or("invalid retained event string")?;
                let parsed = serde_json::from_str::<Value>(original)
                    .ok()
                    .filter(Value::is_object);
                let parsed = match parsed {
                    Some(parsed) if original.len() <= 65_536 => parsed,
                    _ => {
                        let mut value = row.value;
                        value["record"]["quarantined"] = json!(true);
                        self.declarations.event.replace_in_transaction(
                            &tx,
                            EVENT_DEFINITION,
                            &row.reference,
                            &row.revision,
                            &value,
                        )?;
                        continue;
                    }
                };
                let envelope = serde_json::to_vec(&row.value)?.len();
                if envelope_bytes + envelope > 1_048_576 {
                    break 'pages;
                }
                let size = serde_json::to_vec(&parsed)?.len() + usize::from(!result.is_empty());
                if bytes + size > 1_000_000 {
                    break 'pages;
                }
                bytes += size;
                envelope_bytes += envelope;
                result.push((
                    Selection {
                        reference: row.reference,
                        revision: row.revision,
                    },
                    parsed,
                ));
                if result.len() == 250 {
                    break 'pages;
                }
            }
            if page.complete {
                break;
            }
            if examined >= MAX_QUEUE_EVENTS {
                return Err("pending read exhausted queue work budget".into());
            }
            after = page.next;
            if after.is_none() {
                return Err("pending read cannot make progress".into());
            }
        }
        tx.commit()?;
        Ok(result)
    }
}
