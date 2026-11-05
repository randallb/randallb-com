use super::*;
use rusqlite::{OpenFlags, Transaction, TransactionBehavior};
use std::time::Duration;
#[path = "physical.rs"]
mod physical;
mod preflight;
const STATE: &str = "cwp://org.coworkerprotocol/cwpdb/schemas/runtime-state/2026-09";
const TIMELINE_STATE: &str =
    "cwp://org.coworkerprotocol/cwpdb/schemas/runtime-state/2026-09/conversation-timeline-v1";

pub(super) fn open(path: &Path, scope: &Scope, options: &OpenOptions) -> CwpdbResult<Connection> {
    preflight::options(options)?;
    if path.to_str().is_none() {
        return Err("CWR runtime database path must be valid UTF-8".into());
    }
    valid_ref(&scope.runtime_root_ref)?;
    valid_ref(&scope.context.workspace_ref)?;
    if path.try_exists()? {
        let reader = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        crate::profiling::trace(&reader);
        if matches!(
            physical::inspect_physical_schema(&reader, path)?,
            physical::DatabaseSchema::Current
        ) {
            let snapshot = reader.unchecked_transaction()?;
            preflight::needed(&snapshot, scope, options)?;
            snapshot.commit()?;
        }
    }
    let parent = path.parent().ok_or("runtime database needs a parent")?;
    std::fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    let mut connection = Connection::open(path)?;
    crate::profiling::trace(&connection);
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "recursive_triggers", "ON")?;
    let wal: String = connection.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))?;
    if !wal.eq_ignore_ascii_case("wal") {
        return Err("SQLite did not enable WAL for CWR".into());
    }
    connection.pragma_update(None, "synchronous", "FULL")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    let needs_write = {
        let snapshot = connection.transaction()?;
        let needed = match physical::inspect_physical_schema(&snapshot, path)? {
            physical::DatabaseSchema::New => true,
            physical::DatabaseSchema::Current => preflight::needed(&snapshot, scope, options)?,
        };
        snapshot.commit()?;
        needed
    };
    if needs_write {
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        match physical::inspect_physical_schema(&tx, path)? {
            physical::DatabaseSchema::New => {
                crate::initialize(&tx)?;
                initialize(&tx, scope, options)?;
            }
            physical::DatabaseSchema::Current => {
                if preflight::needed(&tx, scope, options)? {
                    initialize(&tx, scope, options)?;
                }
            }
        }
        tx.commit()?;
    }
    Ok(connection)
}
fn validate_identity(db: &Connection, scope: &Scope, options: &OpenOptions) -> CwpdbResult<()> {
    let count: i64 = db.query_row(
        "SELECT count(*) FROM nodes WHERE kind='runtime'",
        [],
        |row| row.get(0),
    )?;
    if count != 1 {
        return Err("ambiguous or missing CWR runtime identity".into());
    }
    let metadata = runtime_metadata(db, scope)?;
    if metadata["schema_ref"] != crate::SCHEMA_REF {
        return Err("unsupported cwpDb schema URL".into());
    }
    if metadata["runtime_root_ref"] != scope.runtime_root_ref {
        return Err("runtime database belongs to another runtime".into());
    }
    if metadata["runtime_state_version"] != STATE
        && metadata["runtime_state_version"] != TIMELINE_STATE
    {
        return Err("unsupported CWR runtime state version".into());
    }
    let explicit:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM nodes WHERE kind IN ('conversation_timeline_connection','conversation_timeline_item'))",[],|row|row.get(0))?;
    if explicit && metadata["runtime_state_version"] != TIMELINE_STATE {
        return Err(
            "retained conversation timelines require explicit-timeline runtime state version"
                .into(),
        );
    }
    if metadata
        .get("cloud_workspace_ref")
        .is_some_and(|value| value != &scope.context.workspace_ref)
    {
        return Err("runtime database belongs to another workspace".into());
    }
    let stage = metadata["conversation_timeline_rollout"]
        .as_str()
        .unwrap_or("disabled");
    if !["disabled", "new_conversations", "migrate_legacy"].contains(&stage) {
        return Err("unsupported retained timeline rollout".into());
    }
    if stage != "disabled" && options.timeline_rollout.is_none() {
        return Err("activated timeline database requires an explicitly configured writer".into());
    }
    Ok(())
}
pub(super) fn initialize(
    tx: &Transaction<'_>,
    scope: &Scope,
    options: &OpenOptions,
) -> CwpdbResult<()> {
    preflight::options(options)?;
    let count: i64 = tx.query_row(
        "SELECT count(*) FROM nodes WHERE kind='runtime'",
        [],
        |row| row.get(0),
    )?;
    if count == 0 {
        let any: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM nodes)", [], |row| row.get(0))?;
        if any {
            return Err("graph database has no CWR runtime identity".into());
        }
        crate::put_node(
            tx,
            &root_ref(scope),
            "runtime",
            &json!({"schema_ref":crate::SCHEMA_REF,"runtime_state_version":options.runtime_state_version,"runtime_root_ref":scope.runtime_root_ref,"cloud_workspace_ref":scope.context.workspace_ref,"commit_revision":0,"timeline_projection_version":1}),
        )?;
    } else {
        if count != 1 {
            return Err("ambiguous CWR runtime identity".into());
        }
        validate_identity(tx, scope, options)?;
        tx.execute("UPDATE nodes SET props=jsonb_set(props,'$.cloud_workspace_ref',?1) WHERE kind='runtime' AND json_type(props,'$.cloud_workspace_ref') IS NULL",[&scope.context.workspace_ref])?;
    }
    if let Some(binding) = &options.workspace_binding {
        bind_workspace(tx, scope, binding)?;
    }
    if let Some(stage) = &options.timeline_rollout {
        if configure_rollout(tx, scope, stage)? {
            tx.execute("UPDATE nodes SET props=jsonb_set(props,'$.commit_revision',json_extract(props,'$.commit_revision')+1) WHERE kind='runtime'",[])?;
        }
    }
    // A former fingerprint location is rewritten without changing admission wire
    // identity, history, ordering, or its source comparison value.
    tx.execute("UPDATE nodes SET props=jsonb_set(jsonb_remove(props,'$.record.source_input_fingerprint'),'$.source_input_fingerprint',json_extract(props,'$.record.source_input_fingerprint')),revision=revision+1 WHERE kind='admission' AND json_type(props,'$.record.source_input_fingerprint')='text'",[])?;
    // This legacy field was accepted by the protocol decoder but omitted from
    // normalized WorkUpdate values. Repair its retained projection before the
    // first snapshot so pure CWR deltas cannot overlook the physical drift.
    let normalized = tx.execute("UPDATE edges SET props=jsonb_remove(props,'$.record.event.attachment'),revision=revision+1 WHERE kind='timeline_activity' AND json_extract(props,'$.record.event.kind')='work_update' AND json_type(props,'$.record.event.attachment') IS NOT NULL AND EXISTS(SELECT 1 FROM edges owned WHERE owned.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND owned.kind='conversation' AND owned.target_id=edges.source_id)", [root_ref(scope)])?;
    if normalized > 0 {
        tx.execute("UPDATE nodes SET props=jsonb_set(props,'$.commit_revision',json_extract(props,'$.commit_revision')+1) WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)", [root_ref(scope)])?;
    }
    Ok(())
}
fn bind_workspace(
    tx: &Transaction<'_>,
    scope: &Scope,
    binding: &WorkspaceBinding,
) -> CwpdbResult<()> {
    if !preflight::workspace(tx, scope, binding)? {
        return Ok(());
    }
    let reference = format!("{}/workspace", root_ref(scope));
    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStrExt;
        binding.workspace_directory.as_os_str().as_bytes().to_vec()
    };
    #[cfg(not(unix))]
    let bytes = binding
        .workspace_directory
        .to_string_lossy()
        .as_bytes()
        .to_vec();
    crate::put_node(
        tx,
        &reference,
        "workspace",
        &json!({"workspace_id":binding.workspace_id,"workspace_path":bytes}),
    )?;
    Ok(())
}

pub(super) fn configure_rollout(
    tx: &Transaction<'_>,
    scope: &Scope,
    stage: &str,
) -> CwpdbResult<bool> {
    if !preflight::rollout(tx, scope, stage)? {
        return Ok(false);
    }
    tx.execute("UPDATE nodes SET props=jsonb_set(props,'$.conversation_timeline_rollout',?2) WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)",rusqlite::params![root_ref(scope),stage])?;
    Ok(true)
}

#[cfg(test)]
mod tests;

/// Read-only schema validation for a frozen finite native selection.
pub(super) fn inspect_read_schema(connection: &Connection, path: &Path) -> CwpdbResult<bool> {
    Ok(matches!(
        physical::inspect_physical_schema(connection, path)?,
        physical::DatabaseSchema::Current
    ))
}
