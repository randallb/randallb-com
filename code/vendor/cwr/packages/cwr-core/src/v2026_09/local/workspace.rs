const LOCAL_WORKSPACE_IDENTITY_TYPE: &str =
    "cwp://org.coworkerprotocol/2026-09/schemas/workspace_identity";

/// Deprecated terminology retained for compatibility: this record identifies
/// a local CWR runtime, not a Workloop workspace. Do not emulate this name in
/// new product APIs. See [Workloop Desktop design guidance](https://github.com/getworkloop/wlp-mono/blob/main/memos/policy/workloop/desktop-design.md).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LocalWorkspaceIdentity {
    #[serde(rename = "type")]
    pub type_ref: String,
    pub workspace_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_root_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalWorkspaceBinding {
    pub workspace_id: String,
    pub workspace_directory: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalWorkspacePaths {
    pub cwr_toml: PathBuf,
    pub workspace_directory: PathBuf,
    pub identity_path: PathBuf,
    pub identity: LocalWorkspaceIdentity,
    pub state_directory: PathBuf,
    pub runtime_directory: PathBuf,
    pub runtime_root_ref: String,
}

impl LocalWorkspacePaths {
    pub fn resolve(cwr_toml: impl AsRef<Path>) -> CwrResult<Self> {
        let state_home = platform_state_home()?;
        let runtime_home = platform_runtime_home()?;
        Self::resolve_with_base_directories(cwr_toml, state_home, runtime_home)
    }

    pub fn resolve_with_base_directories(
        cwr_toml: impl AsRef<Path>,
        state_home: impl AsRef<Path>,
        runtime_home: impl AsRef<Path>,
    ) -> CwrResult<Self> {
        let cwr_toml = canonical_cwr_path(cwr_toml.as_ref())?;
        fs::create_dir_all(state_home.as_ref())?;
        fs::create_dir_all(runtime_home.as_ref())?;
        let workspace_directory = cwr_toml
            .parent()
            .ok_or_else(|| invalid_error("CWR.toml has no containing workspace directory"))?
            .to_owned();
        ensure_workspace_identity_ignored(&workspace_directory)?;
        let identity_directory = workspace_directory.join(".cwr");
        ensure_workspace_identity_directory(&identity_directory, &workspace_directory)?;
        let identity_path = identity_directory.join("workspace.toml");
        let identity = load_or_create_workspace_identity(&identity_path)?;
        let state_root = state_home.as_ref().join(application_directory_name());
        let state_workspaces = state_root.join("workspaces");
        let state_directory = state_workspaces.join(&identity.workspace_id);
        let runtime_root = runtime_home.as_ref().join("cwr");
        let runtime_workspaces = runtime_root.join("workspaces");
        let runtime_directory = runtime_workspaces.join(&identity.workspace_id);
        create_private_directory(&state_root)?;
        create_private_directory(&state_workspaces)?;
        create_private_directory(&state_directory)?;
        create_private_directory(&runtime_root)?;
        create_private_directory(&runtime_workspaces)?;
        create_private_directory(&runtime_directory)?;
        let runtime_root_ref = identity.runtime_root_ref()?;
        Ok(Self {
            cwr_toml,
            workspace_directory,
            identity_path,
            identity,
            state_directory,
            runtime_directory,
            runtime_root_ref,
        })
    }

    /// Inspect an already initialized ordinary workspace without creating an identity,
    /// database, or resident. Missing identities fail closed.
    pub fn inspect_existing(cwr_toml: impl AsRef<Path>) -> CwrResult<Self> {
        let cwr_toml = canonical_cwr_path(cwr_toml.as_ref())?;
        let workspace_directory = cwr_toml.parent().unwrap().to_owned();
        let identity_directory = workspace_directory.join(".cwr");
        if fs::symlink_metadata(&identity_directory)?.file_type().is_symlink() {
            return invalid("workspace identity directory must not be a symlink");
        }
        let mut identity_path = identity_directory.join("workspace.toml");
        if !identity_path.try_exists()? {
            identity_path = identity_directory.join("workspace.json");
        }
        let identity = parse_workspace_identity(&identity_path, &read_workspace_identity(&identity_path)?)?;
        let state_directory = platform_state_home()?.join(application_directory_name())
            .join("workspaces").join(&identity.workspace_id);
        let runtime_directory = platform_runtime_home()?.join("cwr/workspaces").join(&identity.workspace_id);
        let runtime_root_ref = identity.runtime_root_ref()?;
        Ok(Self { cwr_toml, workspace_directory, identity_path, identity,
            state_directory, runtime_directory, runtime_root_ref })
    }

    pub fn database_path(&self) -> PathBuf {
        self.state_directory.join("cwpdb.sqlite3")
    }

    pub fn host_socket_path(&self) -> PathBuf {
        self.runtime_directory.join("host.sock")
    }

    pub fn runtime_socket_path(&self) -> PathBuf {
        self.runtime_directory.join("runtime.sock")
    }

    pub fn resident_pid_path(&self) -> PathBuf {
        self.runtime_directory.join("resident-runtime.pid")
    }

    pub fn binding(&self) -> LocalWorkspaceBinding {
        LocalWorkspaceBinding {
            workspace_id: self.identity.workspace_id.clone(),
            workspace_directory: self.workspace_directory.clone(),
        }
    }
}

pub fn local_runtime_root_ref(cwr_toml: &Path) -> CwrResult<String> {
    Ok(LocalWorkspacePaths::resolve(cwr_toml)?.runtime_root_ref)
}

/// Reconstruct the root used by version 1 workspace identities. Use
/// `LocalWorkspacePaths` to load a workspace's current, persisted runtime root.
pub fn runtime_root_ref_for_workspace_id(workspace_id: &str) -> CwrResult<String> {
    let workspace_id = parse_workspace_id(workspace_id)?;
    Ok(format!(
        "cwp://org.coworkerprotocol.local/runtimes/{workspace_id}"
    ))
}

include!("workspace/composition.rs");
include!("workspace/configured_identity.rs");
include!("workspace/identity.rs");
include!("workspace/hostname.rs");
include!("workspace/ignore.rs");

fn parse_workspace_id(workspace_id: &str) -> CwrResult<String> {
    let parsed = Uuid::parse_str(workspace_id).map_err(|error| {
        invalid_error(format!(
            "invalid CWR workspace_id {workspace_id:?}: {error}"
        ))
    })?;
    Ok(parsed.hyphenated().to_string())
}

fn create_private_directory(path: &Path) -> CwrResult<()> {
    // SAFETY: geteuid reads process credentials and has no preconditions.
    let effective_user_id = unsafe { libc::geteuid() };
    create_or_verify_private_directory(path, effective_user_id, "CWR private directory")
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn application_directory_name() -> &'static str {
    "CWR"
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn application_directory_name() -> &'static str {
    "cwr"
}

fn platform_state_home() -> CwrResult<PathBuf> {
    if let Some(path) = absolute_environment_directory("XDG_STATE_HOME") {
        return Ok(path);
    }
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .ok_or_else(|| io::Error::other("could not determine the local CWR state directory").into())
}

fn platform_runtime_home() -> CwrResult<PathBuf> {
    if let Some(path) = absolute_environment_directory("XDG_RUNTIME_DIR") {
        return Ok(path);
    }
    if let Some(path) = dirs::runtime_dir() {
        return Ok(path);
    }
    // Keep Unix-domain socket paths below the platform limit. macOS's
    // per-user temporary directory is already close to SUN_LEN by itself.
    let mut path = PathBuf::from("/tmp");
    // SAFETY: geteuid reads process credentials and has no preconditions.
    let effective_user_id = unsafe { libc::geteuid() };
    path.push(format!("cwr-{effective_user_id}"));
    create_or_verify_private_runtime_home(&path, effective_user_id)?;
    Ok(path)
}

include!("workspace/platform.rs");

fn create_or_verify_private_runtime_home(
    path: &Path,
    effective_user_id: libc::uid_t,
) -> CwrResult<()> {
    create_or_verify_private_directory(path, effective_user_id, "CWR runtime fallback")
}

fn create_or_verify_private_directory(
    path: &Path,
    effective_user_id: libc::uid_t,
    label: &str,
) -> CwrResult<()> {
    use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _};

    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    match builder.create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    let mut metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir() || metadata.uid() != effective_user_id {
        return invalid(format!(
            "{label} {} must be a private directory owned by user {effective_user_id}",
            path.display()
        ));
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        metadata = fs::symlink_metadata(path)?;
    }
    if !metadata.file_type().is_dir()
        || metadata.uid() != effective_user_id
        || metadata.permissions().mode() & 0o077 != 0
    {
        return invalid(format!(
            "{label} {} must be a private directory owned by user {effective_user_id}",
            path.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod workspace_tests {
    use super::*;

    fn workspace(root: &Path) -> PathBuf {
        let path = root.join("workspace/CWR.toml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "version = '2026-09'\n").unwrap();
        path
    }

    #[test]
    fn workspace_identity_is_stable_and_state_is_machine_local() {
        let temporary = tempfile::tempdir().unwrap();
        let manifest = workspace(temporary.path());
        let first = LocalWorkspacePaths::resolve_with_base_directories(
            &manifest,
            temporary.path().join("state"),
            temporary.path().join("runtime"),
        )
        .unwrap();
        let second = LocalWorkspacePaths::resolve_with_base_directories(
            &manifest,
            temporary.path().join("other-state"),
            temporary.path().join("other-runtime"),
        )
        .unwrap();

        assert_eq!(first.identity, second.identity);
        assert_ne!(first.state_directory, second.state_directory);
        assert!(first.identity_path.ends_with(".cwr/workspace.toml"));
        assert!(first.database_path().ends_with("cwpdb.sqlite3"));
        assert!(first.host_socket_path().ends_with("host.sock"));
        assert_eq!(
            fs::read_to_string(first.workspace_directory.join(".gitignore")).unwrap(),
            ".cwr/\n"
        );
    }

    #[test]
    fn inspect_existing_never_initializes_a_missing_identity() {
        let temporary = tempfile::tempdir().unwrap();
        let manifest = workspace(temporary.path());
        assert!(LocalWorkspacePaths::inspect_existing(&manifest).is_err());
        assert!(!manifest.parent().unwrap().join(".cwr").exists());
        assert!(!manifest.parent().unwrap().join(".gitignore").exists());
    }

    #[test]
    fn concurrent_first_open_publishes_one_complete_identity() {
        let temporary = tempfile::tempdir().unwrap();
        let manifest = workspace(temporary.path());
        let state_home = temporary.path().join("state");
        let runtime_home = temporary.path().join("runtime");
        let threads = (0..8)
            .map(|_| {
                let manifest = manifest.clone();
                let state_home = state_home.clone();
                let runtime_home = runtime_home.clone();
                thread::spawn(move || {
                    LocalWorkspacePaths::resolve_with_base_directories(
                        manifest,
                        state_home,
                        runtime_home,
                    )
                    .unwrap()
                    .identity
                })
            })
            .collect::<Vec<_>>();
        let identities = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>();

        assert!(identities.iter().all(|identity| identity == &identities[0]));
        assert_eq!(
            parse_workspace_identity(
                &manifest.parent().unwrap().join(".cwr/workspace.toml"),
                &fs::read(manifest.parent().unwrap().join(".cwr/workspace.toml")).unwrap(),
            )
            .unwrap(),
            identities[0]
        );
    }

    #[test]
    fn invalid_workspace_identity_fails_without_replacement() {
        let temporary = tempfile::tempdir().unwrap();
        let manifest = workspace(temporary.path());
        let identity_path = manifest.parent().unwrap().join(".cwr/workspace.toml");
        fs::create_dir_all(identity_path.parent().unwrap()).unwrap();
        fs::write(&identity_path, b"not json").unwrap();

        assert!(LocalWorkspacePaths::resolve_with_base_directories(
            &manifest,
            temporary.path().join("state"),
            temporary.path().join("runtime"),
        )
        .is_err());
        assert_eq!(fs::read(identity_path).unwrap(), b"not json");
    }

    #[test]
    fn copied_workspace_is_rejected_while_the_original_path_exists() {
        let temporary = tempfile::tempdir().unwrap();
        let state_home = temporary.path().join("state");
        let runtime_home = temporary.path().join("runtime");
        let original_manifest = workspace(temporary.path());
        let original = LocalWorkspacePaths::resolve_with_base_directories(
            &original_manifest,
            &state_home,
            &runtime_home,
        )
        .unwrap();
        ConversationAdmissionStore::open_bound(
            original.database_path(),
            &original.runtime_root_ref,
            &original.identity.workspace_id,
            &original.workspace_directory,
        )
        .unwrap();

        let copy_directory = temporary.path().join("copy");
        fs::create_dir_all(copy_directory.join(".cwr")).unwrap();
        fs::copy(&original.cwr_toml, copy_directory.join("CWR.toml")).unwrap();
        fs::copy(
            &original.identity_path,
            copy_directory.join(".cwr/workspace.toml"),
        )
        .unwrap();
        let copied = LocalWorkspacePaths::resolve_with_base_directories(
            copy_directory.join("CWR.toml"),
            &state_home,
            &runtime_home,
        )
        .unwrap();

        let error = ConversationAdmissionStore::open_bound(
            copied.database_path(),
            &copied.runtime_root_ref,
            &copied.identity.workspace_id,
            &copied.workspace_directory,
        )
        .err()
        .expect("copied workspace should be rejected");
        assert!(error
            .to_string()
            .contains("appears to be a copied workspace"));
    }

    #[test]
    fn moved_workspace_rebinds_when_the_old_path_no_longer_exists() {
        let temporary = tempfile::tempdir().unwrap();
        let state_home = temporary.path().join("state");
        let runtime_home = temporary.path().join("runtime");
        let original_manifest = workspace(temporary.path());
        let original = LocalWorkspacePaths::resolve_with_base_directories(
            &original_manifest,
            &state_home,
            &runtime_home,
        )
        .unwrap();
        ConversationAdmissionStore::open_bound(
            original.database_path(),
            &original.runtime_root_ref,
            &original.identity.workspace_id,
            &original.workspace_directory,
        )
        .unwrap();

        let moved_directory = temporary.path().join("moved");
        fs::rename(&original.workspace_directory, &moved_directory).unwrap();
        let moved = LocalWorkspacePaths::resolve_with_base_directories(
            moved_directory.join("CWR.toml"),
            &state_home,
            &runtime_home,
        )
        .unwrap();

        ConversationAdmissionStore::open_bound(
            moved.database_path(),
            &moved.runtime_root_ref,
            &moved.identity.workspace_id,
            &moved.workspace_directory,
        )
        .unwrap();
    }

    #[test]
    fn inaccessible_original_workspace_does_not_rebind() {
        let original_root = tempfile::tempdir().unwrap();
        let local_root = tempfile::tempdir().unwrap();
        let copy_root = tempfile::tempdir().unwrap();
        let state_home = local_root.path().join("state");
        let runtime_home = local_root.path().join("runtime");
        let original_manifest = workspace(original_root.path());
        let original = LocalWorkspacePaths::resolve_with_base_directories(
            &original_manifest,
            &state_home,
            &runtime_home,
        )
        .unwrap();
        ConversationAdmissionStore::open_bound(
            original.database_path(),
            &original.runtime_root_ref,
            &original.identity.workspace_id,
            &original.workspace_directory,
        )
        .unwrap();
        let copied_directory = copy_root.path().join("workspace");
        fs::create_dir_all(copied_directory.join(".cwr")).unwrap();
        fs::copy(&original.cwr_toml, copied_directory.join("CWR.toml")).unwrap();
        fs::copy(
            &original.identity_path,
            copied_directory.join(".cwr/workspace.toml"),
        )
        .unwrap();
        let copied = LocalWorkspacePaths::resolve_with_base_directories(
            copied_directory.join("CWR.toml"),
            &state_home,
            &runtime_home,
        )
        .unwrap();

        fs::set_permissions(original_root.path(), fs::Permissions::from_mode(0o000)).unwrap();
        let inaccessible = ConversationAdmissionStore::open_bound(
            copied.database_path(),
            &copied.runtime_root_ref,
            &copied.identity.workspace_id,
            &copied.workspace_directory,
        )
        .err()
        .expect("an inaccessible original binding must fail closed");
        fs::set_permissions(original_root.path(), fs::Permissions::from_mode(0o700)).unwrap();

        assert!(!inaccessible
            .to_string()
            .contains("appears to be a copied workspace"));
        let still_bound = ConversationAdmissionStore::open_bound(
            copied.database_path(),
            &copied.runtime_root_ref,
            &copied.identity.workspace_id,
            &copied.workspace_directory,
        )
        .err()
        .expect("the failed check must not update the binding");
        assert!(still_bound
            .to_string()
            .contains("appears to be a copied workspace"));
    }

    #[test]
    fn runtime_fallback_is_created_private_and_owned() {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("fallback");
        // SAFETY: geteuid reads process credentials and has no preconditions.
        let effective_user_id = unsafe { libc::geteuid() };

        create_or_verify_private_runtime_home(&path, effective_user_id).unwrap();

        let metadata = fs::symlink_metadata(path).unwrap();
        assert!(metadata.is_dir());
        assert_eq!(metadata.uid(), effective_user_id);
        assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
    }

    #[test]
    fn runtime_fallback_repairs_an_owned_existing_directory() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("fallback");
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        // SAFETY: geteuid reads process credentials and has no preconditions.
        let effective_user_id = unsafe { libc::geteuid() };

        create_or_verify_private_runtime_home(&path, effective_user_id).unwrap();

        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn runtime_fallback_rejects_a_symlink() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().unwrap();
        let target = temporary.path().join("target");
        let path = temporary.path().join("fallback");
        fs::create_dir(&target).unwrap();
        symlink(target, &path).unwrap();
        // SAFETY: geteuid reads process credentials and has no preconditions.
        let effective_user_id = unsafe { libc::geteuid() };

        let error = create_or_verify_private_runtime_home(&path, effective_user_id)
            .expect_err("a symlink fallback must fail closed");

        assert!(error.to_string().contains("must be a private directory"));
    }
}
