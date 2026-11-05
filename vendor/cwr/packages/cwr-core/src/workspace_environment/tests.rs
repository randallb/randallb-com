use super::*;
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
    thread,
    time::Duration,
};

#[path = "report_tests.rs"]
mod report_tests;

#[path = "review_tests.rs"]
mod review_tests;

#[path = "declared_variable_tests.rs"]
mod declared_variable_tests;

#[path = "declared_secret_tests.rs"]
mod declared_secret_tests;

use super::TEST_ENVIRONMENT_LOCK as ENVIRONMENT_LOCK;

#[derive(Default)]
struct MemoryStore(Mutex<BTreeMap<String, String>>);

impl SecretStore for MemoryStore {
    fn put(&self, id: &str, value: &str) -> Result<(), &'static str> {
        self.0.lock().unwrap().insert(id.into(), value.into());
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Option<String>, &'static str> {
        Ok(self.0.lock().unwrap().get(id).cloned())
    }

    fn contains(&self, id: &str) -> Result<bool, &'static str> {
        Ok(self.0.lock().unwrap().contains_key(id))
    }

    fn remove(&self, id: &str) -> Result<(), &'static str> {
        self.0.lock().unwrap().remove(id);
        Ok(())
    }
}

#[derive(Default)]
struct FlakyStore {
    values: Mutex<BTreeMap<String, String>>,
    fail_put_after_write: AtomicBool,
    fail_remove: AtomicBool,
    fail_contains: AtomicBool,
    fail_get: AtomicBool,
}

impl SecretStore for FlakyStore {
    fn put(&self, id: &str, value: &str) -> Result<(), &'static str> {
        self.values.lock().unwrap().insert(id.into(), value.into());
        if self.fail_put_after_write.load(Ordering::Relaxed) {
            return Err("simulated uncertain credential write");
        }
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Option<String>, &'static str> {
        if self.fail_get.load(Ordering::Relaxed) {
            return Err("simulated locked credential store");
        }
        Ok(self.values.lock().unwrap().get(id).cloned())
    }

    fn contains(&self, id: &str) -> Result<bool, &'static str> {
        if self.fail_contains.load(Ordering::Relaxed) {
            return Err("simulated locked credential store");
        }
        Ok(self.values.lock().unwrap().contains_key(id))
    }

    fn remove(&self, id: &str) -> Result<(), &'static str> {
        if self.fail_remove.load(Ordering::Relaxed) {
            return Err("simulated locked credential store");
        }
        self.values.lock().unwrap().remove(id);
        Ok(())
    }
}

#[derive(Default)]
struct SavePause {
    state: Mutex<(bool, bool)>,
    changed: Condvar,
}

#[derive(Clone, Default)]
struct PausingStore {
    values: Arc<Mutex<BTreeMap<String, String>>>,
    pause: Arc<SavePause>,
}

impl PausingStore {
    fn wait_until_written(&self) {
        let mut state = self.pause.state.lock().unwrap();
        while !state.0 {
            state = self.pause.changed.wait(state).unwrap();
        }
    }

    fn resume(&self) {
        let mut state = self.pause.state.lock().unwrap();
        state.1 = true;
        self.pause.changed.notify_all();
    }
}

impl SecretStore for PausingStore {
    fn put(&self, id: &str, value: &str) -> Result<(), &'static str> {
        self.values.lock().unwrap().insert(id.into(), value.into());
        let mut state = self.pause.state.lock().unwrap();
        state.0 = true;
        self.pause.changed.notify_all();
        while !state.1 {
            state = self.pause.changed.wait(state).unwrap();
        }
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Option<String>, &'static str> {
        Ok(self.values.lock().unwrap().get(id).cloned())
    }

    fn contains(&self, id: &str) -> Result<bool, &'static str> {
        Ok(self.values.lock().unwrap().contains_key(id))
    }

    fn remove(&self, id: &str) -> Result<(), &'static str> {
        self.values.lock().unwrap().remove(id);
        Ok(())
    }
}

#[test]
fn recovery_waits_for_a_complete_secret_save() {
    let _guard = ENVIRONMENT_LOCK.lock().unwrap();
    let _identity = TestIdentity::new();
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let previous = env::var_os("WORKLOOP_STATE_HOME");
    env::set_var("WORKLOOP_STATE_HOME", root.path().join("state"));
    let store = PausingStore::default();
    let save_store = store.clone();
    let save_workspace = workspace.clone();
    let saving = thread::spawn(move || {
        save(
            &save_workspace,
            &save_store,
            "TOKEN",
            EnvironmentEntryKind::Secret,
            "value",
            None,
            None,
        )
    });
    store.wait_until_written();
    let list_store = store.clone();
    let list_workspace = workspace.clone();
    let (sent, received) = std::sync::mpsc::channel();
    let listing = thread::spawn(move || sent.send(list(&list_workspace, &list_store)).unwrap());
    assert!(received.recv_timeout(Duration::from_millis(100)).is_err());
    store.resume();
    let saved = saving.join().unwrap().unwrap();
    let listed = received
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    listing.join().unwrap();
    assert_eq!(listed.entries, saved.entries);
    assert!(listed.entries[0].configured);
    if let Some(previous) = previous {
        env::set_var("WORKLOOP_STATE_HOME", previous);
    } else {
        env::remove_var("WORKLOOP_STATE_HOME");
    }
}

#[test]
fn uncertain_keychain_write_keeps_cleanup_work_until_recovery() {
    let _guard = ENVIRONMENT_LOCK.lock().unwrap();
    let _identity = TestIdentity::new();
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let previous = env::var_os("WORKLOOP_STATE_HOME");
    env::set_var("WORKLOOP_STATE_HOME", root.path().join("state"));
    let store = FlakyStore::default();
    store.fail_put_after_write.store(true, Ordering::Relaxed);
    store.fail_remove.store(true, Ordering::Relaxed);
    assert!(save(
        &workspace,
        &store,
        "TOKEN",
        EnvironmentEntryKind::Secret,
        "value",
        None,
        None,
    )
    .is_err());
    assert_eq!(store.values.lock().unwrap().len(), 1);
    store.fail_put_after_write.store(false, Ordering::Relaxed);
    store.fail_remove.store(false, Ordering::Relaxed);
    let recovered = list(&workspace, &store).unwrap();
    assert!(recovered.entries.is_empty());
    assert!(!recovered.cleanup_pending);
    assert!(store.values.lock().unwrap().is_empty());
    if let Some(previous) = previous {
        env::set_var("WORKLOOP_STATE_HOME", previous);
    } else {
        env::remove_var("WORKLOOP_STATE_HOME");
    }
}

fn with_state<T>(run: impl FnOnce(&Path, &MemoryStore, &Path) -> T) -> T {
    let _guard = ENVIRONMENT_LOCK.lock().unwrap();
    let _identity = TestIdentity::new();
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let previous = env::var_os("WORKLOOP_STATE_HOME");
    env::set_var("WORKLOOP_STATE_HOME", root.path().join("state"));
    let result = run(&workspace, &MemoryStore::default(), root.path());
    if let Some(previous) = previous {
        env::set_var("WORKLOOP_STATE_HOME", previous);
    } else {
        env::remove_var("WORKLOOP_STATE_HOME");
    }
    result
}

#[test]
fn records_variables_and_secret_references_in_cwpdb_without_secret_values() {
    with_state(|workspace, store, _| {
        let variable = save(
            workspace,
            store,
            "FAMILY_TIMEZONE",
            EnvironmentEntryKind::Variable,
            "America/New_York",
            None,
            None,
        )
        .unwrap();
        let secret = save(
            workspace,
            store,
            "GROUPME_TOKEN",
            EnvironmentEntryKind::Secret,
            "recognizable-test-secret",
            None,
            None,
        )
        .unwrap();
        assert_eq!(secret.entries.len(), 2);
        assert_eq!(
            secret
                .entries
                .iter()
                .find(|entry| entry.name == "GROUPME_TOKEN")
                .unwrap()
                .value,
            None
        );
        let secret_entry = secret
            .entries
            .iter()
            .find(|entry| entry.name == "GROUPME_TOKEN")
            .unwrap();
        assert_eq!(
            resolve_with(
                workspace,
                store,
                &secret_entry.reference,
                secret_entry.revision
            )
            .unwrap(),
            "recognizable-test-secret"
        );
        let reopened = list(workspace, store).unwrap();
        assert_eq!(reopened.entries, secret.entries);
        let (db, _) = open(workspace).unwrap();
        assert!(!db
            .fixture_contains_secret("recognizable-test-secret")
            .unwrap());
        assert_eq!(
            variable.entries[0].value.as_deref(),
            Some("America/New_York")
        );
    });
}

#[test]
fn replacement_is_versioned_and_stale_work_cannot_resolve() {
    with_state(|workspace, store, _| {
        let first = save(
            workspace,
            store,
            "TOKEN",
            EnvironmentEntryKind::Secret,
            "first",
            None,
            None,
        )
        .unwrap();
        let entry = first.entries.first().unwrap();
        let old_reference = entry.reference.clone();
        let old_revision = entry.revision;
        let second = save(
            workspace,
            store,
            "TOKEN",
            EnvironmentEntryKind::Secret,
            "second",
            Some(&old_reference),
            Some(old_revision),
        )
        .unwrap();
        let current = second.entries.first().unwrap();
        assert!(current.revision > old_revision);
        assert!(resolve_with(workspace, store, &old_reference, old_revision).is_err());
        assert_eq!(
            resolve_with(workspace, store, &old_reference, current.revision).unwrap(),
            "second"
        );
        assert!(save(
            workspace,
            store,
            "TOKEN",
            EnvironmentEntryKind::Secret,
            "stale",
            Some(&old_reference),
            Some(old_revision),
        )
        .is_err());
    });
}

#[test]
fn remove_keeps_reference_binding_and_revokes_resolution() {
    with_state(|workspace, store, _| {
        let saved = save(
            workspace,
            store,
            "TOKEN",
            EnvironmentEntryKind::Secret,
            "value",
            None,
            None,
        )
        .unwrap();
        let entry = saved.entries.first().unwrap();
        let reference = entry.reference.clone();
        assert!(remove(workspace, store, &reference, entry.revision)
            .unwrap()
            .entries
            .is_empty());
        assert!(resolve_with(workspace, store, &reference, entry.revision).is_err());
        let (db, _) = open(workspace).unwrap();
        assert!(db.fixture_retired_entry_binding(&reference).unwrap());
    });
}

#[test]
fn removed_references_cannot_be_recreated() {
    with_state(|workspace, store, _| {
        let saved = save(
            workspace,
            store,
            "TOKEN",
            EnvironmentEntryKind::Secret,
            "value",
            None,
            None,
        )
        .unwrap();
        let removed = saved.entries.first().unwrap();
        remove(workspace, store, &removed.reference, removed.revision).unwrap();
        assert!(save(
            workspace,
            store,
            "TOKEN",
            EnvironmentEntryKind::Secret,
            "replacement",
            Some(&removed.reference),
            None,
        )
        .is_err());
        let recreated = save(
            workspace,
            store,
            "TOKEN",
            EnvironmentEntryKind::Secret,
            "replacement",
            None,
            None,
        )
        .unwrap();
        assert_ne!(recreated.entries[0].reference, removed.reference);
    });
}

#[test]
fn storage_identity_is_persisted_and_cross_train_reuse_is_rejected() {
    let _guard = ENVIRONMENT_LOCK.lock().unwrap();
    let _identity = TestIdentity::new();
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let previous_state = env::var_os("WORKLOOP_STATE_HOME");
    let previous_identity = env::var_os("WORKLOOP_STORAGE_IDENTITY");
    env::set_var("WORKLOOP_STATE_HOME", root.path().join("state"));
    env::set_var("WORKLOOP_STORAGE_IDENTITY", "alpha");
    save(
        &workspace,
        &MemoryStore::default(),
        "TIMEZONE",
        EnvironmentEntryKind::Variable,
        "America/New_York",
        None,
        None,
    )
    .unwrap();
    env::set_var("WORKLOOP_STORAGE_IDENTITY", "beta");
    let error = list(&workspace, &MemoryStore::default()).unwrap_err();
    assert!(error
        .to_string()
        .contains("different Workloop storage identity"));
    if let Some(previous) = previous_state {
        env::set_var("WORKLOOP_STATE_HOME", previous);
    } else {
        env::remove_var("WORKLOOP_STATE_HOME");
    }
    if let Some(previous) = previous_identity {
        env::set_var("WORKLOOP_STORAGE_IDENTITY", previous);
    } else {
        env::remove_var("WORKLOOP_STORAGE_IDENTITY");
    }
}

#[test]
fn interrupted_replacement_keeps_cleanup_work_and_recovers_later() {
    let _guard = ENVIRONMENT_LOCK.lock().unwrap();
    let _identity = TestIdentity::new();
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let previous = env::var_os("WORKLOOP_STATE_HOME");
    env::set_var("WORKLOOP_STATE_HOME", root.path().join("state"));
    let store = FlakyStore::default();
    let first = save(
        &workspace,
        &store,
        "TOKEN",
        EnvironmentEntryKind::Secret,
        "first",
        None,
        None,
    )
    .unwrap();
    let entry = first.entries.first().unwrap();
    store.fail_remove.store(true, Ordering::Relaxed);
    let replaced = save(
        &workspace,
        &store,
        "TOKEN",
        EnvironmentEntryKind::Secret,
        "second",
        Some(&entry.reference),
        Some(entry.revision),
    )
    .unwrap();
    assert!(replaced.cleanup_pending);
    assert_eq!(store.values.lock().unwrap().len(), 2);
    store.fail_remove.store(false, Ordering::Relaxed);
    assert!(!list(&workspace, &store).unwrap().cleanup_pending);
    assert_eq!(store.values.lock().unwrap().len(), 1);
    if let Some(previous) = previous {
        env::set_var("WORKLOOP_STATE_HOME", previous);
    } else {
        env::remove_var("WORKLOOP_STATE_HOME");
    }
}

#[test]
fn cleanup_retries_after_the_original_workspace_disappears() {
    let _guard = ENVIRONMENT_LOCK.lock().unwrap();
    let _identity = TestIdentity::new();
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    let recovery_workspace = root.path().join("recovery-workspace");
    fs::create_dir(&workspace).unwrap();
    fs::create_dir(&recovery_workspace).unwrap();
    let previous = env::var_os("WORKLOOP_STATE_HOME");
    env::set_var("WORKLOOP_STATE_HOME", root.path().join("state"));
    let store = FlakyStore::default();
    let saved = save(
        &workspace,
        &store,
        "TOKEN",
        EnvironmentEntryKind::Secret,
        "value",
        None,
        None,
    )
    .unwrap();
    let entry = saved.entries.first().unwrap();
    store.fail_remove.store(true, Ordering::Relaxed);
    let removed = remove(&workspace, &store, &entry.reference, entry.revision).unwrap();
    assert!(removed.cleanup_pending);
    assert_eq!(store.values.lock().unwrap().len(), 1);

    fs::remove_dir(&workspace).unwrap();
    store.fail_remove.store(false, Ordering::Relaxed);
    list(&recovery_workspace, &store).unwrap();
    assert!(store.values.lock().unwrap().is_empty());

    if let Some(previous) = previous {
        env::set_var("WORKLOOP_STATE_HOME", previous);
    } else {
        env::remove_var("WORKLOOP_STATE_HOME");
    }
}

#[cfg(target_os = "macos")]
#[path = "native_tests.rs"]
mod native_tests;

struct TestIdentity(Option<std::ffi::OsString>);
impl TestIdentity {
    fn new() -> Self {
        let old = env::var_os("WORKLOOP_STORAGE_IDENTITY");
        env::set_var("WORKLOOP_STORAGE_IDENTITY", "test");
        Self(old)
    }
}
impl Drop for TestIdentity {
    fn drop(&mut self) {
        if let Some(v) = &self.0 {
            env::set_var("WORKLOOP_STORAGE_IDENTITY", v);
        } else {
            env::remove_var("WORKLOOP_STORAGE_IDENTITY");
        }
    }
}

#[path = "upgrade_tests.rs"]
mod upgrade_tests;
