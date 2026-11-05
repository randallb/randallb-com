//! Outbox totals use Track 2's generic graph-state helper, not queue scans.
use super::*;
use crate::sequence_accounting_ledger::LedgerAccounting;
use rusqlite::Transaction;
use std::collections::BTreeSet;
pub(super) fn ledger(declarations: &Declarations) -> LedgerAccounting<'_> {
    LedgerAccounting {
        catalog: &declarations.state,
        definition: STATE_DEFINITION,
        reference: STATE_REF,
        record_envelope: true,
        rows_field: "live_rows",
        bytes_field: "live_bytes",
    }
}
// Startup alone scans the retained queue. Validate original UTF-8 sizes, high
// water, and full traversal before installing missing totals. Stale or corrupt
// existing totals fail closed; the transaction preserves the retained source.
pub(super) fn initialize(tx: &Transaction<'_>, declarations: &Declarations) -> CwpdbResult<()> {
    let state = declarations
        .state
        .get_in_connection(tx, STATE_DEFINITION, STATE_REF)?;
    let high_water = state.value.properties["record"]["high_water"]
        .as_i64()
        .ok_or("invalid high-water")?;
    // Admission sequence is untrusted retained data, so it cannot delimit this
    // validation scan. Include either discriminator to reject schema/kind
    // mismatches, and include edges so the node-only domain cannot hide them.
    // Stream canonical refs before hydration; a single cursor has no sequence
    // page boundary that can skip invalid or duplicate sequence values.
    let mut statement = tx.prepare(
        "SELECT b.canonical_ref,length(n.props),0 FROM nodes n
         LEFT JOIN ref_bindings b ON b.ref_id=n.ref_id
         WHERE n.kind='posthog_event' OR n.schema_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)
         UNION ALL
         SELECT b.canonical_ref,length(e.props),1 FROM edges e
         LEFT JOIN ref_bindings b ON b.ref_id=e.ref_id
         WHERE e.kind='posthog_event' OR e.schema_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)
         LIMIT 10001",
    )?;
    let scoped = statement.query_map([EVENT_DEFINITION], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, u32>(1)?,
            r.get::<_, bool>(2)?,
        ))
    })?;
    let mut sequences = BTreeSet::new();
    let (mut rows, mut bytes, mut envelope_bytes) = (0u64, 0u64, 0u64);
    for entry in scoped {
        let (reference, stored_bytes, edge) = entry?;
        if edge || rows >= MAX_QUEUE_EVENTS as u64 || stored_bytes > 1_048_576 {
            return Err(
                "retained outbox exceeds validation bounds or has wrong record type".into(),
            );
        }
        let value = declarations
            .event
            .get_in_connection(tx, EVENT_DEFINITION, &reference)?
            .value
            .properties;
        let envelope = serde_json::to_vec(&value)?.len() as u64;
        envelope_bytes = envelope_bytes
            .checked_add(envelope)
            .ok_or("envelope total overflow")?;
        if envelope > 1_048_576 || envelope_bytes > 64 * 1024 * 1024 {
            return Err("retained outbox exceeds validation byte limits".into());
        }
        let record = &value["record"];
        let sequence = record["sequence"]
            .as_i64()
            .ok_or("invalid retained sequence")?;
        let original = record["event"].as_str().ok_or("invalid original event")?;
        if sequence <= 0
            || sequence > high_water
            || !sequences.insert(sequence)
            || reference != event_ref(sequence)
            || record["bytes"].as_u64() != Some(original.len() as u64)
        {
            return Err("invalid or duplicate retained outbox accounting source".into());
        }
        rows += 1;
        bytes = bytes
            .checked_add(original.len() as u64)
            .ok_or("byte total overflow")?;
        if bytes > MAX_QUEUE_BYTES as u64 {
            return Err("retained outbox exceeds accounting limits".into());
        }
    }
    ledger(declarations).initialize(tx, rows, bytes)
}
pub(super) fn remove(
    tx: &Transaction<'_>,
    declarations: &Declarations,
    values: &[Value],
) -> CwpdbResult<()> {
    let bytes = values
        .iter()
        .try_fold(0i64, |sum, value| -> CwpdbResult<i64> {
            let event = value["record"]["event"]
                .as_str()
                .ok_or("invalid original event")?;
            if value["record"]["bytes"].as_u64() != Some(event.len() as u64) {
                return Err("invalid original byte count".into());
            }
            Ok(sum
                .checked_add(i64::try_from(event.len())?)
                .ok_or("byte total overflow")?)
        })?;
    ledger(declarations).adjust(tx, -i64::try_from(values.len())?, -bytes)?;
    Ok(())
}
