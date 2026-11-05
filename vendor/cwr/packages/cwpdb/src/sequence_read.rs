//! Trusted declaration-scoped keysets. No public SQL, transaction or cursor ABI.
//! Callers authorize the bound ledger and validate writes against this declaration.
use crate::CwpdbResult;
use rusqlite::{
    params_from_iter, types::Value as SqlValue, Connection, OptionalExtension, Transaction,
};
use serde_json::Value;

/// Constructed by the CWR declaration owner, never from a wire request.
#[derive(Clone, Debug)]
pub struct SequenceIndex {
    pub name: String,
    pub kind: String,
    pub schema: String,
    pub sequence: String,
    pub receipt: String,
    /// Legacy Desktop values nest fields under record; new declarations may be flat.
    pub record_envelope: bool,
    /// Exact equality prefix; define a separate index for each supported filter set.
    pub equalities: Vec<String>,
}
#[derive(Clone, Copy, Debug)]
pub enum Direction {
    Ascending,
    Descending,
}
#[derive(Clone, Copy, Debug)]
pub struct ReadBounds {
    pub rows: usize,
    pub bytes: usize,
    pub work: usize,
}
#[derive(Debug)]
pub struct SequenceRow {
    pub reference: String,
    pub sequence: i64,
    pub revision: crate::ref_read::RefRevision,
    pub value: Value,
}
#[derive(Debug, PartialEq, Eq)]
pub enum StopReason {
    Rows,
    Bytes,
    Work,
}
#[derive(Debug)]
pub struct SequencePage {
    pub items: Vec<SequenceRow>,
    /// Last examined position, including cutoff-filtered rows. Resume exclusively.
    pub next: Option<i64>,
    pub complete: bool,
    pub stopped_by: Option<StopReason>,
    /// Required envelope bytes when the next item cannot fit this page.
    pub required_bytes: Option<usize>,
    pub examined: usize,
    pub bytes: usize,
}
fn identifier(s: &str) -> CwpdbResult<()> {
    if s.is_empty() || s.len() > 80 || !s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Err("invalid declared index identifier".into());
    }
    Ok(())
}
fn literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}
impl SequenceIndex {
    fn field(&self, s: &str) -> String {
        let prefix = if self.record_envelope {
            "$.record."
        } else {
            "$."
        };
        format!("json_extract(n.props,'{prefix}{s}')")
    }
    fn validate(&self) -> CwpdbResult<()> {
        for s in [&self.name, &self.sequence, &self.receipt]
            .into_iter()
            .chain(self.equalities.iter())
        {
            identifier(s)?;
        }
        if self.equalities.len() > 3 || self.kind.is_empty() || self.schema.is_empty() {
            return Err("invalid sequence declaration".into());
        }
        Ok(())
    }
    /// Install centrally in the migration transaction before any consumer read.
    /// Sequence uniqueness is an owning append invariant, not inferred here.
    pub fn install(&self, tx: &Transaction<'_>) -> CwpdbResult<()> {
        self.validate()?;
        let mut columns = self
            .equalities
            .iter()
            .map(|s| self.field(s).replace("n.props", "props"))
            .collect::<Vec<_>>();
        columns.insert(0, "schema_id".into());
        columns.push(self.field(&self.sequence).replace("n.props", "props"));
        let sql = format!(
            "CREATE INDEX {} ON nodes({}) WHERE kind={}",
            self.name,
            columns.join(","),
            literal(&self.kind)
        );
        let existing: Option<String> = tx
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='index' AND name=?",
                [&self.name],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(existing) = existing {
            if existing != sql {
                return Err("declared sequence index conflicts with retained schema".into());
            }
        } else {
            tx.execute_batch(&sql)?;
        }
        Ok(())
    }
    /// One connection snapshot (or the caller's existing write transaction).
    /// `cutoff` selects receipts strictly older than the timestamp.
    /// Fixed index prefix isolates unrelated kinds/equality groups; residual work
    /// is capped even when receipt filtering is sparse. Byte budget counts the
    /// serialized examined envelopes (including cutoff misses), plus one lookahead.
    pub fn read(
        &self,
        db: &Connection,
        equalities: &[SqlValue],
        direction: Direction,
        after: Option<i64>,
        cutoff: Option<i64>,
        bounds: ReadBounds,
    ) -> CwpdbResult<SequencePage> {
        self.validate()?;
        if equalities.len() != self.equalities.len()
            || bounds.rows == 0
            || bounds.rows > 250
            || bounds.bytes == 0
            || bounds.bytes > 1_048_576
            || bounds.work == 0
            || bounds.work > 10_000
        {
            return Err("invalid sequence read bounds or filters".into());
        }
        let (op, order, start) = match direction {
            Direction::Ascending => (">", "ASC", 0),
            Direction::Descending => (if after.is_some() { "<" } else { "<=" }, "DESC", i64::MAX),
        };
        if after.is_some_and(|v| v <= 0) {
            return Err("invalid sequence continuation".into());
        }
        let mut predicates = vec![
            format!("n.kind={}", literal(&self.kind)),
            "n.schema_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?)".into(),
        ];
        let mut args = vec![SqlValue::Text(self.schema.clone())];
        for (name, value) in self.equalities.iter().zip(equalities) {
            predicates.push(format!("{} IS ?", self.field(name)));
            args.push(value.clone());
        }
        predicates.push(format!("{} {op} ?", self.field(&self.sequence)));
        args.push(SqlValue::Integer(after.unwrap_or(start)));
        args.push(SqlValue::Integer((bounds.work + 1) as i64));
        let sql = format!("SELECT r.canonical_ref,{},json(n.props),{},n.revision FROM nodes n INDEXED BY {} JOIN ref_bindings r ON r.ref_id=n.ref_id WHERE {} ORDER BY {} {order} LIMIT ?", self.field(&self.sequence),self.field(&self.receipt), self.name,predicates.join(" AND "),self.field(&self.sequence));
        let mut statement = db.prepare(&sql)?;
        let mut rows = statement.query(params_from_iter(args))?;
        let mut page = SequencePage {
            items: vec![],
            next: after,
            complete: true,
            stopped_by: None,
            required_bytes: None,
            examined: 0,
            bytes: 0,
        };
        while let Some(row) = rows.next()? {
            if page.examined == bounds.work {
                page.stopped_by = Some(StopReason::Work);
                page.complete = false;
                break;
            }
            let sequence: i64 = row.get(1)?;
            if sequence <= 0 {
                return Err("invalid retained sequence".into());
            }
            let json: String = row.get(2)?;
            let receipt: i64 = row.get(3)?;
            if json.len() > 1_048_576 {
                return Err("retained record exceeds read safety limit".into());
            }
            let selected = cutoff.is_none_or(|cutoff| receipt < cutoff);
            if selected && page.items.len() == bounds.rows {
                page.stopped_by = Some(StopReason::Rows);
                page.complete = false;
                break;
            }
            if json.len() > bounds.bytes - page.bytes {
                page.stopped_by = Some(StopReason::Bytes);
                page.required_bytes = Some(json.len());
                page.complete = false;
                break;
            }
            // Charge every examined envelope, including cutoff-filtered rows.
            // One unconsumed lookahead can be hydrated to establish a stop.
            page.bytes += json.len();
            if selected {
                page.items.push(SequenceRow {
                    reference: row.get(0)?,
                    revision: crate::ref_read::RefRevision::from_storage_counter(row.get(4)?),
                    sequence,
                    value: serde_json::from_str(&json)?,
                });
            }
            page.examined += 1;
            page.next = Some(sequence);
        }
        if page.complete {
            page.next = None;
        }
        Ok(page)
    }
}
#[cfg(test)]
#[path = "sequence_read_tests.rs"]
mod tests;

/// Inclusive ledger accounting, including quarantined rows. Bounded O(work)
/// across this declaration's schema; unrelated schemas use another index range.
/// High-water here is only the maximum LIVE sequence, never migration authority.
#[derive(Debug, PartialEq, Eq)]
pub struct SequenceAccounting {
    pub rows: usize,
    pub bytes: u64,
    pub max_live_sequence: i64,
    pub complete: bool,
}
impl SequenceIndex {
    pub fn account(
        &self,
        db: &Connection,
        bytes_field: &str,
        work: usize,
    ) -> CwpdbResult<SequenceAccounting> {
        self.validate()?;
        identifier(bytes_field)?;
        if work == 0 || work > 10_000 {
            return Err("invalid accounting work bound".into());
        }
        let sql = format!("SELECT {},{},n.kind FROM nodes n INDEXED BY nodes_by_schema WHERE n.schema_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?) LIMIT ?", self.field(bytes_field), self.field(&self.sequence));
        let mut statement = db.prepare(&sql)?;
        let mut rows = statement.query(rusqlite::params![self.schema, (work + 1) as i64])?;
        let mut result = SequenceAccounting {
            rows: 0,
            bytes: 0,
            max_live_sequence: 0,
            complete: true,
        };
        while let Some(row) = rows.next()? {
            if result.rows == work {
                result.complete = false;
                break;
            }
            let retained_kind: String = row.get(2)?;
            if retained_kind != self.kind {
                return Err("retained accounting schema/kind conflict".into());
            }
            let bytes: i64 = row.get(0)?;
            let sequence: i64 = row.get(1)?;
            if bytes < 0 || sequence <= 0 {
                return Err("invalid retained accounting value".into());
            }
            result.rows += 1;
            result.bytes = result
                .bytes
                .checked_add(bytes as u64)
                .ok_or("accounting overflow")?;
            result.max_live_sequence = result.max_live_sequence.max(sequence);
        }
        Ok(result)
    }
}
