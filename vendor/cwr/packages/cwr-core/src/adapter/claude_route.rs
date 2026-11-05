fn resolve_claude_adapter(
    route: &ResolvedProcessAdapterRoute,
    provider_store: ProviderStore,
) -> CwrResult<LocalAdapterResolution> {
    let home = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| protocol_error("Claude adapter requires HOME"))?;
    let path = std::env::var_os("PATH").filter(|value| !value.is_empty());
    let executable = match std::env::var_os("CWR_CLAUDE_EXECUTABLE") {
        Some(path) => PathBuf::from(path),
        None => find_path_executable("claude")?,
    }
    .canonicalize()?;
    let path = claude_process_path(path);
    let api_credentials = ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"]
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()));
    let use_cli_login = !api_credentials;
    let config_directory = match provider_store {
        ProviderStore::Shared => return protocol_failure("Claude does not support the shared provider store"),
        ProviderStore::System => return protocol_failure("Claude does not support the system provider store"),
        ProviderStore::Managed => Some(managed_provider_home("claude")?),
        ProviderStore::Legacy => std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(|| api_credentials.then(|| PathBuf::from(&home).join(".local/share/cwr/claude"))),
    };
    let mut process = ResolvedProcessAdapter::new(sibling_adapter_executable(
        "CWR_CLAUDE_ADAPTER_EXECUTABLE",
        "cwr-adapter-claude",
    )?)
    .with_current_directory(&route.working_path)
    .with_environment("CWR_COWORKER_DIRECTORY", &route.coworker_directory)
    .with_environment(
        "CWR_CLAUDE_DANGER_FULL_ACCESS_WORKSPACE",
        std::fs::canonicalize(&route.working_path)?,
    )
    .with_environment("HOME", home)
    .with_environment("PATH", path)
    .with_shutdown_grace_period(Duration::from_secs(2));
    // Leave the variable unset for a normal CLI login: setting even the default
    // directory can change where Claude looks for its existing account config.
    if let Some(directory) = config_directory {
        process = process.with_environment("CLAUDE_CONFIG_DIR", directory);
    }
    if provider_store == ProviderStore::Managed {
        process = process.with_environment("CWR_PROVIDER_STORE", "managed");
    }
    if let Some(user) = claude_process_user(std::env::var_os("USER"), local_os_username) {
        process = process.with_environment("USER", user);
    }
    // Secrets stay in the child environment, never in authored resource params.
    for name in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "ANTHROPIC_BASE_URL",
    ] {
        if let Some(value) = std::env::var_os(name) {
            process = process.with_environment(name, value);
        }
    }
    Ok(LocalAdapterResolution {
        process,
        integration_params: Some(
            json!({"claude_executable":executable,"use_cli_login":use_cli_login}),
        ),
    })
}

fn claude_process_path(inherited: Option<OsString>) -> OsString {
    inherited
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| OsString::from("/usr/bin:/bin"))
}

fn claude_process_user(
    inherited: Option<OsString>,
    local_os_username: impl FnOnce() -> Option<OsString>,
) -> Option<OsString> {
    inherited.or_else(local_os_username)
}

fn local_os_username() -> Option<OsString> {
    use std::{ffi::CStr, os::unix::ffi::OsStringExt as _};

    // SAFETY: geteuid reads process credentials and has no preconditions.
    let effective_user_id = unsafe { libc::geteuid() };
    // POSIX permits an indeterminate recommendation; 16 KiB is a conventional
    // starting point and ERANGE below grows it when the account record is larger.
    // SAFETY: sysconf reads process configuration and has no pointer preconditions.
    let recommended = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
    let mut buffer_length = usize::try_from(recommended).unwrap_or(16 * 1024).max(1024);

    loop {
        // SAFETY: passwd is a C POD whose pointers are populated by getpwuid_r.
        let mut account = unsafe { std::mem::zeroed::<libc::passwd>() };
        let mut result = std::ptr::null_mut();
        let mut buffer = vec![0_u8; buffer_length];
        // SAFETY: account, result, and buffer are valid writable storage for the
        // duration of the call. A successful pw_name points into buffer.
        let status = unsafe {
            libc::getpwuid_r(
                effective_user_id,
                &mut account,
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if status == libc::ERANGE && buffer_length < 1024 * 1024 {
            buffer_length *= 2;
            continue;
        }
        if status != 0 || result.is_null() || account.pw_name.is_null() {
            return None;
        }
        // SAFETY: getpwuid_r returned success and pw_name is a NUL-terminated
        // string backed by the still-live buffer.
        let username = unsafe { CStr::from_ptr(account.pw_name) }.to_bytes();
        return (!username.is_empty()).then(|| OsString::from_vec(username.to_vec()));
    }
}
