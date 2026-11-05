const CONFIGURED_RUNTIME_IDENTITY_TYPE: &str =
    "cwp://org.coworkerprotocol/2026-09/schemas/configured_runtime_identity";
const CONFIGURED_RUNTIME_IDENTITY_FILE: &str = "runtime-identity.toml";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ConfiguredRuntimeIdentity {
    #[serde(rename = "type")]
    type_ref: String,
    runtime_id: String,
    runtime_root_ref: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfiguredRuntimeIdentityOrigin {
    Persisted,
    Fresh,
    LegacyDatabase,
}

struct SelectedConfiguredRuntimeIdentity {
    runtime_root_ref: String,
    origin: ConfiguredRuntimeIdentityOrigin,
}

fn select_configured_runtime_identity(
    state_directory: &Path,
    runtime_id: &str,
) -> CwrResult<SelectedConfiguredRuntimeIdentity> {
    select_configured_runtime_identity_with_authority(
        state_directory,
        runtime_id,
        local_machine_authority,
    )
}

fn select_configured_runtime_identity_with_authority(
    state_directory: &Path,
    runtime_id: &str,
    authority: impl FnOnce() -> CwrResult<String>,
) -> CwrResult<SelectedConfiguredRuntimeIdentity> {
    let path = configured_runtime_identity_path(state_directory);
    match read_configured_runtime_identity(&path) {
        Ok(contents) => {
            let identity = parse_configured_runtime_identity(&path, &contents, runtime_id)?;
            Ok(SelectedConfiguredRuntimeIdentity {
                runtime_root_ref: identity.runtime_root_ref,
                origin: ConfiguredRuntimeIdentityOrigin::Persisted,
            })
        }
        Err(error) if is_absent_during_inspection(error.as_ref()) => {
            let database_path = state_directory.join("cwpdb.sqlite3");
            let database_exists = match database_path.try_exists() {
                Ok(exists) => exists,
                Err(error) if is_absent_during_inspection(&error) => false,
                Err(error) => return Err(error.into()),
            };
            let (runtime_root_ref, origin) = if database_exists {
                (
                    legacy_configured_runtime_root(runtime_id),
                    ConfiguredRuntimeIdentityOrigin::LegacyDatabase,
                )
            } else {
                (
                    format!("cwp://{}/runtimes/{runtime_id}", authority()?),
                    ConfiguredRuntimeIdentityOrigin::Fresh,
                )
            };
            validate_configured_runtime_root(&runtime_root_ref, runtime_id)?;
            Ok(SelectedConfiguredRuntimeIdentity {
                runtime_root_ref,
                origin,
            })
        }
        Err(error) => Err(error),
    }
}

fn persist_configured_runtime_identity(
    state_directory: &Path,
    runtime_id: &str,
    runtime_root_ref: &str,
) -> CwrResult<()> {
    validate_configured_runtime_root(runtime_root_ref, runtime_id)?;
    let path = configured_runtime_identity_path(state_directory);
    match read_configured_runtime_identity(&path) {
        Ok(contents) => {
            let existing = parse_configured_runtime_identity(&path, &contents, runtime_id)?;
            ensure_configured_runtime_identity_matches(&path, &existing, runtime_root_ref)
        }
        Err(error) if is_not_found(error.as_ref()) => {
            publish_configured_runtime_identity(
                &path,
                state_directory,
                ConfiguredRuntimeIdentity {
                    type_ref: CONFIGURED_RUNTIME_IDENTITY_TYPE.into(),
                    runtime_id: runtime_id.into(),
                    runtime_root_ref: runtime_root_ref.into(),
                },
            )
        }
        Err(error) => Err(error),
    }
}

fn configured_runtime_identity_path(state_directory: &Path) -> PathBuf {
    state_directory.join(CONFIGURED_RUNTIME_IDENTITY_FILE)
}

fn legacy_configured_runtime_root(runtime_id: &str) -> String {
    format!("cwp://org.coworkerprotocol.local/loops/{runtime_id}")
}

fn read_configured_runtime_identity(path: &Path) -> CwrResult<Vec<u8>> {
    use std::io::Read as _;

    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return invalid(format!(
            "{}: configured runtime identity must be a regular file",
            path.display()
        ));
    }
    let mut contents = Vec::new();
    file.read_to_end(&mut contents)?;
    Ok(contents)
}

fn parse_configured_runtime_identity(
    path: &Path,
    contents: &[u8],
    runtime_id: &str,
) -> CwrResult<ConfiguredRuntimeIdentity> {
    let identity: ConfiguredRuntimeIdentity = toml::from_str(std::str::from_utf8(contents)?)?;
    if identity.type_ref != CONFIGURED_RUNTIME_IDENTITY_TYPE {
        return invalid(format!(
            "{}: unsupported configured runtime identity type",
            path.display()
        ));
    }
    if identity.runtime_id != runtime_id {
        return invalid(format!(
            "{}: configured runtime identity belongs to another runtime",
            path.display()
        ));
    }
    validate_configured_runtime_root(&identity.runtime_root_ref, runtime_id)
        .map_err(|error| invalid_error(format!("{}: {error}", path.display())))?;
    Ok(identity)
}

fn validate_configured_runtime_root(runtime_root_ref: &str, runtime_id: &str) -> CwrResult<()> {
    if runtime_root_ref == legacy_configured_runtime_root(runtime_id) {
        return Ok(());
    }
    let reference = CwrRef::parse(runtime_root_ref)?;
    let valid_host = match reference.root() {
        crate::CwrRoot::Named(name) => name
            .strip_prefix("local.")
            .is_some_and(crate::refs::is_hostname_label),
        _ => false,
    };
    if !valid_host
        || reference.checkpoint().is_some()
        || reference.fragment().is_some()
        || reference.segments() != ["runtimes", runtime_id]
    {
        return invalid(format!(
            "configured runtime root must be cwp://local.<hostname>/runtimes/{runtime_id}"
        ));
    }
    Ok(())
}

fn publish_configured_runtime_identity(
    path: &Path,
    state_directory: &Path,
    identity: ConfiguredRuntimeIdentity,
) -> CwrResult<()> {
    let contents = toml::to_string_pretty(&identity)?.into_bytes();
    let temporary_path = state_directory.join(format!(
        ".{CONFIGURED_RUNTIME_IDENTITY_FILE}.{}.tmp",
        Uuid::new_v4().simple()
    ));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .mode(0o600)
        .open(&temporary_path)?;
    use std::io::Write as _;
    let publication = (|| -> CwrResult<()> {
        file.write_all(&contents)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::hard_link(&temporary_path, path)?;
        fs::remove_file(&temporary_path)?;
        File::open(state_directory)?.sync_all()?;
        Ok(())
    })();
    match publication {
        Ok(()) => Ok(()),
        Err(error) if is_already_exists(error.as_ref()) => {
            let _ = fs::remove_file(&temporary_path);
            let existing = parse_configured_runtime_identity(
                path,
                &read_configured_runtime_identity(path)?,
                &identity.runtime_id,
            )?;
            ensure_configured_runtime_identity_matches(path, &existing, &identity.runtime_root_ref)
        }
        Err(error) => {
            let _ = fs::remove_file(&temporary_path);
            Err(error)
        }
    }
}

fn ensure_configured_runtime_identity_matches(
    path: &Path,
    existing: &ConfiguredRuntimeIdentity,
    expected_root: &str,
) -> CwrResult<()> {
    if existing.runtime_root_ref != expected_root {
        return invalid(format!(
            "{}: configured runtime identity is {}, not {expected_root}",
            path.display(),
            existing.runtime_root_ref
        ));
    }
    Ok(())
}

fn is_not_found(error: &(dyn std::error::Error + Send + Sync + 'static)) -> bool {
    error
        .downcast_ref::<io::Error>()
        .is_some_and(|error| error.kind() == io::ErrorKind::NotFound)
}

fn is_absent_during_inspection(
    error: &(dyn std::error::Error + Send + Sync + 'static),
) -> bool {
    is_not_found(error)
        || error
            .downcast_ref::<io::Error>()
            .is_some_and(|error| error.raw_os_error() == Some(libc::ENOTDIR))
}

fn is_already_exists(error: &(dyn std::error::Error + Send + Sync + 'static)) -> bool {
    error
        .downcast_ref::<io::Error>()
        .is_some_and(|error| error.kind() == io::ErrorKind::AlreadyExists)
}

#[cfg(test)]
mod configured_runtime_identity_tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::sync::Barrier;

    const RUNTIME_ID: &str =
        "c234503497e5b9bc9b1e6a6279bdba95ea9dbaf12c0b3eb28f2b0f84b55af010";

    #[test]
    fn fresh_identity_is_machine_scoped_without_creating_state() {
        let temporary = tempfile::tempdir().unwrap();
        let state = temporary.path().join("state/runtime");
        let selected = select_configured_runtime_identity_with_authority(
            &state,
            RUNTIME_ID,
            || Ok("local.test-mac".into()),
        )
        .unwrap();

        assert_eq!(
            selected.runtime_root_ref,
            format!("cwp://local.test-mac/runtimes/{RUNTIME_ID}")
        );
        assert_eq!(selected.origin, ConfiguredRuntimeIdentityOrigin::Fresh);
        assert!(!state.exists(), "inspection must not create state");
        assert!(!selected
            .runtime_root_ref
            .contains("org.coworkerprotocol.local"));
    }

    #[test]
    fn inspection_treats_a_non_directory_state_ancestor_as_absent() {
        let temporary = tempfile::tempdir().unwrap();
        let state_root = temporary.path().join("state");
        fs::write(&state_root, "runtime state is unavailable").unwrap();
        let state = state_root.join("CWR/loops").join(RUNTIME_ID);

        let selected = select_configured_runtime_identity_with_authority(
            &state,
            RUNTIME_ID,
            || Ok("local.test-mac".into()),
        )
        .unwrap();

        assert_eq!(selected.origin, ConfiguredRuntimeIdentityOrigin::Fresh);
        assert_eq!(
            selected.runtime_root_ref,
            format!("cwp://local.test-mac/runtimes/{RUNTIME_ID}")
        );
        assert_eq!(
            fs::read_to_string(state_root).unwrap(),
            "runtime state is unavailable"
        );
    }

    #[test]
    fn persisted_identity_survives_hostname_changes_and_concurrent_publication() {
        let temporary = tempfile::tempdir().unwrap();
        let state = temporary.path().join("state");
        fs::create_dir(&state).unwrap();
        let root = format!("cwp://local.original-mac/runtimes/{RUNTIME_ID}");
        let barrier = Arc::new(Barrier::new(4));
        let publishers = (0..4)
            .map(|_| {
                let state = state.clone();
                let root = root.clone();
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    persist_configured_runtime_identity(&state, RUNTIME_ID, &root)
                })
            })
            .collect::<Vec<_>>();
        for publisher in publishers {
            publisher.join().unwrap().unwrap();
        }

        let selected = select_configured_runtime_identity_with_authority(
            &state,
            RUNTIME_ID,
            || panic!("a persisted identity must not inspect the current hostname"),
        )
        .unwrap();
        assert_eq!(selected.runtime_root_ref, root);
        assert_eq!(
            selected.origin,
            ConfiguredRuntimeIdentityOrigin::Persisted
        );
    }

    #[test]
    fn database_without_identity_is_grandfathered_without_mutating_inspection() {
        let temporary = tempfile::tempdir().unwrap();
        let state = temporary.path().join("state");
        fs::create_dir(&state).unwrap();
        fs::write(state.join("cwpdb.sqlite3"), []).unwrap();

        let selected = select_configured_runtime_identity_with_authority(
            &state,
            RUNTIME_ID,
            || panic!("legacy history must not inspect the current hostname"),
        )
        .unwrap();
        assert_eq!(
            selected.runtime_root_ref,
            legacy_configured_runtime_root(RUNTIME_ID)
        );
        assert_eq!(
            selected.origin,
            ConfiguredRuntimeIdentityOrigin::LegacyDatabase
        );
        assert!(!configured_runtime_identity_path(&state).exists());
    }

    #[test]
    fn malformed_mismatched_and_symlinked_identities_fail_closed() {
        let temporary = tempfile::tempdir().unwrap();
        let state = temporary.path().join("state");
        fs::create_dir(&state).unwrap();
        let path = configured_runtime_identity_path(&state);
        fs::write(&path, "not toml").unwrap();
        assert!(select_configured_runtime_identity_with_authority(
            &state,
            RUNTIME_ID,
            || panic!("invalid saved identity must not create a replacement"),
        )
        .is_err());

        fs::remove_file(&path).unwrap();
        let outside = temporary.path().join("outside");
        fs::write(&outside, "outside").unwrap();
        symlink(&outside, &path).unwrap();
        assert!(select_configured_runtime_identity_with_authority(
            &state,
            RUNTIME_ID,
            || panic!("a symlink must not create a replacement"),
        )
        .is_err());
        assert_eq!(fs::read_to_string(outside).unwrap(), "outside");
    }
}
