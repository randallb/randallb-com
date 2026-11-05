use super::*;

#[test]
fn adapter_storage_selection_is_enforced_by_startup_requirements() {
    let root = tempfile::tempdir().unwrap();
    publish_requirement(root.path(), None, SmolvmEnvironment::default(), None, None).unwrap();
    let path = root.path().join(REQUIREMENT_NAME);
    let original: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    validate_requirement(
        root.path(),
        &None,
        &SmolvmEnvironment::default(),
        &None,
        &None,
    )
    .unwrap();
    for field in [
        "data_root",
        "source_home",
        "codex_home",
        "user_home",
        "xdg_data_home",
    ] {
        let mut changed = original.clone();
        changed["adapter_storage"][field] =
            serde_json::json!(format!("{}/changed", original["adapter_storage"][field]));
        fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(
            validate_requirement(
                root.path(),
                &None,
                &SmolvmEnvironment::default(),
                &None,
                &None
            )
            .is_err(),
            "{field}"
        );
    }
    // Older hosts do not impose a storage choice. A new host's explicit
    // requirement must still reject a differently configured starter above.
    let mut legacy = original;
    legacy.as_object_mut().unwrap().remove("adapter_storage");
    fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
    validate_requirement(
        root.path(),
        &None,
        &SmolvmEnvironment::default(),
        &None,
        &None,
    )
    .unwrap();
    let old: ResidentEnvironment =
        serde_json::from_str(r#"{"pid":1,"codex_executable":null}"#).unwrap();
    assert_eq!(old.adapter_storage, None);
}

#[test]
fn new_runtime_environment_check_does_not_create_state() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("not-started");
    ensure(
        &directory,
        &directory.join("runtime.sock"),
        &directory.join("host.sock"),
    )
    .unwrap();
    assert!(!directory.exists());
}

#[test]
fn codex_tier_file_is_part_of_resident_environment_requirement() {
    let temporary = tempfile::tempdir().unwrap();
    let file = Some("/host/codex-tier.json".into());
    publish_requirement(
        temporary.path(),
        None,
        SmolvmEnvironment::default(),
        None,
        file.clone(),
    )
    .unwrap();
    validate_requirement(
        temporary.path(),
        &None,
        &SmolvmEnvironment::default(),
        &None,
        &file,
    )
    .unwrap();
    assert!(validate_requirement(
        temporary.path(),
        &None,
        &SmolvmEnvironment::default(),
        &None,
        &None
    )
    .is_err());
    assert!(validate_requirement(
        temporary.path(),
        &None,
        &SmolvmEnvironment::default(),
        &None,
        &Some("/other/tier.json".into())
    )
    .is_err());
    let old: ResidentEnvironment =
        serde_json::from_str(r#"{"pid":1,"codex_executable":null}"#).unwrap();
    assert_eq!(old.codex_service_tier_file, None);
}

#[test]
fn sms_socket_is_part_of_resident_environment_requirement() {
    let temporary = tempfile::tempdir().unwrap();
    let directory = temporary.path();
    let codex = Some("/workloop/codex".into());
    let sms = Some("/private/workloop-sms.sock".into());
    assert_eq!(nonempty_sms_bridge_socket(Some(String::new())), None);
    assert_eq!(nonempty_sms_bridge_socket(Some("  ".into())), None);
    assert_eq!(nonempty_sms_bridge_socket(sms.clone()), sms);
    publish_requirement(
        directory,
        codex.clone(),
        SmolvmEnvironment::default(),
        sms.clone(),
        None,
    )
    .unwrap();
    validate_requirement(
        directory,
        &codex,
        &SmolvmEnvironment::default(),
        &sms,
        &None,
    )
    .unwrap();
    assert!(validate_requirement(
        directory,
        &codex,
        &SmolvmEnvironment::default(),
        &None,
        &None
    )
    .unwrap_err()
    .to_string()
    .contains("required runtime environment"));
    assert!(validate_requirement(
        directory,
        &codex,
        &SmolvmEnvironment::default(),
        &Some("/private/other.sock".into()),
        &None
    )
    .is_err());

    // A resident published before this field existed cannot satisfy an
    // enabled SMS connection merely because its Codex executable matches.
    let old: ResidentEnvironment =
        serde_json::from_str(r#"{"pid":1,"codex_executable":"/workloop/codex"}"#).unwrap();
    assert_eq!(old.sms_bridge_socket, None);
    assert!(validate_requirement(
        directory,
        &old.codex_executable,
        &old.smolvm,
        &old.sms_bridge_socket,
        &None
    )
    .is_err());
}

#[test]
fn smolvm_image_and_executable_changes_require_resident_replacement() {
    let root = tempfile::tempdir().unwrap();
    let selected = SmolvmEnvironment {
        executable: Some("/nix/store/pinned/bin/smolvm".into()),
        image: Some("/images/guest.tar.gz".into()),
        image_sha256: Some("a".repeat(64)),
    };
    publish_requirement(root.path(), None, selected.clone(), None, None).unwrap();
    validate_requirement(root.path(), &None, &selected, &None, &None).unwrap();
    for changed in [
        SmolvmEnvironment {
            executable: Some("/other/smolvm".into()),
            ..selected.clone()
        },
        SmolvmEnvironment {
            image: Some("/images/updated.tar.gz".into()),
            ..selected.clone()
        },
        SmolvmEnvironment {
            image_sha256: Some("b".repeat(64)),
            ..selected.clone()
        },
        SmolvmEnvironment::default(),
    ] {
        assert!(validate_requirement(root.path(), &None, &changed, &None, &None).is_err());
    }
    let old: ResidentEnvironment =
        serde_json::from_str(r#"{"pid":1,"codex_executable":null}"#).unwrap();
    assert_eq!(old.smolvm, SmolvmEnvironment::default());
}

#[test]
fn analytics_environment_changes_require_resident_replacement() {
    let root = tempfile::tempdir().unwrap();
    publish_requirement(root.path(), None, SmolvmEnvironment::default(), None, None).unwrap();
    validate_requirement(
        root.path(),
        &None,
        &SmolvmEnvironment::default(),
        &None,
        &None,
    )
    .unwrap();
    let path = root.path().join(REQUIREMENT_NAME);
    let mut requirement: RequiredResidentEnvironment =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    requirement.analytics_environment = Some("different-export-settings".into());
    fs::write(&path, serde_json::to_vec(&requirement).unwrap()).unwrap();
    assert!(validate_requirement(
        root.path(),
        &None,
        &SmolvmEnvironment::default(),
        &None,
        &None
    )
    .is_err());
    let legacy: ResidentEnvironment =
        serde_json::from_str(r#"{"pid":1,"codex_executable":null}"#).unwrap();
    assert_eq!(legacy.analytics_environment, None);
}
