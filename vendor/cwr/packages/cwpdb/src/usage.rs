//! Dedicated local usage ledger operations; privacy/preferences/files stay in Desktop.
use crate::{
    self as cwpdb,
    declared::{Catalog, EntityBinding as EntityCatalog, InsertOutcome},
    ref_read::RefReadError,
    sequence_ledger::{AppendValue, AttemptBatch, SequenceLedger},
    sequence_read::{Direction, ReadBounds, SequenceIndex, SequencePage},
    CwpdbResult,
};
use rusqlite::{types::Value as SqlValue, Transaction};
use serde_json::{json, Value};
const EVENT: &str = "cwp://getworkloop.com/desktop/local-usage/event-definition";
const CURSOR: &str = "cwp://getworkloop.com/desktop/local-usage/cursor-definition";
const CURSOR_REF: &str = "cwp://getworkloop.com/desktop/local-usage/cursor";
pub struct UsageLedger {
    events: EntityCatalog,
    state: EntityCatalog,
}
#[derive(Debug, serde::Serialize)]
pub struct PurgeProgress {
    pub removed: usize,
    pub next: Option<i64>,
    pub complete: bool,
}
impl UsageLedger {
    pub fn new(catalog: &Catalog) -> CwpdbResult<Self> {
        Ok(Self {
            events: catalog.entity(EVENT)?,
            state: catalog.entity(CURSOR)?,
        })
    }
    fn ledger(&self) -> SequenceLedger<'_> {
        SequenceLedger {
            events: &self.events,
            event_definition: EVENT,
            state: &self.state,
            state_definition: CURSOR,
            state_ref: CURSOR_REF,
            state_sequence: "sequence",
            record_envelope: true,
            sequence: "sequence",
            payload: "json",
            bytes: "bytes",
            max_sequence: 9_007_199_254_740_991,
        }
    }
    fn index(mask: u8) -> SequenceIndex {
        SequenceIndex {
            name: format!("declared_usage_sequence_{mask}_v1"),
            kind: "desktop_usage_event".into(),
            schema: "cwp://org.coworkerprotocol/cwpdb/schemas/desktop_usage_event/2026-09".into(),
            sequence: "sequence".into(),
            receipt: "receiptMs".into(),
            record_envelope: true,
            equalities: ["event", "control", "hostSession"]
                .iter()
                .enumerate()
                .filter(|(bit, _)| mask & (1 << bit) != 0)
                .map(|(_, s)| s.to_string())
                .collect(),
        }
    }
    /// Central index build under the coordinator-approved finite physical-index exception.
    pub fn initialize_in_transaction(&self, tx: &Transaction<'_>) -> CwpdbResult<()> {
        let obsolete: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='usage_events' AND type='table')",
            [],
            |r| r.get(0),
        )?;
        if obsolete {
            return Err("obsolete usage table; explicit reset required".into());
        }
        match cwpdb::ref_read::get_ref(tx, CURSOR_REF) {
            Ok(_) => {
                self.state.get_in_connection(tx, CURSOR, CURSOR_REF)?;
            }
            Err(RefReadError::NotFound) => {
                let any: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM nodes WHERE kind='desktop_usage_event')",
                    [],
                    |r| r.get(0),
                )?;
                if any {
                    return Err("missing retained usage high-water; refusing reset".into());
                }
                self.state.create_in_transaction(
                    tx,
                    CURSOR,
                    CURSOR_REF,
                    &json!({"record":{"sequence":0}}),
                )?;
            }
            Err(e) => return Err(e.into()),
        }
        let totals = self.validate_retained(tx)?;
        self.accounting().initialize(tx, totals.0, totals.1)?;
        for mask in 0..8 {
            Self::index(mask).install(tx)?;
        }
        Ok(())
    }
    fn validate_retained(&self, tx: &Transaction<'_>) -> CwpdbResult<(u64, u64)> {
        self.validate_retained_with_limits(tx, 100_000, 128 * 1024 * 1024)
    }
    fn validate_retained_with_limits(
        &self,
        tx: &Transaction<'_>,
        max_rows: usize,
        max_bytes: u64,
    ) -> CwpdbResult<(u64, u64)> {
        let state = self.state.get_in_connection(tx, CURSOR, CURSOR_REF)?;
        let high_water = state.value.properties["record"]["sequence"]
            .as_i64()
            .ok_or("invalid usage high-water")?;
        // Startup is an exhaustive validation, never a sequence keyset scan.
        // Refuse oversized retained stores atomically; do not publish prefix totals.
        let mut total_bytes = 0u64;
        let mut envelope_bytes = 0u64;
        let mut seen = std::collections::BTreeSet::new();
        let mut statement=tx.prepare("SELECT b.canonical_ref, length(CAST(n.props AS BLOB)) FROM nodes n INDEXED BY nodes_by_kind JOIN ref_bindings b ON b.ref_id=n.ref_id WHERE n.kind='desktop_usage_event'")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            if seen.len() >= max_rows {
                return Err("retained usage validation row limit exceeded".into());
            }
            let stored_bytes = u64::try_from(row.get::<_, i64>(1)?)?;
            envelope_bytes = envelope_bytes
                .checked_add(stored_bytes)
                .ok_or("retained envelope accounting overflow")?;
            if stored_bytes > 1_048_576 || envelope_bytes > max_bytes {
                return Err("retained usage validation envelope limit exceeded".into());
            }
            let reference: String = row.get(0)?;
            let record = self.events.get_in_connection(tx, EVENT, &reference)?;
            let value = &record.value.properties["record"];
            let sequence = value["sequence"]
                .as_i64()
                .ok_or("invalid retained usage sequence")?;
            let uuid = value["uuid"]
                .as_str()
                .ok_or("invalid retained usage UUID")?;
            let body = value["json"]
                .as_str()
                .ok_or("invalid retained usage payload")?;
            if sequence <= 0
                || sequence > high_water
                || !seen.insert(sequence)
                || reference != format!("cwp://getworkloop.com/desktop/local-usage/events/{uuid}")
                || body.len() > 65536
                || value["bytes"].as_u64() != Some(body.len() as u64)
            {
                return Err("malformed retained usage ledger; refusing cutover".into());
            }
            total_bytes = total_bytes
                .checked_add(body.len() as u64)
                .ok_or("retained byte accounting overflow")?;
            if total_bytes > max_bytes {
                return Err("retained usage validation byte limit exceeded".into());
            }
        }
        let wrong_kind:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM nodes WHERE schema_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref='cwp://org.coworkerprotocol/cwpdb/schemas/desktop_usage_event/2026-09') AND kind!='desktop_usage_event')",[],|r|r.get(0))?;
        if wrong_kind {
            return Err("retained usage schema/kind conflict".into());
        }
        Ok((seen.len() as u64, total_bytes))
    }
    pub fn append_in_transaction(
        &self,
        tx: &Transaction<'_>,
        records: &[Value],
    ) -> CwpdbResult<AttemptBatch> {
        if self.clearing_in_transaction(tx)? {
            return Err("usage history clear is pending".into());
        }
        let values = records
            .iter()
            .map(|record| {
                let uuid = record["uuid"].as_str().ok_or("missing event UUID")?;
                let mut record = record.clone();
                record["sdkVersion"] = json!("1.288.0");
                Ok(AppendValue {
                    reference: format!("cwp://getworkloop.com/desktop/local-usage/events/{uuid}"),
                    value: json!({"record":record}),
                })
            })
            .collect::<CwpdbResult<Vec<_>>>()?;
        let batch = self
            .ledger()
            .append_attempt_batch_in_transaction(tx, &values)?;
        let mut count = 0i64;
        let mut bytes = 0i64;
        for outcome in &batch.outcomes {
            if let InsertOutcome::Created(record) = outcome {
                count += 1;
                bytes = bytes
                    .checked_add(
                        record.value.properties["record"]["bytes"]
                            .as_i64()
                            .ok_or("invalid event bytes")?,
                    )
                    .ok_or("event bytes overflow")?;
            }
        }
        self.accounting().adjust(tx, count, bytes)?;
        Ok(batch)
    }
    /// Internal page retains incomplete work/byte outcomes; Desktop must continue
    /// or surface them rather than label a short page final or spin without progress.
    #[cfg(test)]
    pub fn page_in_transaction(
        &self,
        tx: &Transaction<'_>,
        filters: [Option<&str>; 3],
        before: Option<i64>,
    ) -> CwpdbResult<SequencePage> {
        let mut mask = 0;
        let mut values = Vec::new();
        for (bit, filter) in filters.iter().enumerate() {
            if let Some(filter) = filter {
                if filter.is_empty() || filter.len() > 1024 {
                    return Err("invalid usage filter".into());
                }
                mask |= 1 << bit;
                values.push(SqlValue::Text(filter.to_string()));
            }
        }
        Self::index(mask).read(
            tx,
            &values,
            Direction::Descending,
            before,
            None,
            ReadBounds {
                rows: 50,
                bytes: 1_048_576,
                work: 10000,
            },
        )
    }
    /// Bounded purge joins the same transaction. Cursor is never selected.
    pub fn expire_batch_in_transaction(
        &self,
        tx: &Transaction<'_>,
        cutoff: i64,
        after: Option<i64>,
    ) -> CwpdbResult<PurgeProgress> {
        let page = Self::index(0).read(
            tx,
            &[],
            Direction::Ascending,
            after,
            Some(cutoff),
            ReadBounds {
                rows: 100,
                bytes: 524288,
                work: 10000,
            },
        )?;
        self.purge_page(tx, &page)
    }
    fn purge_page(&self, tx: &Transaction<'_>, page: &SequencePage) -> CwpdbResult<PurgeProgress> {
        let selected = page
            .items
            .iter()
            .map(|r| (r.reference.as_str(), &r.revision))
            .collect::<Vec<_>>();
        let mut count = selected.len();
        loop {
            match self
                .events
                .purge_batch_in_transaction(tx, EVENT, &selected[..count])
            {
                Ok(()) => break,
                // Shared helper checks this bound BEFORE removal. Reduce the
                // prefix without catching any partial-mutation/storage error.
                Err(error)
                    if count > 1 && error.to_string() == "disposable purge work limit exceeded" =>
                {
                    count /= 2
                }
                Err(error) => return Err(error),
            }
        }
        let bytes = page.items[..count].iter().try_fold(0i64, |sum, row| {
            sum.checked_add(
                row.value["record"]["bytes"]
                    .as_i64()
                    .ok_or("invalid retained event bytes")?,
            )
            .ok_or("purge byte accounting overflow")
        })?;
        self.accounting().adjust(tx, -(count as i64), -bytes)?;
        let all = count == page.items.len();
        Ok(PurgeProgress {
            removed: count,
            next: if all {
                page.next
            } else {
                Some(page.items[count - 1].sequence)
            },
            complete: all && page.complete,
        })
    }
    pub fn clear_batch_in_transaction(&self, tx: &Transaction<'_>) -> CwpdbResult<PurgeProgress> {
        let page = Self::index(0).read(
            tx,
            &[],
            Direction::Ascending,
            None,
            None,
            ReadBounds {
                rows: 100,
                bytes: 524288,
                work: 10000,
            },
        )?;
        let progress = self.purge_page(tx, &page)?;
        let current = self.state.get_in_connection(tx, CURSOR, CURSOR_REF)?;
        let mut value = current.value.properties.clone();
        value["record"]["clearing"] = json!(!progress.complete);
        self.state
            .replace_in_transaction(tx, CURSOR, CURSOR_REF, &current.revision, &value)?;
        Ok(progress)
    }
}

#[cfg(test)]
#[path = "usage_tests.rs"]
mod tests;

impl UsageLedger {
    /// Preserve the public 50-row projection while hydrating large raw samples
    /// through bounded internal chunks in this same snapshot. Never expose raw
    /// event JSON in the inspection projection or silently complete a prefix.
    pub fn inspect_page_in_transaction(
        &self,
        tx: &Transaction<'_>,
        filters: [Option<&str>; 3],
        before: Option<i64>,
    ) -> CwpdbResult<Value> {
        self.expire_now_in_transaction(tx)?;
        let mut mask = 0;
        let mut values = Vec::new();
        for (bit, value) in filters.iter().enumerate() {
            if let Some(value) = value {
                if value.is_empty() || value.len() > 1024 {
                    return Err("invalid usage filter".into());
                }
                mask |= 1 << bit;
                values.push(SqlValue::Text(value.to_string()));
            }
        }
        let mut items = Vec::new();
        let mut after = before;
        let mut bytes = 0;
        loop {
            let page = Self::index(mask).read(
                tx,
                &values,
                Direction::Descending,
                after,
                None,
                ReadBounds {
                    rows: 51 - items.len(),
                    bytes: 1_048_576,
                    work: 10000,
                },
            )?;
            bytes += page.bytes;
            if bytes > 4_194_304 {
                return Err("usage page byte work limit exceeded".into());
            }
            let complete = page.complete;
            let next = page.next;
            let count = page.items.len();
            items.extend(page.items);
            if items.len() == 51 || complete {
                break;
            }
            if count == 0 || next.is_none() || next == after {
                return Err("usage page cannot progress within work bound".into());
            }
            after = next;
        }
        let more = items.len() > 50;
        items.truncate(50);
        let next = if more {
            items.last().map(|r| r.sequence)
        } else {
            None
        };
        let rows=items.into_iter().map(|row| {
            let r=&row.value["record"];
            json!({"sequence":row.sequence,"uuid":r["uuid"],"event":r["event"],"control":r["control"],"sdkTime":r["sdkTime"],"receiptMs":r["receiptMs"],"session":r["hostSession"],"renderer":r["renderer"]})
        }).collect::<Vec<_>>();
        Ok(json!({"rows":rows,"next":next}))
    }
    fn accounting(&self) -> crate::sequence_accounting_ledger::LedgerAccounting<'_> {
        crate::sequence_accounting_ledger::LedgerAccounting {
            catalog: &self.state,
            definition: CURSOR,
            reference: CURSOR_REF,
            record_envelope: true,
            rows_field: "rows",
            bytes_field: "bytes",
        }
    }
    pub fn account_in_transaction(
        &self,
        tx: &Transaction<'_>,
    ) -> CwpdbResult<crate::sequence_accounting_ledger::Accounting> {
        self.accounting().read(tx)
    }
    pub fn clearing_in_transaction(&self, tx: &Transaction<'_>) -> CwpdbResult<bool> {
        let state = self.state.get_in_connection(tx, CURSOR, CURSOR_REF)?;
        Ok(state.value.properties["record"]["clearing"]
            .as_bool()
            .unwrap_or(false))
    }
    pub fn raw_in_transaction(
        &self,
        tx: &Transaction<'_>,
        uuid: &str,
    ) -> CwpdbResult<Option<String>> {
        self.expire_now_in_transaction(tx)?;
        let reference = format!("cwp://getworkloop.com/desktop/local-usage/events/{uuid}");
        self.events.validate_reference(&reference)?;
        match cwpdb::ref_read::get_ref(tx, &reference) {
            Ok(_) => Ok(Some(
                self.events
                    .get_in_connection(tx, EVENT, &reference)?
                    .value
                    .properties["record"]["json"]
                    .as_str()
                    .ok_or("invalid retained event JSON string")?
                    .to_owned(),
            )),
            Err(RefReadError::NotFound) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
    pub fn evict_oldest_batch_in_transaction(
        &self,
        tx: &Transaction<'_>,
        limit: usize,
    ) -> CwpdbResult<PurgeProgress> {
        if limit == 0 || limit > 100 {
            return Err("invalid eviction bound".into());
        }
        let page = Self::index(0).read(
            tx,
            &[],
            Direction::Ascending,
            None,
            None,
            ReadBounds {
                rows: limit,
                bytes: 524288,
                work: 10000,
            },
        )?;
        self.purge_page(tx, &page)
    }
}

#[derive(Debug, serde::Serialize, PartialEq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum AppendOnce {
    Committed { duplicates: usize },
    CommittedResultExpired,
}
impl UsageLedger {
    /// Bound local request-ID replay. Expiration must run first in the same
    /// owning operation; current authorization is supplied by the existing host.
    pub fn append_once_in_transaction(
        &self,
        tx: &Transaction<'_>,
        request_id: &str,
        records: &[Value],
    ) -> CwpdbResult<AppendOnce> {
        self.expire_now_in_transaction(tx)?;
        if records.len() > 100 || serde_json::to_vec(records)?.len() > 524288 {
            return Err("append input limit exceeded".into());
        }
        use cwpdb::sequence_receipt::{self, Replay};
        let args = json!(records);
        match sequence_receipt::check(tx, CURSOR_REF, request_id, &args)? {
            Replay::Retained(value) => {
                return Ok(AppendOnce::Committed {
                    duplicates: value["duplicates"]
                        .as_u64()
                        .ok_or("invalid retained append outcome")?
                        as usize,
                })
            }
            Replay::CommittedResultExpired => return Ok(AppendOnce::CommittedResultExpired),
            Replay::NotCommitted => {}
        }
        let batch = self.append_in_transaction(tx, records)?;
        let duplicates = batch
            .outcomes
            .iter()
            .filter(|r| matches!(r, InsertOutcome::Existing(_)))
            .count();
        let dependencies = batch
            .outcomes
            .iter()
            .map(|r| match r {
                InsertOutcome::Created(r) | InsertOutcome::Existing(r) => r.reference.clone(),
            })
            .collect::<Vec<_>>();
        sequence_receipt::commit(
            tx,
            CURSOR_REF,
            request_id,
            &args,
            &json!({"duplicates":duplicates}),
            &dependencies,
        )?;
        Ok(AppendOnce::Committed { duplicates })
    }
}

impl UsageLedger {
    fn expire_now_in_transaction(&self, tx: &Transaction<'_>) -> CwpdbResult<()> {
        let now = i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis(),
        )?;
        let cutoff = now - 14 * 24 * 60 * 60 * 1000;
        let mut after = None;
        for _ in 0..1000 {
            let progress = self.expire_batch_in_transaction(tx, cutoff, after)?;
            if progress.complete {
                return Ok(());
            }
            if progress.next.is_none() || progress.next == after {
                return Err("expiration made no progress".into());
            }
            after = progress.next;
        }
        Err("usage expiration work limit exceeded".into())
    }
}
