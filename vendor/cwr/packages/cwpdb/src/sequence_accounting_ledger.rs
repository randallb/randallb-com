//! Persistent graph accounting joined to owning ledger mutations. No SQL table.
use crate::declared::EntityBinding as EntityCatalog;
use crate::CwpdbResult;
use rusqlite::Transaction;
use serde_json::Value;
#[derive(Debug, serde::Serialize, PartialEq, Eq)]
pub struct Accounting {
    pub rows: u64,
    pub bytes: u64,
}
pub struct LedgerAccounting<'a> {
    pub catalog: &'a EntityCatalog,
    pub definition: &'a str,
    pub reference: &'a str,
    pub record_envelope: bool,
    pub rows_field: &'a str,
    pub bytes_field: &'a str,
}
impl LedgerAccounting<'_> {
    fn fields<'a>(
        &self,
        value: &'a mut Value,
    ) -> CwpdbResult<&'a mut serde_json::Map<String, Value>> {
        let selected = if self.record_envelope {
            &mut value["record"]
        } else {
            value
        };
        selected
            .as_object_mut()
            .ok_or_else(|| "invalid accounting envelope".into())
    }
    pub fn read(&self, tx: &Transaction<'_>) -> CwpdbResult<Accounting> {
        let mut value = self
            .catalog
            .get_in_connection(tx, self.definition, self.reference)?
            .value
            .properties;
        let fields = self.fields(&mut value)?;
        Ok(Accounting {
            rows: fields
                .get(self.rows_field)
                .and_then(Value::as_u64)
                .ok_or("missing ledger row accounting")?,
            bytes: fields
                .get(self.bytes_field)
                .and_then(Value::as_u64)
                .ok_or("missing ledger byte accounting")?,
        })
    }
    /// Migration supplies complete authoritative totals including quarantined
    /// records. Existing totals must agree; never label a partial scan complete.
    pub fn initialize(&self, tx: &Transaction<'_>, rows: u64, bytes: u64) -> CwpdbResult<()> {
        let current = self
            .catalog
            .get_in_connection(tx, self.definition, self.reference)?;
        let mut value = current.value.properties.clone();
        let fields = self.fields(&mut value)?;
        match (fields.get(self.rows_field), fields.get(self.bytes_field)) {
            (Some(a), Some(b)) if a.as_u64() == Some(rows) && b.as_u64() == Some(bytes) => {
                return Ok(())
            }
            (None, None) => {}
            _ => return Err("retained ledger accounting mismatch".into()),
        }
        fields.insert(self.rows_field.into(), Value::from(rows));
        fields.insert(self.bytes_field.into(), Value::from(bytes));
        self.catalog.replace_in_transaction(
            tx,
            self.definition,
            self.reference,
            &current.revision,
            &value,
        )?;
        Ok(())
    }
    /// Call once for created/purged records in the same transaction, never for
    /// duplicates or quarantine-only updates. Failure rolls back the mutation.
    pub fn adjust(&self, tx: &Transaction<'_>, rows: i64, bytes: i64) -> CwpdbResult<Accounting> {
        let before = self.read(tx)?;
        let next = Accounting {
            rows: before
                .rows
                .checked_add_signed(rows)
                .ok_or("ledger row accounting overflow")?,
            bytes: before
                .bytes
                .checked_add_signed(bytes)
                .ok_or("ledger byte accounting overflow")?,
        };
        if rows == 0 && bytes == 0 {
            return Ok(next);
        }
        let current = self
            .catalog
            .get_in_connection(tx, self.definition, self.reference)?;
        let mut value = current.value.properties.clone();
        let fields = self.fields(&mut value)?;
        fields.insert(self.rows_field.into(), Value::from(next.rows));
        fields.insert(self.bytes_field.into(), Value::from(next.bytes));
        self.catalog.replace_in_transaction(
            tx,
            self.definition,
            self.reference,
            &current.revision,
            &value,
        )?;
        Ok(next)
    }
}
