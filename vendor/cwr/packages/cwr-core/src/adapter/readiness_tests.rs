use super::*;

#[test]
fn retained_non_system_codex_recovers_in_its_original_home() {
    let temporary = tempfile::tempdir().unwrap();
    let original =
        managed_provider_home_at(&temporary.path().join("original ' workspace"), "codex").unwrap();
    let shared = temporary.path().join("shared");
    std::fs::create_dir(&shared).unwrap();
    std::fs::write(shared.join("signed-in"), "shared").unwrap();
    std::fs::write(original.join("session-id"), "retained-session").unwrap();
    let executable = temporary.path().join("Codex's $(touch unexpected)");
    std::fs::write(
        &executable,
        r#"#!/bin/sh
set -eu
if [ "$*" = 'login status' ]; then test -f "$CODEX_HOME/signed-in"; exit; fi
[ "$*" = login ] || exit 2
: > "$CODEX_HOME/signed-in"
"#,
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let probe = |home: &Path, store| {
        let readiness = codex_readiness(
            Some(temporary.path().as_os_str().to_owned()),
            None,
            Some(executable.as_os_str().to_owned()),
            Some(OsString::from("/usr/bin:/bin")),
            Some(home),
            Duration::from_secs(3),
        );
        codex_store_login_recovery(readiness, store, Some(home), Some(executable.clone()))
    };
    assert!(probe(&shared, ProviderStore::Shared).is_ready());
    let before = probe(&original, ProviderStore::Managed);
    assert_eq!(before.code, "provider_login_required");
    assert!(
        before.repair.is_none(),
        "shared Settings cannot repair this home"
    );
    let command = before
        .message
        .split_once("Run ")
        .unwrap()
        .1
        .strip_suffix(" in Terminal, then retry.")
        .unwrap();
    assert!(Command::new("/bin/sh")
        .args(["-c", command])
        .current_dir(temporary.path())
        .status()
        .unwrap()
        .success());
    assert!(probe(&original, ProviderStore::Managed).is_ready());
    assert_eq!(
        std::fs::read_to_string(original.join("session-id")).unwrap(),
        "retained-session"
    );
    assert_eq!(
        std::fs::read_to_string(shared.join("signed-in")).unwrap(),
        "shared"
    );
    std::fs::remove_file(shared.join("signed-in")).unwrap();
    std::fs::write(shared.join("session-id"), "retained-shared-session").unwrap();
    let shared_before = probe(&shared, ProviderStore::Shared);
    assert_eq!(shared_before.code, "provider_login_required");
    assert!(
        shared_before.repair.is_none(),
        "system Settings cannot repair a retained shared home"
    );
    let command = shared_before
        .message
        .split_once("Run ")
        .unwrap()
        .1
        .strip_suffix(" in Terminal, then retry.")
        .unwrap();
    assert!(Command::new("/bin/sh")
        .args(["-c", command])
        .current_dir(temporary.path())
        .status()
        .unwrap()
        .success());
    assert!(probe(&shared, ProviderStore::Shared).is_ready());
    assert_eq!(
        std::fs::read_to_string(shared.join("session-id")).unwrap(),
        "retained-shared-session"
    );
    assert!(!temporary.path().join("unexpected").exists());
    let mut invocation = AdapterInvocation::new("inv", "respond_to--message", json!({}), json!({}));
    invocation.continuation =
        Some(json!({"thread_id":"retained-session","provider_store":"managed"}));
    assert_eq!(
        codex_provider_store(&invocation).unwrap(),
        ProviderStore::Managed
    );
}

fn fake_codex(exit: i32) -> (tempfile::TempDir, OsString, OsString) {
    let temporary = tempfile::tempdir().unwrap();
    let executable = temporary.path().join("codex");
    std::fs::write(
        &executable,
        format!("#!/bin/sh\n[ \"$1 $2\" = \"login status\" ] || exit 2\nexit {exit}\n"),
    )
    .unwrap();
    let mut permissions = executable.metadata().unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(executable, permissions).unwrap();
    let path = temporary.path().as_os_str().to_owned();
    (temporary, OsString::from("/tmp"), path)
}

fn slow_codex() -> (tempfile::TempDir, OsString, OsString) {
    let temporary = tempfile::tempdir().unwrap();
    let executable = temporary.path().join("codex");
    std::fs::write(
        &executable,
        "#!/bin/sh\n/bin/sleep 10 &\necho $! > \"$HOME/probe-child\"\nwait\n",
    )
    .unwrap();
    let mut permissions = executable.metadata().unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(executable, permissions).unwrap();
    let path = temporary.path().as_os_str().to_owned();
    let home = temporary.path().as_os_str().to_owned();
    (temporary, home, path)
}

fn process_exists(pid: i32) -> bool {
    // SAFETY: signal zero performs existence/permission checking without
    // delivering a signal, and the PID came from the spawned test child.
    unsafe { libc::kill(pid, 0) == 0 }
}

fn catalog_server() -> (tempfile::TempDir, Child) {
    let temporary = tempfile::tempdir().unwrap();
    let executable = temporary.path().join("codex");
    std::fs::write(
        &executable,
        r#"#!/bin/sh
read initialize
printf '%s\n' '{"id":"cwr-model-catalog-initialize","result":{}}'
read initialized
read first
printf '%s\n' '{"id":"cwr-model-catalog-1","result":{"data":[{"model":"gpt-6-sol","supportedReasoningEfforts":[{"reasoningEffort":"low"},{"reasoningEffort":"max"}]}],"nextCursor":"next"}}'
read second
printf '%s\n' '{"id":"cwr-model-catalog-2","result":{"data":[{"model":"gpt-6-luna","supportedReasoningEfforts":[{"reasoningEffort":"medium"}]}],"nextCursor":null}}'
"#,
    )
    .unwrap();
    let mut permissions = executable.metadata().unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&executable, permissions).unwrap();
    let child = Command::new(executable)
        .args(["app-server", "--stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    (temporary, child)
}

#[test]
fn codex_readiness_reports_installation_and_login_setup() {
    let temporary = tempfile::tempdir().unwrap();
    let missing = codex_readiness(
        Some(OsString::from("/tmp")),
        None,
        None,
        Some(OsString::from("/definitely/missing")),
        Some(temporary.path()),
        Duration::from_secs(1),
    );
    assert_eq!(missing.status, AdapterReadinessStatus::SetupRequired);
    assert_eq!(missing.code, "provider_executable_missing");

    let (_temporary, home, path) = fake_codex(1);
    let signed_out = codex_readiness(
        Some(home),
        None,
        None,
        Some(path),
        Some(temporary.path()),
        Duration::from_secs(3),
    );
    assert_eq!(signed_out.status, AdapterReadinessStatus::SetupRequired);
    assert_eq!(signed_out.code, "provider_login_required");
    assert_eq!(
        signed_out.repair,
        Some(AdapterRepairOffer {
            kind: AdapterRepairKind::ProviderLogin,
            label: "Sign in".into(),
        })
    );
    assert_eq!(
        serde_json::to_value(&signed_out).unwrap(),
        json!({
            "status": "setup_required",
            "code": "provider_login_required",
            "message": "Codex is installed but not signed in to Workloop. Open AI provider settings to sign in, then retry.",
            "repair": {"kind": "provider_login", "label": "Sign in"},
        })
    );
}

#[test]
fn legacy_codex_login_does_not_offer_managed_home_repair() {
    let (temporary, home, path) = fake_codex(1);
    let signed_out = codex_readiness(
        Some(home),
        None,
        None,
        Some(path),
        None,
        Duration::from_secs(3),
    );
    assert_eq!(signed_out.code, "provider_login_required");
    assert!(signed_out.repair.is_none());
    assert!(signed_out.message.contains("codex login in Terminal"));
    assert!(temporary.path().exists());
}

#[test]
fn codex_readiness_accepts_a_signed_in_cli() {
    let (temporary, home, path) = fake_codex(0);
    let readiness = codex_readiness(
        Some(home),
        None,
        None,
        Some(path),
        Some(temporary.path()),
        Duration::from_secs(3),
    );
    assert!(readiness.is_ready());
    assert_eq!(readiness.code, "ready");
    assert!(readiness.repair.is_none());
    assert!(serde_json::to_value(&readiness)
        .unwrap()
        .get("repair")
        .is_none());
}

#[test]
fn codex_login_probe_uses_the_managed_home() {
    let temporary = tempfile::tempdir().unwrap();
    let executable = temporary.path().join("codex");
    let user_home = temporary.path().join("user");
    let codex_home = temporary.path().join("managed-codex");
    std::fs::create_dir(&user_home).unwrap();
    std::fs::create_dir(&codex_home).unwrap();
    std::fs::write(
        &executable,
        "#!/bin/sh\nprintf '%s' \"$CODEX_HOME\" > \"$HOME/probed-codex-home\"\n",
    )
    .unwrap();
    let mut permissions = executable.metadata().unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&executable, permissions).unwrap();

    let readiness = codex_readiness(
        Some(user_home.clone().into()),
        None,
        Some(executable.into()),
        Some(temporary.path().into()),
        Some(&codex_home),
        Duration::from_secs(3),
    );
    assert!(readiness.is_ready());
    assert_eq!(
        std::fs::read_to_string(user_home.join("probed-codex-home")).unwrap(),
        codex_home.to_string_lossy()
    );
}

#[test]
fn codex_executable_override_does_not_fall_back_to_path() {
    let (temporary, _home, path) = fake_codex(0);
    assert!(codex_executable(None, Some(&path)).is_some());
    assert!(
        codex_executable(Some(temporary.path().join("missing").into()), Some(&path),).is_none()
    );
}

#[test]
fn claude_executable_override_does_not_fall_back_to_path() {
    let temporary = tempfile::tempdir().unwrap();
    let executable = temporary.path().join("claude");
    std::fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
    let mut permissions = executable.metadata().unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&executable, permissions).unwrap();
    assert!(claude_executable(None, Some(temporary.path().into())).is_some());
    assert!(claude_executable(
        Some(temporary.path().join("missing").into()),
        Some(temporary.path().into()),
    )
    .is_none());
}

#[test]
fn shell_runner_readiness_checks_the_configured_executable() {
    assert!(shell_runner_readiness(&json!({}), Path::new("/tmp")).is_ready());
    let temporary = tempfile::tempdir().unwrap();
    let executable = temporary.path().join("runner");
    std::fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
    let mut permissions = executable.metadata().unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(executable, permissions).unwrap();
    assert!(
        shell_runner_readiness(&json!({"runner": ["./runner", "-c"]}), temporary.path()).is_ready()
    );
    let missing = shell_runner_readiness(
        &json!({"runner": ["/definitely/missing/cwr-shell", "-c"]}),
        temporary.path(),
    );
    assert_eq!(missing.status, AdapterReadinessStatus::SetupRequired);
    assert_eq!(missing.code, "provider_executable_missing");
}

#[test]
fn codex_readiness_stops_a_probe_that_exceeds_its_deadline() {
    let (temporary, home, path) = slow_codex();
    let readiness = codex_readiness(
        Some(home),
        None,
        None,
        Some(path),
        Some(temporary.path()),
        Duration::from_secs(2),
    );
    assert_eq!(
        readiness.status,
        AdapterReadinessStatus::TemporarilyUnavailable
    );
    assert_eq!(readiness.code, "provider_readiness_failed");
    // Under a fully parallel test run, the timeout can expire before the
    // script starts its descendant. If it did start, verify group cleanup.
    if let Ok(pid) = std::fs::read_to_string(temporary.path().join("probe-child")) {
        let child_pid = pid.trim().parse::<i32>().unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while process_exists(child_pid) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(!process_exists(child_pid));
    }
}

#[test]
fn codex_model_catalog_reads_all_pages_and_reasoning_efforts() {
    let (_temporary, mut child) = catalog_server();
    let catalog = read_codex_model_catalog(&mut child).unwrap();
    assert!(catalog.models["gpt-6-sol"].contains("low"));
    assert!(catalog.models["gpt-6-sol"].contains("max"));
    assert!(catalog.models["gpt-6-luna"].contains("medium"));
    let _ = child.wait();
}

#[test]
fn codex_model_readiness_explains_missing_models_and_efforts() {
    let catalog = CodexModelCatalog {
        models: BTreeMap::from([("gpt-6-sol".into(), BTreeSet::from(["low".into()]))]),
    };
    let unavailable = ResolvedProcessAdapterRoute::new(
        CODEX_ADAPTER_REF,
        "/work",
        "/role",
        json!({"model":"gpt-6-luna", "reasoning_effort":"low"}),
        json!({}),
        json!({}),
    );
    assert_eq!(
        codex_model_readiness(&unavailable, &catalog).code,
        "provider_model_unavailable"
    );
    let unsupported = ResolvedProcessAdapterRoute::new(
        CODEX_ADAPTER_REF,
        "/work",
        "/role",
        json!({"model":"gpt-6-sol", "reasoning_effort":"max"}),
        json!({}),
        json!({}),
    );
    assert_eq!(
        codex_model_readiness(&unsupported, &catalog).code,
        "provider_reasoning_effort_unsupported"
    );
}

#[test]
fn unsupported_adapter_readiness_is_user_facing() {
    let route = ResolvedProcessAdapterRoute::new(
        "cwp://org.example/adapters/unimplemented",
        "/work",
        "/role",
        json!({}),
        json!({}),
        json!({}),
    );
    let readiness = route.readiness();
    assert_eq!(readiness.status, AdapterReadinessStatus::SetupRequired);
    assert_eq!(readiness.code, "adapter_implementation_missing");
    assert!(readiness.message.contains("version of Workloop"));
}

#[test]
fn decisions_readiness_requires_credentials_for_remote_endpoints() {
    let router = json!({"endpoint":"https://openrouter.ai/api/alpha/decisions"});
    let missing = decisions_readiness(&router, || None);
    assert_eq!(missing.status, AdapterReadinessStatus::SetupRequired);
    assert_eq!(missing.code, "decisions_credentials_missing");
    assert!(decisions_readiness(&router, || Some("host-key".into())).is_ready());
    assert!(!decisions_readiness(&router, || Some("  ".into())).is_ready());
    let custom = json!({"endpoint":"https://example.com/decisions"});
    assert!(!decisions_readiness(&custom, || panic!(
        "untrusted origin must not inspect host credentials"
    ))
    .is_ready());
    assert!(decisions_readiness(
        &json!({"endpoint":"https://example.com/decisions","api_key":"private-key"}),
        || panic!("explicit credential is sufficient")
    )
    .is_ready());
    // Readiness observes the configured slot, without retrieving the secret.
    assert!(decisions_readiness(
        &json!({"endpoint":"https://example.com/decisions","api_key":"managed-input:router"}),
        || panic!("managed credential is configured")
    )
    .is_ready());
    for endpoint in [
        "http://localhost:8080/decisions",
        "http://127.0.0.1:8080/decisions",
        "http://[::1]:8080/decisions",
    ] {
        assert!(
            decisions_readiness(&json!({"endpoint":endpoint}), || panic!(
                "offline fixtures need no credential"
            ))
            .is_ready()
        );
    }
}

#[test]
fn decisions_invalid_endpoints_are_not_ready_even_with_private_keys() {
    for endpoint in [
        "http://localhost:0/grade",
        "https://example.com:65536/grade",
        "https://openrouter.ai/grade?query=1",
        "https://user@example.com/grade",
    ] {
        let params = json!({"endpoint":endpoint,"api_key":"private-key"});
        let readiness = decisions_readiness(&params, || {
            panic!("invalid endpoint must not read host key")
        });
        assert_eq!(readiness.code, "decisions_endpoint_invalid", "{endpoint}");
        assert!(!readiness.is_ready());
        assert!(decisions_api_key(&params, true, || panic!(
            "invalid endpoint must not read host key"
        ))
        .is_none());
    }
}
