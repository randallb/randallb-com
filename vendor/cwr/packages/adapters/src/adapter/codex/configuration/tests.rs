use super::*;
use std::os::unix::fs::{symlink, PermissionsExt as _};

#[test]
fn configuration_imports_portable_files_once_without_mutating_source() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("original");
    let data = root.path().join("adapter");
    fs::create_dir_all(source.join("sessions")).unwrap();
    fs::create_dir_all(source.join("skills/tool")).unwrap();
    fs::write(source.join("skills/tool/run.sh"), "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(
        source.join("skills/tool/run.sh"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    fs::create_dir(&data).unwrap();
    fs::write(source.join("auth.json"), "original-auth").unwrap();
    fs::write(source.join("config.toml"), "state_db = '/external'").unwrap();
    fs::write(source.join("sessions/rollout.jsonl"), "session").unwrap();
    fs::write(source.join("sessions/live.sqlite-wal"), "database").unwrap();
    symlink(source.join("auth.json"), source.join("sessions/link")).unwrap();
    let first = configure_at(data.clone(), Some(source.clone())).unwrap();
    assert_eq!(
        fs::read_to_string(first.provider_home.join("auth.json")).unwrap(),
        "original-auth"
    );
    assert!(first.provider_home.join("sessions/rollout.jsonl").exists());
    assert!(!first
        .provider_home
        .join("sessions/live.sqlite-wal")
        .exists());
    assert!(!first.provider_home.join("sessions/link").exists());
    assert!(!first.provider_home.join("config.toml").exists());
    assert_eq!(
        fs::metadata(first.provider_home.join("skills/tool/run.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    fs::write(first.provider_home.join("auth.json"), "new-auth").unwrap();
    let second = configure_at(data, Some(source.clone())).unwrap();
    assert_eq!(first.provider_home, second.provider_home);
    assert_eq!(
        fs::read_to_string(second.provider_home.join("auth.json")).unwrap(),
        "new-auth"
    );
    assert_eq!(
        fs::read_to_string(source.join("auth.json")).unwrap(),
        "original-auth"
    );
}

#[test]
fn configuration_publishes_once_for_concurrent_workspace_launches() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("adapter");
    fs::create_dir(&data).unwrap();
    let threads = (0..4)
        .map(|_| {
            let data = data.clone();
            std::thread::spawn(move || configure_at(data, None).unwrap().provider_home)
        })
        .collect::<Vec<_>>();
    for thread in threads {
        assert_eq!(thread.join().unwrap(), data.join("provider-home"));
    }
}

#[test]
fn configuration_refuses_symlink_destination_and_recursive_import() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("adapter");
    fs::create_dir(&data).unwrap();
    assert!(configure_at(data.clone(), Some(root.path().to_owned())).is_err());
    symlink(root.path(), data.join("provider-home")).unwrap();
    assert!(configure_at(data, None).is_err());
}

#[test]
fn configuration_rejects_recursive_import_through_a_source_alias() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let data = source.join("skills/adapter");
    fs::create_dir_all(&data).unwrap();
    let alias = root.path().join("source-alias");
    symlink(&source, &alias).unwrap();
    let error = configure_at(data.clone(), Some(alias)).unwrap_err();
    assert!(error.to_string().contains("outside the source home"));
    assert!(!data.join("provider-home").exists());
    assert_eq!(
        fs::read_dir(data).unwrap().count(),
        1,
        "only the lock remains"
    );
}
fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("original");
    let data = root.path().join("adapter");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&data).unwrap();
    (root, source, data)
}

#[test]
fn imports_plugins_marketplaces_and_rebased_config_without_runtime_files() {
    let (_root, source, data) = fixture();
    for folder in [
        "plugins/cache/example/tool/1",
        "plugins/data/tool",
        ".tmp/marketplaces/example",
        ".tmp/staging",
        "shell_snapshots",
        "secrets",
    ] {
        fs::create_dir_all(source.join(folder)).unwrap();
        fs::write(
            source.join(folder).join("asset.db"),
            "portable-plugin-asset",
        )
        .unwrap();
    }
    fs::write(source.join("state_5.sqlite"), "live-db").unwrap();
    fs::write(
        source.join(".credentials.json"),
        "fictional-mcp-credentials",
    )
    .unwrap();
    fs::write(source.join("config.toml"), format!("sqlite_home = '/external/db'\n[features]\nplugins = true\n[plugins.\"tool@example\"]\nenabled = false\n[marketplaces.example]\nsource_type = 'local'\nsource = '{}'\n[profiles.test]\nlog_dir = '/external/log'\nmodel = 'fixture-model'\n", source.join(".tmp/marketplaces/example").display())).unwrap();
    let report = importer::run(&data, &source, false, false).unwrap();
    let home = fs::canonicalize(data.join("provider-home")).unwrap();
    assert!(home.join("plugins/cache/example/tool/1/asset.db").exists());
    assert!(home.join("plugins/data/tool/asset.db").exists());
    assert!(home.join(".tmp/marketplaces/example/asset.db").exists());
    assert!(home.join(".credentials.json").exists());
    assert!(!home.join(".tmp/staging").exists());
    assert!(!home.join("shell_snapshots").exists());
    assert!(!home.join("secrets").exists());
    assert!(!home.join("state_5.sqlite").exists());
    let config: toml::Value =
        toml::from_str(&fs::read_to_string(home.join("config.toml")).unwrap()).unwrap();
    assert_eq!(
        config["plugins"]["tool@example"]["enabled"].as_bool(),
        Some(false)
    );
    assert_eq!(
        config["marketplaces"]["example"]["source"].as_str(),
        home.join(".tmp/marketplaces/example").to_str()
    );
    assert!(config.get("sqlite_home").is_none());
    assert!(config["profiles"]["test"].get("log_dir").is_none());
    assert_eq!(
        config["profiles"]["test"]["model"].as_str(),
        Some("fixture-model")
    );
    assert!(report.reauthentication_required);
}

#[test]
fn dry_run_is_read_only_including_an_absent_destination() {
    let (root, source, _data) = fixture();
    fs::write(source.join("auth.json"), "fictional-auth").unwrap();
    let data = root.path().join("not-created/adapter");
    let report = importer::run(&data, &source, true, false).unwrap();
    assert_eq!(report.imported, vec![PathBuf::from("auth.json")]);
    assert!(!root.path().join("not-created").exists());
}

#[test]
fn reimport_merges_config_preserves_conflicts_and_backs_up_replacements() {
    let (_root, source, data) = fixture();
    fs::write(source.join("auth.json"), "old-auth").unwrap();
    fs::write(
        source.join("config.toml"),
        "[plugins.\"tool@example\"]\nenabled = true\n",
    )
    .unwrap();
    importer::run(&data, &source, false, false).unwrap();
    let home = fs::canonicalize(data.join("provider-home")).unwrap();
    fs::write(home.join("auth.json"), "current-auth").unwrap();
    fs::write(
        home.join("config.toml"),
        "[plugins.\"tool@example\"]\nenabled = false\n",
    )
    .unwrap();
    fs::write(
        source.join("config.toml"),
        "[plugins.\"tool@example\"]\nenabled = true\n[features]\nplugins = true\n",
    )
    .unwrap();
    let report = importer::run(&data, &source, false, false).unwrap();
    assert!(report.conflicts.contains(&PathBuf::from("auth.json")));
    assert!(report
        .conflicts
        .contains(&PathBuf::from("config.toml:plugins:tool@example:enabled")));
    assert_eq!(
        fs::read_to_string(home.join("auth.json")).unwrap(),
        "current-auth"
    );
    let cfg: toml::Value =
        toml::from_str(&fs::read_to_string(home.join("config.toml")).unwrap()).unwrap();
    assert_eq!(
        cfg["plugins"]["tool@example"]["enabled"].as_bool(),
        Some(false)
    );
    assert_eq!(cfg["features"]["plugins"].as_bool(), Some(true));
    let replaced = importer::run(&data, &source, false, true).unwrap();
    let backup = replaced.backup_directory.unwrap();
    assert_eq!(
        fs::read_to_string(backup.join("auth.json")).unwrap(),
        "current-auth"
    );
    assert_eq!(
        fs::read_to_string(home.join("auth.json")).unwrap(),
        "old-auth"
    );
    assert!(fs::read_to_string(backup.join("config.toml"))
        .unwrap()
        .contains("false"));
    assert_eq!(
        fs::read_to_string(source.join("auth.json")).unwrap(),
        "old-auth"
    );
}

#[test]
fn migration_backfills_an_existing_home_once_without_overwrite() {
    let (_root, source, data) = fixture();
    fs::create_dir_all(data.join("provider-home")).unwrap();
    fs::write(data.join("provider-home/auth.json"), "current-auth").unwrap();
    fs::write(source.join("auth.json"), "source-auth").unwrap();
    fs::create_dir_all(source.join("plugins/cache/tool")).unwrap();
    fs::write(source.join("plugins/cache/tool/plugin.json"), "{}").unwrap();
    configure_at(data.clone(), Some(source.clone())).unwrap();
    assert!(data
        .join("provider-home/plugins/cache/tool/plugin.json")
        .exists());
    assert_eq!(
        fs::read_to_string(data.join("provider-home/auth.json")).unwrap(),
        "current-auth"
    );
    fs::write(source.join("new-content.txt"), "later").unwrap();
    configure_at(data.clone(), Some(source.clone())).unwrap();
    assert!(!data.join("provider-home/new-content.txt").exists());
    importer::run(&data, &source, false, false).unwrap();
    assert!(data.join("provider-home/new-content.txt").exists());
}

#[test]
fn reimport_rejects_symlink_targets_and_missing_or_overlapping_sources() {
    let (root, source, data) = fixture();
    fs::create_dir_all(source.join("skills/tool")).unwrap();
    fs::write(source.join("skills/tool/SKILL.md"), "fixture").unwrap();
    fs::create_dir(data.join("provider-home")).unwrap();
    let outside = root.path().join("outside");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, data.join("provider-home/skills")).unwrap();
    assert!(importer::run(&data, &source, true, false).is_err());
    assert!(importer::run(&data, &root.path().join("missing"), false, false).is_err());
    assert!(importer::run(&data, &data.join("provider-home"), true, false).is_err());
    assert_eq!(fs::read_dir(outside).unwrap().count(), 0);
}

#[test]
fn source_alias_rebases_config_and_failed_overlap_does_not_mutate_source() {
    let (root, source, data) = fixture();
    let alias = root.path().join("source-alias");
    symlink(&source, &alias).unwrap();
    fs::write(
        source.join("config.toml"),
        format!(
            "[marketplaces.fixture]\nsource_type = 'local'\nsource = '{}'\n",
            alias.join(".tmp/marketplaces/fixture").display()
        ),
    )
    .unwrap();
    fs::write(source.join("auth.json"), "fictional-auth").unwrap();
    importer::run(&data, &alias, false, false).unwrap();
    let home = fs::canonicalize(data.join("provider-home")).unwrap();
    let config: toml::Value =
        toml::from_str(&fs::read_to_string(home.join("config.toml")).unwrap()).unwrap();
    assert_eq!(
        config["marketplaces"]["fixture"]["source"].as_str(),
        home.join(".tmp/marketplaces/fixture").to_str()
    );
    assert!(home.join("auth.json").exists());
    let nested_data = source.join("not-created/adapter");
    assert!(importer::validate_source(&nested_data, &source).is_err());
    assert!(!source.join("not-created").exists());
}

#[test]
fn destination_alias_rebases_config_to_the_canonical_provider_home() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let canonical = root.path().join("canonical");
    let alias = root.path().join("alias");
    let data = alias.join("adapter");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&canonical).unwrap();
    symlink(&canonical, &alias).unwrap();
    fs::create_dir(&data).unwrap();
    fs::create_dir_all(source.join(".tmp/marketplaces/fixture")).unwrap();
    fs::write(
        source.join("config.toml"),
        format!(
            "model = 'source-model'\n[marketplaces.fixture]\nsource_type = 'local'\nsource = '{}'\n",
            source.join(".tmp/marketplaces/fixture").display()
        ),
    )
    .unwrap();

    let report = importer::run(&data, &source, false, false).unwrap();
    let canonical_home = fs::canonicalize(canonical.join("adapter/provider-home")).unwrap();
    assert_eq!(report.provider_home, canonical_home);
    let config: toml::Value =
        toml::from_str(&fs::read_to_string(canonical_home.join("config.toml")).unwrap()).unwrap();
    assert!(config.get("model").is_none());
    assert_eq!(
        config["marketplaces"]["fixture"]["source"].as_str(),
        canonical_home.join(".tmp/marketplaces/fixture").to_str()
    );
}

#[test]
fn provider_home_symlink_is_rejected_before_canonical_rebasing() {
    let (_root, source, data) = fixture();
    fs::write(source.join("config.toml"), "model = 'source-model'\n").unwrap();
    symlink(&source, data.join("provider-home")).unwrap();

    assert!(importer::run(&data, &source, false, true).is_err());
    assert_eq!(
        fs::read_to_string(source.join("config.toml")).unwrap(),
        "model = 'source-model'\n"
    );
}

#[test]
fn reimport_removes_stale_global_model_override() {
    let (_root, source, data) = fixture();
    fs::write(source.join("config.toml"), "model = 'source-model'\n").unwrap();
    let home = data.join("provider-home");
    fs::create_dir(&home).unwrap();
    fs::write(home.join("config.toml"), "model = 'destination-model'\n").unwrap();

    importer::run(&data, &source, false, true).unwrap();
    let config: toml::Value =
        toml::from_str(&fs::read_to_string(home.join("config.toml")).unwrap()).unwrap();
    assert!(config.get("model").is_none());
}

#[test]
fn config_rebases_normalized_internal_paths_but_preserves_external_references() {
    let (root, source, data) = fixture();
    fs::create_dir(source.join("nested")).unwrap();
    fs::create_dir(source.join("internal-market")).unwrap();
    let external = root.path().join("external-market");
    fs::create_dir(&external).unwrap();
    symlink(&external, source.join("external-alias")).unwrap();
    let internal = source.join("nested/../internal-market");
    let parent_escape = source.join("../external-market");
    let alias_escape = source.join("external-alias");
    let missing_escape = alias_escape.join("not-created");
    // The path through alias/.. resolves relative to the external target,
    // not relative to the spelling of the symlink inside source.
    let symlink_parent_escape = alias_escape.join("../another-external");
    let paths = [
        &internal,
        &parent_escape,
        &alias_escape,
        &missing_escape,
        &symlink_parent_escape,
    ];
    let mut text = String::new();
    for (index, path) in paths.iter().enumerate() {
        text.push_str(&format!(
            "[marketplaces.m{index}]\nsource_type = 'local'\nsource = '{}'\n",
            path.display()
        ));
        text.push_str(&format!(
            "[projects.'{}']\ntrust_level = 'trusted'\n",
            path.display()
        ));
    }
    fs::write(source.join("config.toml"), text).unwrap();
    importer::run(&data, &source, false, false).unwrap();
    let home = fs::canonicalize(data.join("provider-home")).unwrap();
    let config: toml::Value =
        toml::from_str(&fs::read_to_string(home.join("config.toml")).unwrap()).unwrap();
    assert_eq!(
        config["marketplaces"]["m0"]["source"].as_str(),
        home.join("internal-market").to_str()
    );
    assert!(config["projects"]
        .get(home.join("internal-market").to_str().unwrap())
        .is_some());
    for (index, path) in paths.iter().enumerate().skip(1) {
        assert_eq!(
            config["marketplaces"]
                .get(format!("m{index}").as_str())
                .unwrap()["source"]
                .as_str(),
            path.to_str()
        );
        assert!(config["projects"].get(path.to_str().unwrap()).is_some());
    }
}

#[test]
fn portable_empty_plugin_directories_are_planned_and_imported() {
    let (_root, source, data) = fixture();
    let resource = Path::new("plugins/cache/example/tool/1/empty-resource");
    let working = Path::new("plugins/data/example/tool/empty-working");
    fs::create_dir_all(source.join(resource)).unwrap();
    fs::create_dir_all(source.join(working)).unwrap();
    let home = data.join("provider-home");
    let preview = importer::run(&data, &source, true, false).unwrap();
    assert!(preview.imported.contains(&resource.to_owned()));
    assert!(preview.imported.contains(&working.to_owned()));
    assert!(!home.exists());
    let applied = importer::run(&data, &source, false, false).unwrap();
    assert_eq!(preview.imported, applied.imported);
    assert!(home.join(resource).is_dir());
    assert!(home.join(working).is_dir());
    assert!(importer::run(&data, &source, false, false)
        .unwrap()
        .imported
        .is_empty());
}
