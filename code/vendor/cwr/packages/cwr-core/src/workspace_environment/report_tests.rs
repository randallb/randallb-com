use super::*;
fn fixture_value(name: &str, kind: &str) -> cwpdb::environment::EntryValue {
    cwpdb::environment::EntryValue {
        name: name.into(),
        kind: kind.into(),
        value: (kind == "variable").then(|| "test".into()),
        credential_id: (kind == "secret").then(|| "00000000-0000-4000-8000-000000000001".into()),
    }
}
#[test]
fn report_reads_all_membership_pages_and_preserves_display_order_and_revisions() {
    let db = EnvironmentStore::in_memory("test-store").unwrap();
    let workspace = WorkspaceIdentity {
        storage_identity: "test-store".into(),
        reference: "cwp://com.example/workspaces/one".into(),
    };
    db.bind_workspace(&workspace.reference, "fixture").unwrap();
    let mut expected = Vec::new();
    for index in (0..260).rev() {
        let name = format!("VAR_{:03}", index / 2);
        let reference = format!("cwp://com.example/environment/{index:03}");
        db.fixture_seed_entry(
            &workspace.reference,
            &reference,
            &fixture_value(&name, "variable"),
        )
        .unwrap();
        expected.push((name, reference));
    }
    db.fixture_seed_entry(
        "cwp://com.example/workspaces/other",
        "cwp://com.example/environment/other",
        &fixture_value("EXCLUDED", "variable"),
    )
    .unwrap();
    db.fixture_seed_unrelated_entry(
        &workspace.reference,
        "cwp://com.example/environment/unrelated",
        &fixture_value("EXCLUDED", "variable"),
    )
    .unwrap();
    db.fixture_seed_missing_member(&workspace.reference, "cwp://com.example/external")
        .unwrap();
    db.fixture_seed_wrong_kind_member(&workspace.reference, "cwp://com.example/wrong")
        .unwrap();
    expected.sort();
    let report = storage::report(&db, &workspace, &MemoryStore::default()).unwrap();
    assert_eq!(report.entries.len(), 260);
    assert_eq!(report.storage_identity, "test-store");
    assert!(!report.cleanup_pending);
    assert_eq!(
        report
            .entries
            .iter()
            .map(|e| (e.name.clone(), e.reference.clone()))
            .collect::<Vec<_>>(),
        expected
    );
    for entry in report.entries {
        let stored = db
            .entry(&workspace.reference, &entry.reference)
            .unwrap()
            .unwrap();
        assert_eq!(entry.revision, stored.revision);
        assert_eq!(entry.status, EnvironmentEntryStatus::Available);
        assert!(entry.configured);
    }
}
#[test]
fn report_rejects_invalid_entry_values_and_preserves_cleanup_status() {
    let db = EnvironmentStore::in_memory("test-store").unwrap();
    let workspace = WorkspaceIdentity {
        storage_identity: "test-store".into(),
        reference: "cwp://com.example/workspaces/one".into(),
    };
    db.bind_workspace(&workspace.reference, "fixture").unwrap();
    db.reserve_cleanup(&workspace.reference, "credential")
        .unwrap();
    assert!(
        storage::report(&db, &workspace, &MemoryStore::default())
            .unwrap()
            .cleanup_pending
    );
    let reference = "cwp://com.example/environment/invalid";
    db.fixture_seed_entry(
        &workspace.reference,
        reference,
        &fixture_value("INVALID NAME", "variable"),
    )
    .unwrap();
    assert!(storage::report(&db, &workspace, &MemoryStore::default()).is_err());
    db.fixture_seed_entry(
        &workspace.reference,
        reference,
        &fixture_value("REPAIRED", "variable"),
    )
    .unwrap();
    assert_eq!(
        storage::report(&db, &workspace, &MemoryStore::default())
            .unwrap()
            .entries
            .len(),
        1
    );
}
#[test]
fn report_keeps_one_snapshot_when_a_writer_changes_later_pages() {
    struct WritingStore {
        writer: EnvironmentStore,
        workspace: String,
        changed: AtomicBool,
    }
    impl SecretStore for WritingStore {
        fn put(&self, _: &str, _: &str) -> Result<(), &'static str> {
            unreachable!()
        }
        fn get(&self, _: &str) -> Result<Option<String>, &'static str> {
            unreachable!()
        }
        fn remove(&self, _: &str) -> Result<(), &'static str> {
            unreachable!()
        }
        fn contains(&self, _: &str) -> Result<bool, &'static str> {
            if !self.changed.swap(true, Ordering::Relaxed) {
                for index in 0..130 {
                    self.writer
                        .fixture_seed_entry(
                            &self.workspace,
                            &format!("cwp://com.example/environment/{index}"),
                            &fixture_value("CHANGED", "secret"),
                        )
                        .unwrap();
                }
            }
            Ok(true)
        }
    }
    let root = tempfile::tempdir().unwrap();
    let db = EnvironmentStore::open(root.path(), "test-store").unwrap();
    let workspace = WorkspaceIdentity {
        storage_identity: "test-store".into(),
        reference: "cwp://com.example/workspaces/one".into(),
    };
    db.bind_workspace(&workspace.reference, "fixture").unwrap();
    for index in 0..130 {
        db.fixture_seed_entry(
            &workspace.reference,
            &format!("cwp://com.example/environment/{index}"),
            &fixture_value("ORIGINAL", "secret"),
        )
        .unwrap();
    }
    let store = WritingStore {
        writer: EnvironmentStore::open(root.path(), "test-store").unwrap(),
        workspace: workspace.reference.clone(),
        changed: AtomicBool::new(false),
    };
    let report = storage::report(&db, &workspace, &store).unwrap();
    assert_eq!(report.entries.len(), 130);
    assert!(store.changed.load(Ordering::Relaxed));
    assert!(report.entries.iter().all(|entry| entry.name == "ORIGINAL"));
    let fresh = storage::report(&db, &workspace, &store).unwrap();
    assert!(fresh.entries.iter().all(|entry| entry.name == "CHANGED"));
}
