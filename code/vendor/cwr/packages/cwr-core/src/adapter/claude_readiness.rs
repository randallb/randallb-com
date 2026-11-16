fn claude_auth_readiness(executable: &Path, claude_home: Option<&Path>, timeout: Duration) -> AdapterReadiness {
    // Match the Claude adapter's credential environment. Never print probe
    // output: authentication status can include account identifiers.
    let mut command = Command::new(executable);
    command.env_clear().env("DISABLE_UPDATES", "1").env("DISABLE_AUTOUPDATER", "1")
        .args(["auth", "status", "--text"])
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    if let Some(claude_home) = claude_home { command.env("CLAUDE_CONFIG_DIR", claude_home); }
    for name in ["HOME", "USER", "PATH", "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN", "ANTHROPIC_BASE_URL"] {
        if let Some(value) = std::env::var_os(name) { command.env(name, value); }
    }
    if let Some(home) = std::env::var_os("HOME") { command.current_dir(home); }
    command.process_group(0);
    let unavailable = || adapter_readiness(
        AdapterReadinessStatus::TemporarilyUnavailable,
        "provider_readiness_failed",
        "Workloop couldn’t check Claude sign-in. Update Claude if needed, then retry.",
    );
    let mut child = match command.spawn() { Ok(child) => child, Err(_) => return unavailable() };
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return adapter_readiness(
                AdapterReadinessStatus::Ready, "ready", "Claude is installed and signed in."),
            Ok(Some(status)) if status.code() == Some(1) => return adapter_readiness(
                AdapterReadinessStatus::SetupRequired, "provider_login_required",
                "Claude is installed but not signed in to Workloop. Open AI provider settings to sign in, then retry."),
            Ok(Some(_)) => return unavailable(),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            Ok(None) | Err(_) => { terminate_probe_group(&mut child); return unavailable(); }
        }
    }
}

#[cfg(test)]
mod claude_auth_tests {
    use super::*;

    #[test]
    fn auth_status_checks_sign_in_and_fails_closed_on_probe_errors() {
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("claude");
        for (exit, expected) in [(0, AdapterReadinessStatus::Ready), (1, AdapterReadinessStatus::SetupRequired), (2, AdapterReadinessStatus::TemporarilyUnavailable)] {
            std::fs::write(&executable, format!("#!/bin/sh\n[ \"$*\" = \"auth status --text\" ] || exit 2\nexit {exit}\n")).unwrap();
            let mut permissions = executable.metadata().unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&executable, permissions).unwrap();
            let readiness = claude_auth_readiness(&executable, Some(temp.path()), Duration::from_secs(3));
            assert_eq!(readiness.status, expected);
            assert!(readiness.repair.is_none());
        }
        std::fs::write(&executable, "#!/bin/sh\n/bin/sleep 10\n").unwrap();
        assert_eq!(claude_auth_readiness(&executable, Some(temp.path()), Duration::from_millis(30)).status, AdapterReadinessStatus::TemporarilyUnavailable);
    }

    #[test]
    fn auth_probe_uses_the_managed_config_directory() {
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("claude");
        let observed = temp.path().join("observed-config");
        let claude_home = temp.path().join("managed-claude");
        std::fs::write(&executable, format!("#!/bin/sh\nprintf '%s' \"$CLAUDE_CONFIG_DIR\" > '{}'\n", observed.display())).unwrap();
        let mut permissions = executable.metadata().unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();

        assert!(claude_auth_readiness(&executable, Some(&claude_home), Duration::from_secs(3)).is_ready());
        assert_eq!(std::fs::read_to_string(observed).unwrap(), claude_home.to_string_lossy());
    }
}
