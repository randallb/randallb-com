// DEPRECATED TERMINOLOGY: the workspace-identity names in this compatibility
// implementation identify a local CWR runtime, not a Workloop workspace. Do
// not emulate them in new product APIs. See
// https://github.com/getworkloop/wlp-mono/blob/main/memos/policy/workloop/desktop-design.md

fn load_or_create_workspace_identity(path: &Path) -> CwrResult<LocalWorkspaceIdentity> {
    load_or_create_workspace_identity_with_hostname(path, local_hostname)
}

fn load_or_create_workspace_identity_with_hostname(
    path: &Path,
    hostname: impl FnOnce() -> CwrResult<String>,
) -> CwrResult<LocalWorkspaceIdentity> {
    match read_workspace_identity(path) {
        Ok(contents) => match parse_workspace_identity(path, &contents) {
            Ok(identity) => Ok(identity),
            Err(_) => wait_for_concurrent_workspace_identity(path),
        },
        Err(error)
            if error
                .downcast_ref::<io::Error>()
                .is_some_and(|error| error.kind() == io::ErrorKind::NotFound) =>
        {
            let legacy_path = path.with_file_name("workspace.json");
            match read_workspace_identity(&legacy_path) {
                Ok(contents) => {
                    let identity = parse_workspace_identity(&legacy_path, &contents)?;
                    publish_workspace_identity(path, path.parent().unwrap(), identity)
                }
                Err(error)
                    if error
                        .downcast_ref::<io::Error>()
                        .is_some_and(|error| error.kind() == io::ErrorKind::NotFound) =>
                {
                    create_workspace_identity(path, &hostname()?)
                }
                Err(error) => Err(error),
            }
        }
        Err(error) => Err(error),
    }
}

fn ensure_workspace_identity_directory(parent: &Path, workspace: &Path) -> CwrResult<()> {
    let parent_was_missing = !parent.try_exists()?;
    if parent_was_missing {
        match fs::create_dir(parent) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    let metadata = fs::symlink_metadata(parent)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return invalid(format!(
            "{}: workspace identity directory must be a real directory inside the workspace",
            parent.display()
        ));
    }
    if fs::canonicalize(parent)?.parent() != Some(workspace) {
        return invalid(format!(
            "{}: workspace identity directory escapes the workspace",
            parent.display()
        ));
    }
    if parent_was_missing {
        File::open(workspace)?.sync_all()?;
    }
    Ok(())
}

fn read_workspace_identity(path: &Path) -> CwrResult<Vec<u8>> {
    use std::io::Read as _;

    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return invalid(format!(
            "{}: workspace identity must be a regular file",
            path.display()
        ));
    }
    let mut contents = Vec::new();
    file.read_to_end(&mut contents)?;
    Ok(contents)
}

fn parse_workspace_identity(path: &Path, contents: &[u8]) -> CwrResult<LocalWorkspaceIdentity> {
    let identity: LocalWorkspaceIdentity = if path.extension().is_some_and(|ext| ext == "json") {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct LegacyIdentity {
            version: u32,
            workspace_id: String,
            runtime_root_ref: Option<String>,
        }
        let legacy: LegacyIdentity = serde_json::from_slice(contents)?;
        match (legacy.version, legacy.runtime_root_ref.as_deref()) {
            (1, None) | (2, Some(_)) => {}
            _ => return invalid("unsupported or inconsistent legacy workspace identity"),
        }
        LocalWorkspaceIdentity {
            type_ref: LOCAL_WORKSPACE_IDENTITY_TYPE.into(),
            workspace_id: legacy.workspace_id,
            runtime_root_ref: legacy.runtime_root_ref,
        }
    } else {
        toml::from_str(std::str::from_utf8(contents)?)?
    };
    if identity.type_ref != LOCAL_WORKSPACE_IDENTITY_TYPE {
        return invalid("unsupported CWR workspace identity type");
    }
    let canonical_id = parse_workspace_id(&identity.workspace_id)?;
    if identity.workspace_id != canonical_id {
        return invalid(format!(
            "{}: workspace_id must use canonical UUID form {canonical_id}",
            path.display()
        ));
    }
    identity
        .runtime_root_ref()
        .map_err(|error| invalid_error(format!("{}: {error}", path.display())))?;
    Ok(identity)
}

impl LocalWorkspaceIdentity {
    fn runtime_root_ref(&self) -> CwrResult<String> {
        match self.runtime_root_ref.as_deref() {
            // Version 1 identities already own records under this exact root.
            None => runtime_root_ref_for_workspace_id(&self.workspace_id),
            Some(root) => {
                let reference = CwrRef::parse(root)?;
                let valid_host = match reference.root() {
                    crate::CwrRoot::Named(name) => name
                        .strip_prefix("local.")
                        .is_some_and(crate::refs::is_hostname_label),
                    _ => false,
                };
                if !valid_host
                    || reference.checkpoint().is_some()
                    || reference.fragment().is_some()
                    || reference.segments() != ["runtimes", self.workspace_id.as_str()]
                {
                    return invalid(
                        "runtime_root_ref must be cwp://local.<hostname>/runtimes/<workspace_id>",
                    );
                }
                Ok(root.to_owned())
            }
        }
    }
}

fn create_workspace_identity(path: &Path, hostname: &str) -> CwrResult<LocalWorkspaceIdentity> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid_error("workspace identity path has no parent directory"))?;
    let workspace = parent
        .parent()
        .ok_or_else(|| invalid_error("workspace identity directory has no parent"))?;
    ensure_workspace_identity_directory(parent, workspace)?;
    let workspace_id = Uuid::new_v4().to_string();
    let hostname = normalize_local_hostname(hostname)?;
    let identity = LocalWorkspaceIdentity {
        type_ref: LOCAL_WORKSPACE_IDENTITY_TYPE.into(),
        runtime_root_ref: Some(format!("cwp://local.{hostname}/runtimes/{workspace_id}")),
        workspace_id,
    };
    publish_workspace_identity(path, parent, identity)
}

fn publish_workspace_identity(
    path: &Path,
    parent: &Path,
    identity: LocalWorkspaceIdentity,
) -> CwrResult<LocalWorkspaceIdentity> {
    let contents = toml::to_string_pretty(&identity)?.into_bytes();
    let temporary_path = parent.join(format!(".workspace.toml.{}.tmp", Uuid::new_v4().simple()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .mode(0o644)
        .open(&temporary_path)?;
    use std::io::Write as _;
    let publication = (|| -> CwrResult<()> {
        file.write_all(&contents)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::hard_link(&temporary_path, path)?;
        fs::remove_file(&temporary_path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    match publication {
        Ok(()) => Ok(identity),
        Err(error)
            if error
                .downcast_ref::<io::Error>()
                .is_some_and(|error| error.kind() == io::ErrorKind::AlreadyExists) =>
        {
            let _ = fs::remove_file(&temporary_path);
            wait_for_concurrent_workspace_identity(path)
        }
        Err(error) => {
            let _ = fs::remove_file(&temporary_path);
            Err(error)
        }
    }
}

fn wait_for_concurrent_workspace_identity(path: &Path) -> CwrResult<LocalWorkspaceIdentity> {
    for _ in 0..100 {
        if let Ok(contents) = read_workspace_identity(path) {
            if let Ok(identity) = parse_workspace_identity(path, &contents) {
                return Ok(identity);
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    parse_workspace_identity(path, &read_workspace_identity(path)?)
}

#[cfg(test)]
mod workspace_identity_tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn persisted_runtime_root_survives_a_hostname_change_and_history_restart() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        let workspace = fs::canonicalize(workspace).unwrap();
        let manifest = workspace.join("CWR.toml");
        fs::write(&manifest, "version = '2026-09'\n").unwrap();
        let identity_path = workspace.join(".cwr/workspace.toml");
        let identity = load_or_create_workspace_identity_with_hostname(&identity_path, || {
            Ok("Original-Mac.local".into())
        })
        .unwrap();
        let expected_root = format!(
            "cwp://local.original-mac/runtimes/{}",
            identity.workspace_id
        );
        assert_eq!(
            identity.runtime_root_ref.as_deref(),
            Some(expected_root.as_str())
        );
        let original_file = fs::read(&identity_path).unwrap();

        let renamed = load_or_create_workspace_identity_with_hostname(&identity_path, || {
            panic!("an existing identity must not look up the current hostname")
        })
        .unwrap();
        assert_eq!(renamed, identity);
        assert_eq!(fs::read(&identity_path).unwrap(), original_file);

        let paths = LocalWorkspacePaths::resolve_with_base_directories(
            &manifest,
            temporary.path().join("state"),
            temporary.path().join("runtime"),
        )
        .unwrap();
        assert_eq!(paths.runtime_root_ref, expected_root);
        let open = || {
            LocalRuntime::open_with_state_path_and_runtime_ref(
                super::tests::fixture_workspace(),
                paths.database_path(),
                &paths.runtime_root_ref,
                super::tests::fixture_host_identity(),
            )
            .unwrap()
        };
        let runtime = open();
        let admission = runtime.admit_text(None, "@helper remember this").unwrap();
        assert!(admission
            .conversation_ref()
            .starts_with(&format!("{expected_root}/conversations/")));
        assert!(admission
            .record
            .invocation_ref
            .as_ref()
            .unwrap()
            .starts_with(&format!("{expected_root}/invocations/")));
        let conversation = runtime
            .conversation(admission.conversation_ref())
            .unwrap()
            .unwrap();
        let message_ref = conversation.messages[0].message_ref.clone();
        assert!(message_ref.starts_with(&format!("{expected_root}/messages/")));
        drop(runtime);
        let reopened = open();
        let retained = reopened
            .conversation(admission.conversation_ref())
            .unwrap()
            .unwrap();
        assert_eq!(retained.messages[0].message_ref, message_ref);
        assert_eq!(retained.messages[0].text, conversation.messages[0].text);
    }

    #[test]
    fn version_one_identity_keeps_its_original_root_without_rewriting_the_file() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().join("workspace");
        fs::create_dir_all(workspace.join(".cwr")).unwrap();
        let manifest = workspace.join("CWR.toml");
        fs::write(&manifest, "version = '2026-09'\n").unwrap();
        let identity_path = workspace.join(".cwr/workspace.json");
        let workspace_id = Uuid::new_v4().to_string();
        let original =
            serde_json::to_vec(&json!({"version": 1, "workspace_id": workspace_id})).unwrap();
        fs::write(&identity_path, &original).unwrap();
        let paths = LocalWorkspacePaths::resolve_with_base_directories(
            &manifest,
            temporary.path().join("state"),
            temporary.path().join("runtime"),
        )
        .unwrap();
        assert_eq!(
            paths.runtime_root_ref,
            format!("cwp://org.coworkerprotocol.local/runtimes/{workspace_id}")
        );
        assert_eq!(fs::read(&identity_path).unwrap(), original);
        let migrated = workspace.join(".cwr/workspace.toml");
        let identity = parse_workspace_identity(&migrated, &fs::read(&migrated).unwrap()).unwrap();
        assert_eq!(identity.runtime_root_ref().unwrap(), paths.runtime_root_ref);
    }

    #[test]
    fn toml_identity_uses_schema_and_migrates_version_two_without_hostname_lookup() {
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().join(".cwr");
        fs::create_dir(&directory).unwrap();
        let path = directory.join("workspace.toml");
        let workspace_id = Uuid::new_v4().to_string();
        let root = format!("cwp://local.original/runtimes/{workspace_id}");
        fs::write(
            directory.join("workspace.json"),
            serde_json::to_vec(&json!({
                "version": 2, "workspace_id": workspace_id, "runtime_root_ref": root,
            }))
            .unwrap(),
        )
        .unwrap();
        let identity =
            load_or_create_workspace_identity_with_hostname(&path, || panic!("saved identity"))
                .unwrap();
        assert_eq!(identity.runtime_root_ref().unwrap(), root);
        let contents = fs::read_to_string(&path).unwrap();
        let document: toml::Value = toml::from_str(&contents).unwrap();
        assert_eq!(
            document["type"].as_str(),
            Some(LOCAL_WORKSPACE_IDENTITY_TYPE)
        );
        assert!(document.get("version").is_none());
        assert_eq!(load_or_create_workspace_identity(&path).unwrap(), identity);
        for invalid in [
            contents.replace(LOCAL_WORKSPACE_IDENTITY_TYPE, "cwp://example/unknown"),
            format!("{contents}version = 2\n"),
        ] {
            assert!(parse_workspace_identity(&path, invalid.as_bytes()).is_err());
        }
    }

    #[test]
    fn malformed_identity_never_falls_back_or_creates_a_new_identity() {
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().join(".cwr");
        fs::create_dir(&directory).unwrap();
        let path = directory.join("workspace.toml");
        let legacy = directory.join("workspace.json");
        fs::write(&legacy, "invalid json").unwrap();
        assert!(
            load_or_create_workspace_identity_with_hostname(&path, || panic!("must not create"))
                .is_err()
        );
        assert!(!path.exists());
        fs::write(
            &legacy,
            serde_json::to_vec(&json!({"version": 1, "workspace_id": Uuid::new_v4().to_string()}))
                .unwrap(),
        )
        .unwrap();
        fs::write(&path, "invalid toml").unwrap();
        assert!(
            load_or_create_workspace_identity_with_hostname(&path, || panic!("must not create"))
                .is_err()
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "invalid toml");
    }

    #[test]
    fn rejects_missing_or_mismatched_persisted_runtime_roots() {
        let workspace_id = Uuid::new_v4().to_string();
        let valid_root = format!("cwp://local.test-mac/runtimes/{workspace_id}");
        let path = Path::new("workspace.json");
        for (version, root) in [
            (2, None),
            (1, Some(valid_root.clone())),
            (3, Some(valid_root.clone())),
            (
                2,
                Some(format!("cwp://local.test-mac/runtimes/{}", Uuid::new_v4())),
            ),
            (2, Some(format!("{valid_root}/messages/one"))),
            (2, Some(format!("{valid_root}#member"))),
            (
                2,
                Some(format!(
                    "cwp://local.test-mac@checkpoint/runtimes/{workspace_id}"
                )),
            ),
            (
                2,
                Some(format!("cwp://local.test_mac/runtimes/{workspace_id}")),
            ),
            (
                2,
                Some(format!("cwp://local.Test-Mac/runtimes/{workspace_id}")),
            ),
            (
                2,
                Some(format!("cwp://com.example/runtimes/{workspace_id}")),
            ),
        ] {
            let contents = serde_json::to_vec(&json!({
                "version": version, "workspace_id": workspace_id, "runtime_root_ref": root,
            }))
            .unwrap();
            assert!(
                parse_workspace_identity(path, &contents).is_err(),
                "version {version}: {root:?}"
            );
        }
    }

    #[test]
    fn refuses_a_symlinked_workspace_identity_directory() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().join("workspace");
        let outside = temporary.path().join("outside");
        fs::create_dir(&workspace).unwrap();
        fs::create_dir(&outside).unwrap();
        symlink(&outside, workspace.join(".cwr")).unwrap();

        let error = create_workspace_identity(&workspace.join(".cwr/workspace.toml"), "test-mac")
            .unwrap_err()
            .to_string();

        assert!(error.contains("must be a real directory"));
        assert!(!outside.join("workspace.toml").exists());
    }

    #[test]
    fn refuses_a_symlinked_workspace_identity_file() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().join("workspace");
        let identity_directory = workspace.join(".cwr");
        let outside = temporary.path().join("outside.json");
        fs::create_dir_all(&identity_directory).unwrap();
        fs::write(&outside, "{}\n").unwrap();
        symlink(&outside, identity_directory.join("workspace.toml")).unwrap();

        assert!(
            load_or_create_workspace_identity(&identity_directory.join("workspace.toml")).is_err()
        );
        assert_eq!(fs::read_to_string(outside).unwrap(), "{}\n");
    }
}
