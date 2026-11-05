//! A trusted native read selection. Only finite public facts leave this module.
use super::*;
use rusqlite::OpenFlags;
use std::sync::{Arc, Mutex};

/// Prepared read-only storage. A finite selection checks out its connection;
/// dropping the selection releases the WAL snapshot and returns the connection.
pub struct NativeReadStore {
    pool: Arc<Mutex<Option<Connection>>>,
    scope: Scope,
    identity: Option<FileIdentity>,
}
struct ReadConnection {
    connection: Option<Connection>,
    pool: Arc<Mutex<Option<Connection>>>,
}
impl std::ops::Deref for ReadConnection {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.connection
            .as_ref()
            .expect("checked-out read connection")
    }
}
impl Drop for ReadConnection {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            let _ = connection.progress_handler(0, None::<fn() -> bool>);
            let _ = connection.execute_batch("ROLLBACK");
            if let Ok(mut available) = self.pool.lock() {
                *available = Some(connection);
            }
        }
    }
}

/// A frozen read-only native selection. Callers authorize access before opening
/// it. No SQL, connection, transaction, native ID, or callback is exposed.
pub struct NativeReadSnapshot {
    connection: ReadConnection,
    scope: Scope,
    control: ReadControl,
    commit_revision: String,
}
struct TrustedSelection;
impl Authorizer for TrustedSelection {
    fn authorize(&self, _: &Context, _: &str, write: bool) -> CwpdbResult<()> {
        if write {
            return Err("read selection cannot authorize writes".into());
        }
        Ok(())
    }
}
fn configure(connection: &Connection, control: &ReadControl) -> CwpdbResult<()> {
    control.check()?;
    connection.busy_timeout(std::time::Duration::from_millis(5))?;
    install_monitor(connection, control)?;
    Ok(())
}
fn install_monitor(connection: &Connection, control: &ReadControl) -> CwpdbResult<()> {
    let monitor = control.clone();
    connection.progress_handler(
        if monitor.vm_steps.is_some() { 1 } else { 1000 },
        Some(move || {
            if let Some(counter) = &monitor.vm_steps {
                counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            monitor.check().is_err()
        }),
    )?;
    Ok(())
}
struct SelectionDeadline<'a> {
    connection: &'a Connection,
    restore: &'a ReadControl,
}
impl Drop for SelectionDeadline<'_> {
    fn drop(&mut self) {
        let _ = install_monitor(self.connection, self.restore);
    }
}
fn read_locked(error: &(dyn std::error::Error + Send + Sync + 'static)) -> bool {
    matches!(error.downcast_ref::<rusqlite::Error>(), Some(rusqlite::Error::SqliteFailure(code, _)) if matches!(code.code, rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked))
}
impl NativeReadStore {
    pub fn open(path: &Path, runtime_root_ref: &str, control: &ReadControl) -> CwpdbResult<Self> {
        loop {
            control.check()?;
            match Self::open_once(path, runtime_root_ref, control) {
                Ok(store) => return Ok(store),
                Err(error) => {
                    control.check()?;
                    if !read_locked(error.as_ref()) {
                        return Err(error);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
        }
    }
    fn open_once(path: &Path, runtime_root_ref: &str, control: &ReadControl) -> CwpdbResult<Self> {
        control.check()?;
        valid_ref(runtime_root_ref)?;
        #[cfg(unix)]
        let identity = {
            use std::os::unix::fs::MetadataExt;
            let metadata = std::fs::metadata(path)?;
            Some(FileIdentity {
                path: path.into(),
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        };
        #[cfg(not(unix))]
        let identity = None;
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        configure(&connection, control)?;
        if !initialization::inspect_read_schema(&connection, path)? {
            return Err("cwpDb database must be initialized before it can serve reads".into());
        }
        connection.execute_batch(include_str!("read_snapshot/read_views.sql"))?;
        connection.pragma_update(None, "query_only", true)?;
        let runtime_count: i64 = connection.query_row(
            "SELECT COUNT(*) FROM nodes WHERE kind='runtime'",
            [],
            |row| row.get(0),
        )?;
        if runtime_count != 1 {
            return Err("ambiguous or missing CWR runtime identity".into());
        }
        let metadata: String = connection.query_row(
            "SELECT json(n.props) FROM nodes n JOIN ref_bindings r ON r.ref_id=n.ref_id WHERE n.kind='runtime' AND r.canonical_ref=?1",
            [format!("{runtime_root_ref}/cwpdb")],
            |row| row.get(0),
        )?;
        let metadata: Value = serde_json::from_str(&metadata)?;
        if metadata["schema_ref"] != crate::SCHEMA_REF
            || metadata["runtime_root_ref"] != runtime_root_ref
        {
            return Err("CWR read selection runtime identity mismatch".into());
        }
        let version = metadata["runtime_state_version"]
            .as_str()
            .ok_or("missing runtime state version")?;
        let state = "cwp://org.coworkerprotocol/cwpdb/schemas/runtime-state/2026-09";
        if version != state && version != format!("{state}/conversation-timeline-v1") {
            return Err("unsupported CWR runtime state version".into());
        }
        let explicit: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM nodes WHERE kind IN ('conversation_timeline_connection','conversation_timeline_item'))", [], |row| row.get(0))?;
        if explicit && version == state {
            return Err("retained conversation timelines require the explicit-timeline runtime state version".into());
        }
        control.check()?;
        connection.progress_handler(0, None::<fn() -> bool>)?;
        verify_identity(identity.as_ref())?;
        Ok(Self {
            identity,
            pool: Arc::new(Mutex::new(Some(connection))),
            scope: Scope {
                runtime_root_ref: runtime_root_ref.into(),
                context: Context {
                    workspace_ref: metadata["cloud_workspace_ref"]
                        .as_str()
                        .unwrap_or(runtime_root_ref)
                        .into(),
                    caller_ref: runtime_root_ref.into(),
                },
            },
        })
    }
    pub fn begin_current(&mut self, control: ReadControl) -> CwpdbResult<NativeReadSnapshot> {
        self.begin_selection(None, control)
    }
    pub fn begin_at_revision(
        &mut self,
        expected_revision: &str,
        control: ReadControl,
    ) -> CwpdbResult<NativeReadSnapshot> {
        self.begin_selection(Some(expected_revision), control)
    }
    fn begin_selection(
        &mut self,
        expected_revision: Option<&str>,
        control: ReadControl,
    ) -> CwpdbResult<NativeReadSnapshot> {
        control.check()?;
        verify_identity(self.identity.as_ref())?;
        let connection = self
            .pool
            .lock()
            .map_err(|_| "read connection pool poisoned")?
            .take()
            .ok_or("a read selection is already active")?;
        let connection = ReadConnection {
            connection: Some(connection),
            pool: self.pool.clone(),
        };
        let revision = loop {
            control.check()?;
            configure(&connection, &control)?;
            let selected = (|| -> CwpdbResult<String> {
                connection.execute_batch("BEGIN DEFERRED")?;
                let metadata = runtime_metadata(&connection, &self.scope)?;
                if metadata["runtime_root_ref"] != self.scope.runtime_root_ref {
                    return Err("CWR read selection runtime identity mismatch".into());
                }
                let revision = metadata["commit_revision"]
                    .as_i64()
                    .filter(|n| *n >= 0)
                    .ok_or("invalid CWR commit revision")?
                    .to_string();
                if expected_revision.is_some_and(|expected| revision != expected) {
                    return Err("conversation read snapshot revision conflict".into());
                }
                control.check()?;
                Ok(revision)
            })();
            match selected {
                Ok(revision) => break revision,
                Err(error) => {
                    // Clear cancellation before rollback: an interrupted rollback
                    // must not leave a transaction checked out of this pool.
                    connection.progress_handler(0, None::<fn() -> bool>)?;
                    connection.execute_batch("ROLLBACK")?;
                    control.check()?;
                    if !read_locked(error.as_ref()) {
                        return Err(error);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
        };
        verify_identity(self.identity.as_ref())?;
        Ok(NativeReadSnapshot {
            connection,
            scope: self.scope.clone(),
            control,
            commit_revision: revision,
        })
    }
}
impl NativeReadSnapshot {
    pub fn open_at_revision(
        path: &Path,
        runtime_root_ref: &str,
        expected_revision: &str,
        control: ReadControl,
    ) -> CwpdbResult<Self> {
        NativeReadStore::open(path, runtime_root_ref, &control)?
            .begin_at_revision(expected_revision, control)
    }
    pub fn runtime_root_ref(&self) -> &str {
        &self.scope.runtime_root_ref
    }
    pub fn commit_revision(&self) -> &str {
        &self.commit_revision
    }

    pub fn resource(&self, reference: &str) -> CwpdbResult<Option<ResourceFacts>> {
        self.control.check()?;
        valid_ref(reference)?;
        let result = resource::snapshot_resource(
            &self.connection,
            &self.scope,
            reference,
            &TrustedSelection,
        )?;
        self.control.check()?;
        Ok(result)
    }

    /// Operational invocation detail is independent of publication visibility.
    /// Inputs, responses and provider history remain excluded by the facts port.
    pub fn invocation(
        &self,
        conversation: &str,
        reference: &str,
    ) -> CwpdbResult<Option<ResourceRecordFacts>> {
        self.control.check()?;
        valid_ref(conversation)?;
        valid_ref(reference)?;
        let node = read::get(&self.connection, reference)?;
        let Some(node) = node.filter(|node| {
            node.kind == RecordKind::Invocation
                && node.properties["record"]["conversation_ref"] == conversation
        }) else {
            return Ok(None);
        };
        if node.schema_ref != crate::schema_ref(RecordKind::Invocation.as_str()) {
            return Err("unsupported invocation schema".into());
        }
        let result = resource::invocation::facts(&self.connection, conversation, &node, None)?;
        if serde_json::to_vec(&result)?.len() > MAX_WIRE_BYTES {
            return Err("invocation facts exceed wire limit".into());
        }
        self.control.check()?;
        Ok(Some(result))
    }

    /// Every nonterminal invocation, including work not placed on a timeline.
    pub fn current(&self, conversation: &str) -> CwpdbResult<Vec<ResourceRecordFacts>> {
        self.control.check()?;
        valid_ref(conversation)?;
        let mut statement = self.connection.prepare("SELECT r.canonical_ref FROM edges e INDEXED BY edges_in JOIN nodes n ON n.ref_id=e.source_id AND n.kind='invocation' JOIN ref_bindings r ON r.ref_id=n.ref_id WHERE e.target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind='field:conversation_ref' AND json_extract(n.props,'$.record.conversation_ref')=?1 AND json_extract(n.props,'$.record.state') IN ('admitted','awaiting_person','running','waiting','stopping') ORDER BY json_extract(n.props,'$.record.admitted_at_unix_ms'),r.canonical_ref LIMIT 50001")?;
        let references = statement
            .query_map([conversation], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        if references.len() > 50000 {
            return Err("current invocation work limit exceeded".into());
        }
        let mut result = Vec::new();
        let mut bytes = 2;
        for reference in references {
            let facts = self
                .invocation(conversation, &reference)?
                .ok_or("active invocation disappeared from snapshot")?;
            bytes += serde_json::to_vec(&facts)?.len() + 1;
            if bytes > MAX_WIRE_BYTES {
                return Err("current invocation facts exceed wire limit".into());
            }
            result.push(facts);
        }
        self.control.check()?;
        Ok(result)
    }

    pub fn conversation_exists(
        &self,
        conversation: &str,
        include_execution: bool,
        owner: Option<(&str, bool)>,
    ) -> CwpdbResult<bool> {
        self.control.check()?;
        valid_ref(conversation)?;
        let Some(facts) =
            resource::conversation_facts(&self.connection, &self.scope, conversation)?
        else {
            return Ok(false);
        };
        let visible = (include_execution || facts.parent_ref.is_none())
            && owner.is_none_or(|(owner, legacy)| match facts.owner_context_ref.as_deref() {
                Some(actual) => !owner.is_empty() && actual == owner,
                None => legacy,
            });
        self.control.check()?;
        Ok(visible)
    }
}

mod message_metadata;
mod publication;
mod summaries;

mod timeline;

mod inspector;

mod child_role;
mod targeting;

mod selection;

mod history;

#[cfg(feature = "test-support")]
mod cancellation_probe;

mod indexed;
mod legacy;

mod owner;
pub(super) use owner::owner_selection;

fn verify_identity(identity: Option<&FileIdentity>) -> CwpdbResult<()> {
    #[cfg(unix)]
    if let Some(identity) = identity {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::metadata(&identity.path)?;
        if !metadata.is_file()
            || metadata.dev() != identity.device
            || metadata.ino() != identity.inode
        {
            return Err("CWR runtime database was replaced while open".into());
        }
    }
    #[cfg(not(unix))]
    let _ = identity;
    Ok(())
}

#[cfg(test)]
mod identity_tests;
