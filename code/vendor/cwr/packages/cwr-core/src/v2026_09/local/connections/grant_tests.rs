use super::*;

fn selections(report: &ProjectConnections) -> BTreeMap<String, String> {
    report
        .requirements
        .iter()
        .filter_map(|r| r.key_id.as_ref().map(|id| (r.alias.clone(), id.clone())))
        .collect()
}

#[test]
fn new_secret_assignment_requires_its_native_item_and_preserves_grants_on_failure() {
    let _serial = environment::TEST_ENVIRONMENT_LOCK.lock().unwrap();
    let root = fixture();
    let _env = EnvironmentGuard::isolate(root.path());
    let store = setup(root.path());
    let path = root.path().join("CWR.toml");
    let before = inspect_project_connections(&path).unwrap();
    let assignments = selections(&before);
    let metadata = EnvironmentAccess::open(root.path(), || false)
        .unwrap()
        .metadata(&assignments["research"])
        .unwrap();
    let id = metadata.credential_id.unwrap();
    let revoked = save_project_connections(
        &path,
        &before.fingerprint,
        &before.grants_revision,
        BTreeMap::new(),
    )
    .unwrap();
    store.remove(&id).unwrap();
    for unavailable in [false, true] {
        let mut probes = 0;
        let error = mutation::save_with(
            &path,
            &revoked.fingerprint,
            &revoked.grants_revision,
            assignments.clone(),
            |selected| {
                probes += 1;
                assert_eq!(selected, id);
                if unavailable {
                    Err("native store unavailable".into())
                } else {
                    Ok(store.contains(selected)?)
                }
            },
        )
        .err()
        .expect("missing or unavailable secrets cannot be granted");
        assert_eq!(probes, 1);
        assert!(error.to_string().contains("Unlock Keychain or replace it"));
        let after = inspect_project_connections(&path).unwrap();
        assert_eq!(after.grants_revision, revoked.grants_revision);
        assert!(after.requirements.iter().all(|r| r.key_id.is_none()));
    }
    store.put(&id, SECRET).unwrap();
    let approved = save_project_connections(
        &path,
        &revoked.fingerprint,
        &revoked.grants_revision,
        assignments,
    )
    .unwrap();
    assert!(approved.requirements.iter().all(|r| r.configured));
}

#[test]
fn secret_probe_rechecks_replacement_and_revocation_before_committing() {
    let _serial = environment::TEST_ENVIRONMENT_LOCK.lock().unwrap();
    for replace in [false, true] {
        let root = fixture();
        let _env = EnvironmentGuard::isolate(root.path());
        let store = setup(root.path());
        let path = root.path().join("CWR.toml");
        let before = inspect_project_connections(&path).unwrap();
        let assignments = selections(&before);
        let entry = environment::entries(root.path())
            .unwrap()
            .into_iter()
            .find(|e| e.kind == EnvironmentEntryKind::Secret)
            .unwrap();
        let mut retained = assignments.clone();
        retained.remove("research");
        let revoked = save_project_connections(
            &path,
            &before.fingerprint,
            &before.grants_revision,
            retained.clone(),
        )
        .unwrap();
        let mut latest_revision = revoked.grants_revision.clone();
        let mut probes = 0;
        let result = mutation::save_with(
            &path,
            &revoked.fingerprint,
            &revoked.grants_revision,
            assignments.clone(),
            |_| {
                probes += 1;
                // Both operations need the same locks as approval. This also
                // verifies native probing holds no grant or environment lock.
                if replace {
                    environment::save(
                        root.path(),
                        &store,
                        "TOKEN",
                        EnvironmentEntryKind::Secret,
                        "replacement",
                        Some(&entry.reference),
                        Some(entry.revision),
                    )?;
                } else {
                    latest_revision = save_project_connections(
                        &path,
                        &revoked.fingerprint,
                        &revoked.grants_revision,
                        BTreeMap::new(),
                    )?
                    .grants_revision;
                }
                Ok(true)
            },
        );
        assert_eq!(probes, 1);
        assert!(result.is_err());
        let after = inspect_project_connections(&path).unwrap();
        assert_eq!(after.grants_revision, latest_revision);
        assert_eq!(
            selections(&after),
            if replace { retained } else { BTreeMap::new() }
        );
    }
}

#[test]
fn retaining_or_revoking_assignments_never_probes_keychain() {
    let _serial = environment::TEST_ENVIRONMENT_LOCK.lock().unwrap();
    let root = fixture();
    let _env = EnvironmentGuard::isolate(root.path());
    let _store = setup(root.path());
    let path = root.path().join("CWR.toml");
    let before = inspect_project_connections(&path).unwrap();
    let mut assignments = selections(&before);
    assignments.remove("context");
    let reduced = mutation::save_with(
        &path,
        &before.fingerprint,
        &before.grants_revision,
        assignments,
        |_| panic!("unchanged secret must not block another revocation"),
    )
    .unwrap();
    assert!(
        reduced
            .requirements
            .iter()
            .find(|r| r.alias == "research")
            .unwrap()
            .configured
    );
    mutation::save_with(
        &path,
        &reduced.fingerprint,
        &reduced.grants_revision,
        BTreeMap::new(),
        |_| panic!("revocation must not probe Keychain"),
    )
    .unwrap();
}
