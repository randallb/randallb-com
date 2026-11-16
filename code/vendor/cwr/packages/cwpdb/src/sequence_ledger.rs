//! Local, owner-authorized sequence operations. Transaction lifetime stays in Rust.
//! This helper provides UUID reconciliation, NOT retained request-ID replay.
use crate::declared::{EntityBinding as EntityCatalog, InsertOutcome};
use crate::{ref_read::RefReadError, CwpdbResult};
use rusqlite::Transaction;
use serde_json::Value;

/// Trusted domain configuration; never supplied by a Desktop wire caller.
pub struct SequenceLedger<'a> {
    pub events: &'a EntityCatalog,
    pub event_definition: &'a str,
    pub state: &'a EntityCatalog,
    pub state_definition: &'a str,
    pub state_ref: &'a str,
    pub state_sequence: &'a str,
    pub record_envelope: bool,
    pub sequence: &'a str,
    pub payload: &'a str,
    pub bytes: &'a str,
    pub max_sequence: i64,
}
pub struct AppendValue {
    pub reference: String,
    pub value: Value,
}
#[derive(Debug)]
pub struct AttemptBatch {
    pub high_water: i64,
    pub outcomes: Vec<InsertOutcome>,
}
fn object_mut(
    value: &mut Value,
    envelope: bool,
) -> CwpdbResult<&mut serde_json::Map<String, Value>> {
    let selected = if envelope {
        &mut value["record"]
    } else {
        value
    };
    selected
        .as_object_mut()
        .ok_or_else(|| "sequence value must be an object".into())
}
impl SequenceLedger<'_> {
    /// Advance from retained authoritative state, never from max(live sequence).
    /// Domain startup must initialize/migrate that state, including deleted IDs.
    pub fn append_attempt_batch_in_transaction(
        &self,
        tx: &Transaction<'_>,
        values: &[AppendValue],
    ) -> CwpdbResult<AttemptBatch> {
        if values.len() > 100 || self.max_sequence <= 0 {
            return Err("append batch limit exceeded".into());
        }
        let current = self
            .state
            .get_in_connection(tx, self.state_definition, self.state_ref)?;
        let mut state = current.value.properties.clone();
        let record = object_mut(&mut state, self.record_envelope)?;
        let previous = record
            .get(self.state_sequence)
            .and_then(Value::as_i64)
            .ok_or("invalid retained high-water")?;
        if previous < 0 {
            return Err("invalid retained high-water".into());
        }
        let high_water = previous
            .checked_add(values.len() as i64)
            .filter(|n| *n <= self.max_sequence)
            .ok_or("sequence exhausted")?;
        let mut prepared = Vec::with_capacity(values.len());
        let mut total = 0usize;
        // Validate the entire batch before any creation or cursor mutation.
        for (offset, input) in values.iter().enumerate() {
            self.events.validate_reference(&input.reference)?;
            let mut value = input.value.clone();
            let record = object_mut(&mut value, self.record_envelope)?;
            let body = record
                .get(self.payload)
                .and_then(Value::as_str)
                .ok_or("invalid event payload")?;
            let bytes = body.len();
            if bytes > 65_536 {
                return Err("event byte limit exceeded".into());
            }
            record.insert(self.bytes.into(), Value::from(bytes));
            record.insert(
                self.sequence.into(),
                Value::from(previous + offset as i64 + 1),
            );
            self.events.validate(self.event_definition, &value)?;
            total = total
                .checked_add(serde_json::to_vec(&value)?.len())
                .ok_or("append bytes overflow")?;
            if total > 524_288 {
                return Err("append byte limit exceeded".into());
            }
            prepared.push(value);
        }
        let mut outcomes = Vec::with_capacity(values.len());
        for (input, value) in values.iter().zip(prepared) {
            match crate::ref_read::get_ref(tx, &input.reference) {
                Ok(_) => {
                    // The domain's UUID duplicate rule preserves original body,
                    // receipt and sequence. Do not weaken shared strict insertion.
                    let original = self.events.get_in_connection(
                        tx,
                        self.event_definition,
                        &input.reference,
                    )?;
                    outcomes.push(InsertOutcome::Existing(original));
                }
                Err(RefReadError::NotFound) => {
                    outcomes.push(self.events.insert_disposable_in_transaction(
                        tx,
                        self.event_definition,
                        &input.reference,
                        &value,
                    )?)
                }
                Err(error) => return Err(error.into()),
            }
        }
        if !values.is_empty() {
            object_mut(&mut state, self.record_envelope)?
                .insert(self.state_sequence.into(), Value::from(high_water));
            self.state.replace_in_transaction(
                tx,
                self.state_definition,
                self.state_ref,
                &current.revision,
                &state,
            )?;
        }
        Ok(AttemptBatch {
            high_water,
            outcomes,
        })
    }
}

#[cfg(test)]
impl SequenceLedger<'_> {
    pub fn state_in_transaction(
        &self,
        tx: &Transaction<'_>,
    ) -> CwpdbResult<crate::ref_read::RefRecord> {
        self.state
            .get_in_connection(tx, self.state_definition, self.state_ref)
    }
}
#[cfg(test)]
#[path = "sequence_ledger_tests.rs"]
mod tests;
