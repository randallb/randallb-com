use super::*;

#[test]
fn fresh_configured_runtime_persists_only_the_machine_scoped_identity_on_open() {
    let temporary = tempfile::tempdir().unwrap();
    let path = workspace(temporary.path(), &declaration("one"));
    let mut composition = LocalWorkspaceComposition::load(&path).unwrap();
    let configured = &mut composition.runtimes[0];
    configured.state_directory = temporary.path().join("state/CWR/loops/one");

    assert!(configured.runtime_ref.starts_with("cwp://local."));
    assert!(configured.runtime_ref.contains("/runtimes/"));
    assert!(!configured
        .runtime_ref
        .contains("org.coworkerprotocol.local"));
    assert!(!configured.state_directory.exists());

    let runtime = configured.open("cwp://example.test/cli").unwrap();
    let identity_path = configured_runtime_identity_path(&configured.state_directory);
    let identity = parse_configured_runtime_identity(
        &identity_path,
        &read_configured_runtime_identity(&identity_path).unwrap(),
        &configured.identity_runtime_id,
    )
    .unwrap();
    assert_eq!(identity.runtime_root_ref, configured.runtime_ref);
    drop(runtime);

    // Reusing the same inspected selection must accept the identity it just published.
    configured.open("cwp://example.test/cli").unwrap();
}

#[test]
fn legacy_database_is_reopened_under_its_exact_root_then_grandfathered() {
    let temporary = tempfile::tempdir().unwrap();
    let path = workspace(temporary.path(), &declaration("one"));
    let mut composition = LocalWorkspaceComposition::load(&path).unwrap();
    let configured = &mut composition.runtimes[0];
    configured.state_directory = temporary.path().join("state/CWR/loops/one");
    configured.runtime_ref = legacy_configured_runtime_root(&configured.identity_runtime_id);

    // Simulate a database created by the prior executable, which knew no sidecar.
    configured.identity_origin = ConfiguredRuntimeIdentityOrigin::Persisted;
    let runtime = configured.open("cwp://example.test/cli").unwrap();
    let admission = runtime
        .admit_action(None, "@cwr--list--coworkers", json!({}))
        .unwrap();
    let conversation_ref = admission.conversation_ref().to_owned();
    assert!(conversation_ref.starts_with(&format!("{}/conversations/", configured.runtime_ref)));
    drop(runtime);
    assert!(!configured_runtime_identity_path(&configured.state_directory).exists());

    let selected = select_configured_runtime_identity_with_authority(
        &configured.state_directory,
        &configured.identity_runtime_id,
        || panic!("legacy history must not inspect the hostname"),
    )
    .unwrap();
    assert_eq!(selected.runtime_root_ref, configured.runtime_ref);
    assert_eq!(
        selected.origin,
        ConfiguredRuntimeIdentityOrigin::LegacyDatabase
    );

    configured.identity_origin = selected.origin;
    let reopened = configured.open("cwp://example.test/cli").unwrap();
    assert!(reopened.conversation(&conversation_ref).unwrap().is_some());
    assert!(configured_runtime_identity_path(&configured.state_directory).exists());
}
