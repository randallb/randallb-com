use super::*;

#[test]
fn secret_status_distinguishes_missing_and_unavailable_keychain_items() {
    let _guard = ENVIRONMENT_LOCK.lock().unwrap();
    let _identity = TestIdentity::new();
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
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
    assert_eq!(saved.entries[0].status, EnvironmentEntryStatus::Stored);
    let (db, identity) = open(&workspace).unwrap();
    let entry = read_entry(&db, &identity.reference, &saved.entries[0].reference)
        .unwrap()
        .unwrap();
    drop(db);
    let id = entry.credential_id.unwrap();
    store.values.lock().unwrap().remove(&id);
    let missing = list(&workspace, &store).unwrap();
    assert_eq!(missing.entries[0].status, EnvironmentEntryStatus::Missing);
    assert!(!missing.entries[0].configured);
    store.values.lock().unwrap().insert(id, "value".into());
    store.fail_contains.store(true, Ordering::Relaxed);
    let unavailable = list(&workspace, &store).unwrap();
    assert_eq!(
        unavailable.entries[0].status,
        EnvironmentEntryStatus::Unavailable
    );
    assert!(!unavailable.entries[0].configured);
    if let Some(previous) = previous {
        env::set_var("WORKLOOP_STATE_HOME", previous);
    } else {
        env::remove_var("WORKLOOP_STATE_HOME");
    }
}

#[test]
fn secret_access_failure_gives_unlock_or_replace_guidance() {
    let _guard = ENVIRONMENT_LOCK.lock().unwrap();
    let _identity = TestIdentity::new();
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
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
    store.fail_get.store(true, Ordering::Relaxed);
    let error = resolve_with(
        &workspace,
        &store,
        &saved.entries[0].reference,
        saved.entries[0].revision,
    )
    .unwrap_err();
    assert!(error.to_string().contains("Unlock Keychain or replace it"));
    if let Some(previous) = previous {
        env::set_var("WORKLOOP_STATE_HOME", previous);
    } else {
        env::remove_var("WORKLOOP_STATE_HOME");
    }
}

#[test]
fn unknown_record_schemas_are_rejected_before_cleanup_or_mutation() {
    with_state(|workspace, store, _| {
        let saved = save(
            workspace,
            store,
            "VALUE",
            EnvironmentEntryKind::Variable,
            "before",
            None,
            None,
        )
        .unwrap();
        let entry = &saved.entries[0];
        let (mut db, identity) = open(workspace).unwrap();
        store.put("pending-test-credential", "keep-me").unwrap();
        add_cleanup(&mut db, &identity.reference, "pending-test-credential").unwrap();
        for reference in [
            cleanup_reference("pending-test-credential"),
            entry.reference.clone(),
            identity.reference.clone(),
        ] {
            let restore = db
                .fixture_corrupt(cwpdb::environment::fixtures::Corruption::FutureNodeSchema(
                    &reference,
                ))
                .unwrap();
            assert!(list(workspace, store).is_err());
            assert!(save(
                workspace,
                store,
                "OTHER",
                EnvironmentEntryKind::Variable,
                "new",
                None,
                None
            )
            .is_err());
            assert!(store.contains("pending-test-credential").unwrap());
            db.fixture_restore(restore).unwrap();
        }
        drop(db);
        assert!(list(workspace, store).is_ok());
        assert!(!store.contains("pending-test-credential").unwrap());
    });
}
#[test]
fn unknown_edge_schemas_and_identities_are_rejected_before_access_or_cleanup() {
    use cwpdb::environment::fixtures::{Corruption, Relationship};
    with_state(|workspace, store, _| {
        let saved = save(
            workspace,
            store,
            "VALUE",
            EnvironmentEntryKind::Variable,
            "before",
            None,
            None,
        )
        .unwrap();
        let entry = &saved.entries[0];
        let (mut db, identity) = open(workspace).unwrap();
        store.put("pending-test-credential", "keep-me").unwrap();
        add_cleanup(&mut db, &identity.reference, "pending-test-credential").unwrap();
        for relationship in [Relationship::Entry, Relationship::Cleanup] {
            for identity_corruption in [false, true] {
                let corruption = if identity_corruption {
                    Corruption::RelationshipIdentity {
                        workspace: &identity.reference,
                        relationship,
                    }
                } else {
                    Corruption::FutureRelationshipSchema {
                        workspace: &identity.reference,
                        relationship,
                    }
                };
                let restore = db.fixture_corrupt(corruption).unwrap();
                assert!(list(workspace, store).is_err());
                assert!(EnvironmentAccess::open(workspace, || false).is_err());
                assert!(resolve_with(workspace, store, &entry.reference, entry.revision).is_err());
                assert!(remove(workspace, store, &entry.reference, entry.revision).is_err());
                assert!(save(
                    workspace,
                    store,
                    "OTHER",
                    EnvironmentEntryKind::Variable,
                    "new",
                    None,
                    None
                )
                .is_err());
                assert!(store.contains("pending-test-credential").unwrap());
                assert_eq!(
                    db.fixture_entry_record(&entry.reference)
                        .unwrap()
                        .unwrap()
                        .0["record"]["value"],
                    "before"
                );
                db.fixture_restore(restore).unwrap();
            }
        }
        drop(db);
        assert!(list(workspace, store).is_ok());
        assert!(!store.contains("pending-test-credential").unwrap());
        assert_eq!(
            resolve_with(workspace, store, &entry.reference, entry.revision).unwrap(),
            "before"
        );
    });
}
