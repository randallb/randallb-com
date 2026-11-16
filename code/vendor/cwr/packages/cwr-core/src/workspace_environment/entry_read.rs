use super::*;
#[derive(Clone, PartialEq, Eq)]
pub(super) struct StoredEntry {
    pub(super) revision: i64,
    pub(super) kind: EnvironmentEntryKind,
    pub(super) value: Option<String>,
    pub(super) credential_id: Option<String>,
}
pub(super) fn read_entry(
    db: &EnvironmentStore,
    workspace: &str,
    reference: &str,
) -> CwrResult<Option<StoredEntry>> {
    db.entry(workspace, reference)?
        .map(|entry| {
            let record = decode_record(entry.value)?;
            Ok(StoredEntry {
                revision: entry.revision,
                kind: record.kind,
                value: record.value,
                credential_id: record.credential_id,
            })
        })
        .transpose()
}
pub(super) fn decode_record(value: cwpdb::environment::EntryValue) -> CwrResult<EntryRecord> {
    let envelope = serde_json::json!({"record":&value});
    if value.kind == "variable" {
        declared_variable::validate(&envelope)?;
    } else if value.kind == "secret" {
        declared_secret::validate(&envelope)?;
    }
    let record: EntryRecord = serde_json::from_value(serde_json::to_value(value)?)?;
    validate_record(&record)?;
    Ok(record)
}
