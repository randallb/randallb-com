use super::*;
#[test]
fn declared_variable_named_commit_survives_reopen_and_rolls_back_late_failure() {
    with_state(|workspace, store, _| {
        let saved = save(
            workspace,
            store,
            "DECLARED",
            EnvironmentEntryKind::Variable,
            "fictional",
            None,
            None,
        )
        .unwrap();
        let entry = &saved.entries[0];
        let (db, identity) = open(workspace).unwrap();
        let reference = "cwp://com.example/variables/rolled-back";
        db.fixture_reject_membership_writes().unwrap();
        let next = cwpdb::environment::EntryValue {
            name: "ROLLBACK".into(),
            kind: "variable".into(),
            value: Some("v".into()),
            credential_id: None,
        };
        assert!(db
            .save(cwpdb::environment::Save {
                workspace: &identity.reference,
                reference,
                expected_revision: None,
                value: &next
            })
            .is_err());
        db.fixture_allow_membership_writes().unwrap();
        assert!(db.fixture_entry_record(reference).unwrap().is_none());
        assert!(read_entry(&db, &identity.reference, &entry.reference)
            .unwrap()
            .is_some());
        drop(db);
        assert_eq!(list(workspace, store).unwrap().entries, saved.entries);
        assert!(save(
            workspace,
            store,
            "DECLARED",
            EnvironmentEntryKind::Variable,
            "duplicate",
            None,
            None
        )
        .is_err());
    });
}
#[test]
fn bundled_variable_rejects_invalid_values() {
    for value in [
        json!({"name":"bad name","kind":"variable","value":"v"}),
        json!({"name":"VALID","kind":"secret","credential_id":"id"}),
        json!({"name":"VALID","kind":"variable","value":"a\u{0000}b"}),
        json!({"name":"VALID","kind":"variable","value":"v","unexpected":true}),
    ] {
        assert!(declared_variable::validate(&json!({"record":value})).is_err());
    }
}
#[test]
fn declared_variable_domain_lifecycle_preserves_revision_and_membership() {
    with_state(|workspace, store, _| {
        let first = save(
            workspace,
            store,
            "LIFECYCLE",
            EnvironmentEntryKind::Variable,
            "first",
            None,
            None,
        )
        .unwrap()
        .entries[0]
            .clone();
        let changed = save(
            workspace,
            store,
            "RENAMED",
            EnvironmentEntryKind::Variable,
            "second",
            Some(&first.reference),
            Some(first.revision),
        )
        .unwrap();
        let second = changed.entries[0].clone();
        assert!(second.revision > first.revision);
        assert_eq!(second.name, "RENAMED");
        assert_eq!(second.value.as_deref(), Some("second"));
        assert!(save(
            workspace,
            store,
            "RENAMED",
            EnvironmentEntryKind::Variable,
            "lost",
            Some(&first.reference),
            Some(first.revision)
        )
        .is_err());
        assert!(remove(workspace, store, &first.reference, first.revision).is_err());
        assert_eq!(list(workspace, store).unwrap().entries, changed.entries);
        let (db, _) = open(workspace).unwrap();
        db.fixture_reject_membership_writes().unwrap();
        assert!(save(
            workspace,
            store,
            "RENAMED",
            EnvironmentEntryKind::Variable,
            "rolled back",
            Some(&second.reference),
            Some(second.revision)
        )
        .is_err());
        db.fixture_allow_membership_writes().unwrap();
        assert_eq!(list(workspace, store).unwrap().entries, changed.entries);
        drop(db);
        assert!(remove(workspace, store, &second.reference, second.revision)
            .unwrap()
            .entries
            .is_empty());
        let (db, identity) = open(workspace).unwrap();
        assert!(read_entry(&db, &identity.reference, &second.reference)
            .unwrap()
            .is_none());
    });
}
#[test]
fn variable_list_and_mutations_reject_values_outside_the_declaration() {
    with_state(|workspace, store, _| {
        let saved = save(
            workspace,
            store,
            "SCHEMA",
            EnvironmentEntryKind::Variable,
            "original",
            None,
            None,
        )
        .unwrap();
        let entry = &saved.entries[0];
        let (db, identity) = open(workspace).unwrap();
        db.fixture_add_unknown_entry_property(&entry.reference)
            .unwrap();
        let invalid = db.fixture_entry_record(&entry.reference).unwrap().unwrap();
        assert!(read_entry(&db, &identity.reference, &entry.reference).is_err());
        assert!(list(workspace, store).is_err());
        assert!(save(
            workspace,
            store,
            "SCHEMA",
            EnvironmentEntryKind::Variable,
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
    });
}
