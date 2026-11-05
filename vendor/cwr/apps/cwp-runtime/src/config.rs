use std::{
    fs,
    num::NonZeroUsize,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

use serde::Deserialize;

use cwr::CwrResult;

const DEFAULT_MAX_CONCURRENT_CONVERSATIONS: usize = 16;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeConfigFile {
    version: u32,
    workspace: PathBuf,
    #[serde(default)]
    state_directory: Option<PathBuf>,
    #[serde(default)]
    runtime_directory: Option<PathBuf>,
    #[serde(default)]
    workspace_id: Option<String>,
    #[serde(default)]
    runtime_root_ref: Option<String>,
    #[serde(default)]
    acting_actor_ref: Option<String>,
    #[serde(default)]
    socket_path: Option<PathBuf>,
    #[serde(default)]
    host_socket_path: Option<PathBuf>,
    #[serde(default)]
    communication_adapters: Vec<ProcessCommunicationAdapterFile>,
    #[serde(default)]
    expected_communication_adapters: Option<usize>,
    #[serde(default)]
    communication_adapter_directory: Option<PathBuf>,
    #[serde(default = "default_max_concurrent_conversations")]
    max_concurrent_conversations: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProcessCommunicationAdapterFile {
    command: PathBuf,
    #[serde(default)]
    args: Vec<String>,
}

pub(crate) struct ProcessCommunicationAdapter {
    pub(crate) command: PathBuf,
    pub(crate) args: Vec<String>,
}

pub(crate) struct RuntimeConfig {
    pub(crate) configured_runtime: Option<cwr::v2026_09::local::PreparedConfiguredRuntime>,
    pub(crate) workspace: PathBuf,
    pub(crate) state_directory: PathBuf,
    pub(crate) runtime_directory: PathBuf,
    pub(crate) workspace_id: Option<String>,
    pub(crate) runtime_root_ref: String,
    pub(crate) acting_actor_ref: Option<String>,
    pub(crate) socket_path: PathBuf,
    pub(crate) host_socket_path: PathBuf,
    pub(crate) config_directory: PathBuf,
    pub(crate) communication_adapters: Vec<ProcessCommunicationAdapter>,
    pub(crate) expected_communication_adapters: usize,
    pub(crate) max_concurrent_conversations: NonZeroUsize,
}

/// Select the existing ordinary resident using the same input as host startup.
/// This does not open a database, create an identity, or launch a resident.
pub(crate) fn inspect_existing_runtime(input: &Path) -> CwrResult<()> {
    let input = fs::canonicalize(input)?;
    let workspace = if input.file_name().is_some_and(|name| name == "CWR.toml") {
        input.clone()
    } else {
        let file: RuntimeConfigFile = toml::from_str(&fs::read_to_string(&input)?)?;
        if file.version != 1
            || file.state_directory.is_some()
            || file.runtime_directory.is_some()
            || file.workspace_id.is_some()
            || file.runtime_root_ref.is_some()
            || file.socket_path.is_some()
        {
            return Err(std::io::Error::other(
                "SMS ordinary selection requires workspace-owned state and socket",
            )
            .into());
        }
        absolute(input.parent().unwrap(), file.workspace)
    };
    let paths = cwr::v2026_09::local::LocalWorkspacePaths::inspect_existing(workspace)?;
    println!(
        "{}",
        serde_json::json!({
            "workspace_path": paths.workspace_directory,
            "runtime_socket_path": paths.runtime_socket_path(),
            "database_path": paths.database_path(),
            "runtime_root_ref": paths.runtime_root_ref,
        })
    );
    Ok(())
}

impl RuntimeConfig {
    pub(crate) fn load(path: PathBuf) -> CwrResult<Self> {
        Self::load_with_workspace_paths(path, |workspace| {
            cwr::v2026_09::local::LocalWorkspacePaths::resolve(workspace)
        })
    }

    fn load_with_workspace_paths(
        path: PathBuf,
        resolve_workspace_paths: impl FnOnce(
            &Path,
        )
            -> CwrResult<cwr::v2026_09::local::LocalWorkspacePaths>,
    ) -> CwrResult<Self> {
        let path = fs::canonicalize(path)?;
        let directory = path.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "runtime config has no parent directory",
            )
        })?;
        let file: RuntimeConfigFile = toml::from_str(&fs::read_to_string(&path)?)?;
        if file.version != 1 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unsupported runtime config version {}", file.version),
            )
            .into());
        }
        let workspace = absolute(directory, file.workspace);
        let (workspace, state_directory, runtime_directory, workspace_id, runtime_root_ref) =
            match file.state_directory {
                Some(state_directory) => {
                    let state_directory = absolute(directory, state_directory);
                    fs::create_dir_all(&state_directory)?;
                    let state_directory = fs::canonicalize(state_directory)?;
                    make_private_directory(&state_directory, "runtime state directory")?;
                    let runtime_directory = file
                        .runtime_directory
                        .map_or_else(|| state_directory.clone(), |path| absolute(directory, path));
                    fs::create_dir_all(&runtime_directory)?;
                    let runtime_directory = fs::canonicalize(runtime_directory)?;
                    make_private_directory(&runtime_directory, "runtime socket directory")?;
                    let runtime_root_ref = file.runtime_root_ref.ok_or_else(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "runtime config with state_directory must declare runtime_root_ref",
                        )
                    })?;
                    (
                        workspace,
                        state_directory,
                        runtime_directory,
                        file.workspace_id,
                        runtime_root_ref,
                    )
                }
                None => {
                    if file.runtime_directory.is_some()
                        || file.workspace_id.is_some()
                        || file.runtime_root_ref.is_some()
                    {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "runtime_directory, workspace_id, and runtime_root_ref require an explicit state_directory",
                        )
                        .into());
                    }
                    let paths = resolve_workspace_paths(&workspace)?;
                    (
                        paths.cwr_toml,
                        paths.state_directory,
                        paths.runtime_directory,
                        Some(paths.identity.workspace_id),
                        paths.runtime_root_ref,
                    )
                }
            };
        let socket_path = file.socket_path.map_or_else(
            || runtime_directory.join("runtime.sock"),
            |path| absolute(directory, path),
        );
        let host_socket_path = file.host_socket_path.map_or_else(
            || runtime_directory.join("host.sock"),
            |path| absolute(directory, path),
        );
        if host_socket_path == socket_path {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "host_socket_path must differ from the private runtime socket_path",
            )
            .into());
        }
        let communication_adapters: Vec<ProcessCommunicationAdapter> = file
            .communication_adapters
            .into_iter()
            .map(|adapter| ProcessCommunicationAdapter {
                command: adapter.command,
                args: adapter.args,
            })
            .collect();
        let expected_communication_adapters = file
            .expected_communication_adapters
            .unwrap_or(communication_adapters.len());
        if expected_communication_adapters < communication_adapters.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "expected_communication_adapters cannot be smaller than the configured process adapters",
            )
            .into());
        }
        let max_concurrent_conversations = NonZeroUsize::new(file.max_concurrent_conversations)
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "max_concurrent_conversations must be greater than zero",
                )
            })?;
        let config_directory = file
            .communication_adapter_directory
            .map_or_else(|| directory.to_owned(), |path| absolute(directory, path));
        Ok(Self {
            configured_runtime: None,
            workspace,
            state_directory,
            runtime_directory,
            workspace_id,
            runtime_root_ref,
            acting_actor_ref: file.acting_actor_ref,
            socket_path,
            host_socket_path,
            config_directory,
            communication_adapters,
            expected_communication_adapters,
            max_concurrent_conversations,
        })
    }

    pub(crate) fn configured_actor_ref(&self) -> CwrResult<&str> {
        self.acting_actor_ref.as_deref().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "runtime config must declare acting_actor_ref unless --host-stdio is used",
            )
            .into()
        })
    }

    pub(crate) fn workspace_paths(&self) -> CwrResult<cwr::v2026_09::local::LocalWorkspacePaths> {
        cwr::v2026_09::local::LocalWorkspacePaths::resolve(&self.workspace)
    }

    pub(crate) fn workspace_binding(
        &self,
    ) -> CwrResult<Option<cwr::v2026_09::local::LocalWorkspaceBinding>> {
        let Some(workspace_id) = &self.workspace_id else {
            return Ok(None);
        };
        let workspace = fs::canonicalize(&self.workspace)?;
        let workspace_directory = if workspace.is_dir() {
            workspace
        } else {
            workspace
                .parent()
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "runtime workspace has no containing directory",
                    )
                })?
                .to_owned()
        };
        Ok(Some(cwr::v2026_09::local::LocalWorkspaceBinding {
            workspace_id: workspace_id.clone(),
            workspace_directory,
        }))
    }
}

fn default_max_concurrent_conversations() -> usize {
    DEFAULT_MAX_CONCURRENT_CONVERSATIONS
}

fn absolute(base: &std::path::Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

fn make_private_directory(path: &Path, label: &str) -> CwrResult<()> {
    let metadata = fs::symlink_metadata(path)?;
    // SAFETY: geteuid reads process credentials and has no preconditions.
    let effective_user_id = unsafe { libc::geteuid() };
    if !metadata.is_dir() || metadata.uid() != effective_user_id {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "{label} {} must be owned by user {effective_user_id}",
                path.display()
            ),
        )
        .into());
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load_config(limit: Option<usize>) -> CwrResult<RuntimeConfig> {
        let temporary = tempfile::tempdir()?;
        let path = temporary.path().join("runtime.toml");
        let limit = limit.map_or_else(String::new, |limit| {
            format!("max_concurrent_conversations = {limit}\n")
        });
        fs::write(
            &path,
            format!(
                "version = 1\nworkspace = '.'\nstate_directory = './state'\nruntime_root_ref = 'cwp://example.test/runtime'\n{limit}"
            ),
        )?;
        RuntimeConfig::load(path)
    }

    #[test]
    fn concurrent_conversation_limit_defaults_to_sixteen_and_validates_overrides() {
        assert_eq!(
            load_config(None)
                .unwrap()
                .max_concurrent_conversations
                .get(),
            16
        );
        assert_eq!(
            load_config(Some(32))
                .unwrap()
                .max_concurrent_conversations
                .get(),
            32
        );
        assert!(load_config(Some(0)).is_err());
    }

    #[test]
    fn omitted_storage_uses_workspace_identity_and_platform_roots() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        fs::write(workspace.join("CWR.toml"), "version = '2026-09'\n").unwrap();
        let config_path = temporary.path().join("runtime.toml");
        fs::write(
            &config_path,
            "version = 1\nworkspace = './workspace/CWR.toml'\n",
        )
        .unwrap();
        let state_home = temporary.path().join("state-home");
        let runtime_home = temporary.path().join("runtime-home");

        let config = RuntimeConfig::load_with_workspace_paths(config_path, |manifest| {
            cwr::v2026_09::local::LocalWorkspacePaths::resolve_with_base_directories(
                manifest,
                &state_home,
                &runtime_home,
            )
        })
        .unwrap();

        assert_eq!(
            config.workspace,
            fs::canonicalize(workspace.join("CWR.toml")).unwrap()
        );
        assert!(config.state_directory.starts_with(&state_home));
        assert!(config
            .state_directory
            .ends_with(Path::new("workspaces").join(config.workspace_id.as_deref().unwrap())));
        assert!(config
            .runtime_directory
            .starts_with(runtime_home.join("cwr/workspaces")));
        let reference = cwr::CwrRef::parse(&config.runtime_root_ref).unwrap();
        assert!(config.runtime_root_ref.starts_with("cwp://local."));
        assert_eq!(
            reference.segments(),
            ["runtimes", config.workspace_id.as_deref().unwrap()]
        );
    }

    #[test]
    fn explicit_runtime_directories_are_owner_only() {
        let temporary = tempfile::tempdir().unwrap();
        let state = temporary.path().join("state");
        let runtime = temporary.path().join("runtime");
        fs::create_dir_all(&state).unwrap();
        fs::create_dir_all(&runtime).unwrap();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o777)).unwrap();
        let config_path = temporary.path().join("runtime.toml");
        fs::write(
            &config_path,
            "version = 1\nworkspace = '.'\nstate_directory = './state'\nruntime_directory = './runtime'\nruntime_root_ref = 'cwp://example.test/runtime'\n",
        )
        .unwrap();

        RuntimeConfig::load(config_path).unwrap();

        assert_eq!(
            fs::metadata(state).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(runtime).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
}
