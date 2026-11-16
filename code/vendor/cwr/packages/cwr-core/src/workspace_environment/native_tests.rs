use super::*;

#[test]
#[ignore = "creates and removes a disposable native Keychain item"]
fn native_keychain_workspace_round_trip() {
    let _guard = ENVIRONMENT_LOCK.lock().unwrap();
    let _identity = TestIdentity::new();
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let previous = env::var_os("WORKLOOP_STATE_HOME");
    let first_state = root.path().join("native-state-a");
    env::set_var("WORKLOOP_STATE_HOME", &first_state);
    let store = workspace_secret_store().unwrap();
    let saved = save(
        &workspace,
        &store,
        "TOKEN",
        EnvironmentEntryKind::Secret,
        "disposable-native-workspace-secret",
        None,
        None,
    )
    .unwrap();
    let entry = saved.entries.first().unwrap();
    assert_eq!(
        resolve_with(&workspace, &store, &entry.reference, entry.revision).unwrap(),
        "disposable-native-workspace-secret"
    );
    env::set_var("WORKLOOP_STATE_HOME", root.path().join("native-state-b"));
    let other_store = workspace_secret_store().unwrap();
    assert!(list(&workspace, &other_store).unwrap().entries.is_empty());
    let other = save(
        &workspace,
        &other_store,
        "TOKEN",
        EnvironmentEntryKind::Secret,
        "other-train-secret",
        None,
        None,
    )
    .unwrap();
    let other_entry = other.entries.first().unwrap();
    assert_eq!(
        resolve_with(
            &workspace,
            &other_store,
            &other_entry.reference,
            other_entry.revision,
        )
        .unwrap(),
        "other-train-secret"
    );
    remove(
        &workspace,
        &other_store,
        &other_entry.reference,
        other_entry.revision,
    )
    .unwrap();
    env::set_var("WORKLOOP_STATE_HOME", &first_state);
    remove(&workspace, &store, &entry.reference, entry.revision).unwrap();
    if let Some(previous) = previous {
        env::set_var("WORKLOOP_STATE_HOME", previous);
    } else {
        env::remove_var("WORKLOOP_STATE_HOME");
    }
}
