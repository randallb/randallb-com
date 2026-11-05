use super::*;
#[test]
fn declared_secret_lifecycle_keeps_values_outside_graph_and_cleans_credentials() {
    with_state(|workspace, store, _| {
        let first = save(
            workspace,
            store,
            "TOKEN",
            EnvironmentEntryKind::Secret,
            "fictional-first",
            None,
            None,
        )
        .unwrap()
        .entries[0]
            .clone();
        let (db, identity) = open(workspace).unwrap();
        let old = db
            .entry(&identity.reference, &first.reference)
            .unwrap()
            .unwrap();
        let old_id = old.value.credential_id.clone().unwrap();
        assert!(!serde_json::to_string(&old)
            .unwrap()
            .contains("fictional-first"));
        assert!(first.value.is_none());
        let changed = save(
            workspace,
            store,
            "RENAMED",
            EnvironmentEntryKind::Secret,
            "fictional-second",
            Some(&first.reference),
            Some(first.revision),
        )
        .unwrap();
        let second = &changed.entries[0];
        assert!(second.revision > first.revision);
        assert_eq!(second.name, "RENAMED");
        assert!(!store.contains(&old_id).unwrap());
        assert!(save(
            workspace,
            store,
            "RENAMED",
            EnvironmentEntryKind::Secret,
            "lost",
            Some(&first.reference),
            Some(first.revision)
        )
        .is_err());
        assert!(remove(workspace, store, &first.reference, first.revision).is_err());
        drop(db);
        assert_eq!(list(workspace, store).unwrap().entries, changed.entries);
        assert_eq!(
            resolve_with(workspace, store, &second.reference, second.revision).unwrap(),
            "fictional-second"
        );
        let (db, identity) = open(workspace).unwrap();
        let record = db
            .entry(&identity.reference, &second.reference)
            .unwrap()
            .unwrap();
        let id = record.value.credential_id.clone().unwrap();
        assert_ne!(id, old_id);
        assert!(!serde_json::to_string(&record)
            .unwrap()
            .contains("fictional-second"));
        db.fixture_reject_membership_writes().unwrap();
        assert!(save(
            workspace,
            store,
            "RENAMED",
            EnvironmentEntryKind::Secret,
            "rolled-back",
            Some(&second.reference),
            Some(second.revision)
        )
        .is_err());
        db.fixture_allow_membership_writes().unwrap();
        assert!(db
            .entry(&identity.reference, &second.reference)
            .unwrap()
            .is_some());
        assert!(store.contains(&id).unwrap());
        assert!(remove(workspace, store, &second.reference, second.revision)
            .unwrap()
            .entries
            .is_empty());
        assert!(!store.contains(&id).unwrap());
        assert!(list(workspace, store).unwrap().entries.is_empty());
        assert!(db
            .entry(&identity.reference, &second.reference)
            .unwrap()
            .is_none());
    });
}
#[test]
fn declared_secret_rejects_invalid_metadata() {
    let id = "00000000-0000-0000-0000-000000000001";
    for record in [
        json!({"name":"bad name","kind":"secret","credential_id":id}),
        json!({"name":"TOKEN","kind":"secret","credential_id":"bad"}),
        json!({"name":"TOKEN","kind":"variable","credential_id":id}),
        json!({"name":"TOKEN","kind":"secret","credential_id":id,"value":"forbidden"}),
        json!({"name":"TOKEN","kind":"secret","credential_id":id,"extra":true}),
    ] {
        assert!(declared_secret::validate(&json!({"record":record})).is_err());
    }
}
#[test]
fn declared_secret_reads_and_mutations_reject_unknown_stored_fields() {
    with_state(|workspace, store, _| {
        let saved = save(
            workspace,
            store,
            "SCHEMA",
            EnvironmentEntryKind::Secret,
            "fictional",
            None,
            None,
        )
        .unwrap();
        let entry = &saved.entries[0];
        let (db, identity) = open(workspace).unwrap();
        db.fixture_add_unknown_entry_property(&entry.reference)
            .unwrap();
        let invalid = db.fixture_entry_record(&entry.reference).unwrap().unwrap();
        let credentials = store.0.lock().unwrap().clone();
        assert!(read_entry(&db, &identity.reference, &entry.reference).is_err());
        assert!(list(workspace, store).is_err());
        assert!(save(
            workspace,
            store,
            "SCHEMA",
            EnvironmentEntryKind::Secret,
            "replacement",
            Some(&entry.reference),
            Some(invalid.1)
        )
        .is_err());
        assert!(remove(workspace, store, &entry.reference, invalid.1).is_err());
        assert_eq!(
            db.fixture_entry_record(&entry.reference).unwrap().unwrap(),
            invalid
        );
        assert_eq!(*store.0.lock().unwrap(), credentials);
    });
}
