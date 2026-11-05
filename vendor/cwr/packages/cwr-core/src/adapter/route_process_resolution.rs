// Resolve a selected route to a local adapter process and its environment.

struct LocalAdapterResolution {
    process: ResolvedProcessAdapter,
    integration_params: Option<Value>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProviderStore {
    Managed,
    Shared,
    System,
    Legacy,
}

// Retained provider IDs can only be resumed from the home where they were made.
// Older state and checkpoints have no marker; current system-home sessions use
// an explicit marker so they cannot be confused with that migration boundary.
fn provider_store(invocation: &AdapterInvocation) -> CwrResult<ProviderStore> {
    let working_state = working_context(&invocation.thread).unwrap_or(&invocation.thread)
        ["working_conversation"]
        .get("state")
        .filter(|state| !state.is_null());
    let retained_store = |state: &Value| -> CwrResult<ProviderStore> {
        match state.get("provider_store") {
            None => Ok(ProviderStore::Legacy),
            Some(Value::String(value)) if value == "managed" => Ok(ProviderStore::Managed),
            Some(Value::String(value)) if value == "shared" => Ok(ProviderStore::Shared),
            Some(Value::String(value)) if value == "system" => Ok(ProviderStore::System),
            _ => protocol_failure("Provider history has an unsupported store"),
        }
    };
    let working_store = working_state.map(retained_store).transpose()?;
    let checkpoint_store = invocation
        .continuation
        .as_ref()
        .map(retained_store)
        .transpose()?;
    if let (Some(working), Some(checkpoint)) = (working_store, checkpoint_store) {
        if working != checkpoint {
            return protocol_failure("Provider history and continuation use different stores");
        }
    }
    Ok(checkpoint_store
        .or(working_store)
        .unwrap_or(ProviderStore::Managed))
}

// Existing managed/shared conversations retain the home where their provider
// thread was created. New Codex conversations use the installed CLI's normal
// home so account, configuration, plugins, and conversation storage stay with
// system Codex.
fn codex_provider_store(invocation: &AdapterInvocation) -> CwrResult<ProviderStore> {
    let has_working = working_context(&invocation.thread).unwrap_or(&invocation.thread)
        ["working_conversation"]
        .get("state")
        .is_some_and(|state| !state.is_null());
    if !has_working && invocation.continuation.is_none() {
        return Ok(ProviderStore::System);
    }
    provider_store(invocation)
}

fn managed_provider_home(provider: &str) -> CwrResult<PathBuf> {
    let root = match std::env::var_os("WORKLOOP_STATE_HOME") {
        Some(root) if !root.is_empty() => PathBuf::from(root),
        _ => crate::workspace_environment::product_state_directory()?,
    };
    managed_provider_home_at(&root, provider)
}

fn managed_provider_home_at(root: &Path, provider: &str) -> CwrResult<PathBuf> {
    use std::os::unix::fs::MetadataExt as _;
    if !root.is_absolute() {
        return protocol_failure("Workloop state home must be an absolute path");
    }
    let providers = root.join("provider-homes");
    let home = providers.join(provider);
    // SAFETY: geteuid reads process credentials and has no preconditions.
    let effective_user_id = unsafe { libc::geteuid() };
    for directory in [&providers, &home] {
        std::fs::create_dir_all(directory)?;
        let metadata = std::fs::symlink_metadata(directory)?;
        // These directories hold provider credentials as well as sessions.
        if !metadata.is_dir() || metadata.uid() != effective_user_id {
            return protocol_failure("Workloop provider home is not a private directory");
        }
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(home)
}

fn resolve_local_adapter(
    route: &ResolvedProcessAdapterRoute,
    invocation: &AdapterInvocation,
    allow_host_credentials: bool,
) -> CwrResult<LocalAdapterResolution> {
    if route.actor_integration_params["execution_environment"] == "workloop_cloud" {
        return resolve_cloud_route(route, invocation);
    }
    if route.adapter_ref == CLAUDE_ADAPTER_REF {
        return resolve_claude_adapter(route, provider_store(invocation)?);
    }
    if route.adapter_ref == CODEX_ADAPTER_REF {
        let home = std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .ok_or_else(|| protocol_error("Codex adapter requires HOME"))?;
        let path = std::env::var_os("PATH").filter(|value| !value.is_empty());
        let codex_executable =
            codex_executable(std::env::var_os("CWR_CODEX_EXECUTABLE"), path.as_deref())
                .ok_or_else(|| protocol_error("Codex executable is missing or is not executable"))?
                .canonicalize()?;
        let granted_workspace = std::fs::canonicalize(&route.working_path)?;
        let mut process = ResolvedProcessAdapter::new(sibling_adapter_executable(
            "CWR_CODEX_ADAPTER_EXECUTABLE",
            "cwr-adapter-codex",
        )?)
        .with_current_directory(&route.working_path)
        .with_environment("CWR_COWORKER_DIRECTORY", &route.coworker_directory)
        .with_environment("CWR_CODEX_DANGER_FULL_ACCESS_WORKSPACE", granted_workspace)
        .with_environment("HOME", home)
        .with_environment("PATH", path.unwrap_or_default());
        match codex_provider_store(invocation)? {
            ProviderStore::Managed => {
                process = process
                    .with_environment("CODEX_HOME", managed_provider_home("codex")?)
                    .with_environment("CWR_PROVIDER_STORE", "managed");
            }
            ProviderStore::Shared => {
                process = process
                    .with_environment("CWR_PROVIDER_STORE", "shared")
                    .with_environment(
                        "CWR_ADAPTER_DATA_DIRECTORY",
                        adapter_data_directory("codex")?,
                    );
                for name in [
                    "CWR_ADAPTER_DATA_ROOT",
                    "CWR_CODEX_SOURCE_HOME",
                    "CODEX_HOME",
                ] {
                    if let Some(value) = std::env::var_os(name) {
                        process = process.with_environment(name, value);
                    }
                }
            }
            ProviderStore::System => {
                process = process.with_environment("CWR_PROVIDER_STORE", "system");
            }
            ProviderStore::Legacy => {}
        }
        if let Some(path) = std::env::var_os("CWR_CODEX_SERVICE_TIER_FILE") {
            process = process.with_environment("CWR_CODEX_SERVICE_TIER_FILE", path);
        }
        if let Some(user) = std::env::var_os("USER") {
            process = process.with_environment("USER", user);
        }
        return Ok(LocalAdapterResolution {
            process,
            integration_params: Some(json!({
                "codex_executable": codex_executable,
            })),
        });
    }
    if route.adapter_ref == DECISIONS_ADAPTER_REF {
        let mut process = ResolvedProcessAdapter::new(sibling_adapter_executable(
            "CWR_DECISIONS_ADAPTER_EXECUTABLE",
            "cwr-adapter-decisions",
        )?)
        .with_current_directory(&route.working_path);
        let key = decisions_api_key(&route.integration_params, allow_host_credentials, || {
            std::env::var_os("OPENROUTER_API_KEY")
        });
        if let Some(key) = key {
            process = process.with_environment("DECISIONS_API_KEY", key);
        }
        return Ok(LocalAdapterResolution {
            process,
            integration_params: None,
        });
    }
    if route.adapter_ref == OPEN_RESPONSES_ADAPTER_REF {
        let mut process = ResolvedProcessAdapter::new(sibling_adapter_executable(
            "CWR_OPEN_RESPONSES_ADAPTER_EXECUTABLE",
            "cwr-adapter-open-responses",
        )?)
        .with_current_directory(&route.working_path)
        .with_environment("CWR_COWORKER_DIRECTORY", &route.coworker_directory);
        if let Some(api_key) = input_slots::open_responses_key(route, allow_host_credentials) {
            process = process.with_environment("OPEN_RESPONSES_API_KEY", api_key);
        }
        return Ok(LocalAdapterResolution {
            process,
            integration_params: None,
        });
    }
    if route.adapter_ref == LOOPBACK_ADAPTER_REF {
        return Ok(LocalAdapterResolution {
            process: ResolvedProcessAdapter::new(sibling_adapter_executable(
                "CWR_LOOPBACK_ADAPTER_EXECUTABLE",
                "cwr-adapter-loopback",
            )?)
            .with_current_directory(&route.working_path),
            integration_params: None,
        });
    }
    if route.adapter_ref == RNG_ADAPTER_REF {
        return Ok(LocalAdapterResolution {
            process: ResolvedProcessAdapter::new(sibling_adapter_executable(
                "CWR_RNG_ADAPTER_EXECUTABLE",
                "cwr-adapter-rng",
            )?)
            .with_current_directory(&route.working_path),
            integration_params: None,
        });
    }
    if route.adapter_ref == SH_ADAPTER_REF {
        if route
            .actor_integration_params
            .get("execution_environment")
            .is_some()
        {
            let (smolvm, image, image_sha256) = resolve_smolvm_shell_route(route)?;
            let process = ResolvedProcessAdapter::new(sibling_adapter_executable(
                "CWR_SMOLVM_SH_ADAPTER_EXECUTABLE",
                "cwr-adapter-smolvm-sh",
            )?)
            // A workspace-local Smolfile must not change host VM creation.
            .with_current_directory("/")
            .with_environment("CWR_SMOLVM_EXECUTABLE", smolvm)
            .with_environment("CWR_SMOLVM_IMAGE", image)
            .with_environment("CWR_SMOLVM_IMAGE_SHA256", image_sha256)
            .with_environment(
                "HOME",
                std::env::var_os("HOME")
                    .filter(|home| !home.is_empty())
                    .ok_or_else(|| protocol_error("SmolVM needs a host home directory"))?,
            )
            .with_environment("PATH", std::env::var_os("PATH").unwrap_or_default())
            .with_shutdown_grace_period(Duration::from_secs(60));
            return Ok(LocalAdapterResolution {
                process,
                integration_params: None,
            });
        }
        let mut process = resolved_shell_adapter(
            sibling_adapter_executable("CWR_SH_ADAPTER_EXECUTABLE", "cwr-adapter-sh")?,
            &route.working_path,
        );
        if let Some(socket) = shell_host_environment_for_action(
            &route.actor_integration_params,
            &invocation.action_name,
            |name| std::env::var_os(name),
        )? {
            process = process.with_environment(WORKLOOP_SMS_BRIDGE_SOCKET_ENV, socket);
        }
        // A shell coworker can invoke another CWR action without requiring a
        // separately installed CLI. Desktop and the local runtime bundle cwr
        // beside cwp-runtime; pass that exact executable through the adapter's
        // otherwise reduced environment.
        if let Some(directory) = std::env::current_exe()?.parent() {
            let control = directory.join("cwr");
            if is_executable_file(&control) {
                process = process.with_environment("CWR_CONTROL_EXECUTABLE", control);
            }
        }
        for name in ["HOME", "PATH", "TMPDIR", "LANG", "LC_ALL"] {
            if let Some(value) = std::env::var_os(name) {
                process = process.with_environment(name, value);
            }
        }
        return Ok(LocalAdapterResolution {
            process,
            integration_params: None,
        });
    }
    protocol_failure(format!(
        "the local host has no process implementation for adapter {}",
        route.adapter_ref
    ))
}

include!("route_smolvm.rs");
include!("route_cloud.rs");

const WORKLOOP_SMS_BRIDGE_SOCKET_ENV: &str = "WORKLOOP_SMS_BRIDGE_SOCKET";
const WORKLOOP_SMS_ACTIONS: [&str; 3] = ["receive--message", "deliver--message", "handle--request"];

fn shell_host_environment_for_action(
    actor_params: &Value,
    action_name: &str,
    host_env: impl FnOnce(&str) -> Option<std::ffi::OsString>,
) -> CwrResult<Option<std::ffi::OsString>> {
    let Some(configured) = actor_params.get("host_env_actions") else {
        return Ok(None);
    };
    let Some(actions) = configured.as_object() else {
        return protocol_failure("Shell host_env_actions must be an action map");
    };
    if actions.iter().any(|(action, name)| {
        !WORKLOOP_SMS_ACTIONS.contains(&action.as_str())
            || name.as_str() != Some(WORKLOOP_SMS_BRIDGE_SOCKET_ENV)
    }) {
        return protocol_failure(
            "Shell host_env_actions contains an unsupported action or host value",
        );
    }
    if !actions.contains_key(action_name) {
        return Ok(None);
    }
    let socket = host_env(WORKLOOP_SMS_BRIDGE_SOCKET_ENV)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| protocol_error("Workloop SMS bridge is unavailable"))?;
    if !Path::new(&socket).is_absolute() {
        return protocol_failure("Workloop SMS bridge path must be absolute");
    }
    Ok(Some(socket))
}

fn resolved_shell_adapter(executable: PathBuf, working_path: &Path) -> ResolvedProcessAdapter {
    ResolvedProcessAdapter::new(executable)
        .with_current_directory(working_path)
        .with_shutdown_grace_period(Duration::from_secs(2))
}

fn has_local_process_implementation(adapter_ref: &str) -> bool {
    adapter_ref == CODEX_ADAPTER_REF
        || adapter_ref == CLAUDE_ADAPTER_REF
        || adapter_ref == DECISIONS_ADAPTER_REF
        || adapter_ref == OPEN_RESPONSES_ADAPTER_REF
        || adapter_ref == LOOPBACK_ADAPTER_REF
        || adapter_ref == RNG_ADAPTER_REF
        || adapter_ref == SH_ADAPTER_REF
        || adapter_ref == crate::action_contract::INTRINSIC_CWR_ADAPTER_REF
}

fn find_path_executable(name: &str) -> CwrResult<PathBuf> {
    let path = std::env::var_os("PATH")
        .ok_or_else(|| protocol_error(format!("{name} executable needs PATH")))?;
    find_executable_on_path(name, &path)
        .ok_or_else(|| protocol_error(format!("{name} executable was not found on PATH")))
}

fn find_executable_on_path(name: &str, path: &OsStr) -> Option<PathBuf> {
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(name);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_executable_file(candidate: &Path) -> bool {
    candidate
        .metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

fn sibling_adapter_executable(environment_name: &str, name: &str) -> CwrResult<PathBuf> {
    let candidate = match std::env::var_os(environment_name).map(PathBuf::from) {
        Some(candidate) => candidate,
        None => {
            let current = std::env::current_exe()?;
            let sibling = current.with_file_name(name);
            if sibling.exists() {
                sibling
            } else if cfg!(test)
                && current.parent().and_then(Path::file_name) == Some(std::ffi::OsStr::new("deps"))
            {
                current
                    .parent()
                    .and_then(Path::parent)
                    .ok_or_else(|| protocol_error("test executable has no target directory"))?
                    .join(name)
            } else {
                sibling
            }
        }
    };
    let candidate = std::fs::canonicalize(&candidate).map_err(|error| {
        protocol_error(format!(
            "resolved adapter executable {} is unavailable: {error}",
            candidate.display()
        ))
    })?;
    if !is_executable_file(&candidate) {
        return protocol_failure(format!(
            "resolved adapter executable {} is not an executable regular file",
            candidate.display()
        ));
    }
    Ok(candidate)
}

// An ambient host credential authorizes OpenRouter only. Explicit private input
// keys instead authorize the integration's selected endpoint.
fn decisions_api_key(
    params: &Value,
    allow_host_credentials: bool,
    host_key: impl FnOnce() -> Option<OsString>,
) -> Option<OsString> {
    let endpoint = decisions_endpoint(params.get("endpoint")?.as_str()?).ok()?;
    if let Some(key) = params
        .get("api_key")
        .and_then(Value::as_str)
        .filter(|key| !key.trim().is_empty())
    {
        return Some(OsString::from(key));
    }
    let trusted_origin = endpoint.scheme() == "https"
        && endpoint.host_str() == Some("openrouter.ai")
        && endpoint.port_or_known_default() == Some(443)
        && endpoint.username().is_empty()
        && endpoint.password().is_none();
    (allow_host_credentials && trusted_origin)
        .then(host_key)
        .flatten()
        .filter(|key| !key.to_string_lossy().trim().is_empty())
}

fn decisions_loopback_endpoint(params: &Value) -> bool {
    params
        .get("endpoint")
        .and_then(Value::as_str)
        .and_then(|endpoint| decisions_endpoint(endpoint).ok())
        .is_some_and(|endpoint| decisions_endpoint_is_loopback(&endpoint))
}
