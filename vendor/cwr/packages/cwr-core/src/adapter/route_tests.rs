#[test]
fn claude_process_user_preserves_inherited_user() {
    let user = claude_process_user(Some(OsString::from("inherited-user")), || {
        panic!("OS account lookup should not run when USER is inherited")
    });
    assert_eq!(user, Some(OsString::from("inherited-user")));
}

#[test]
fn retained_provider_sessions_keep_their_original_store() {
    let mut invocation = AdapterInvocation::new("invocation", "respond_to--message", json!({}), json!({}));
    assert_eq!(provider_store(&invocation).unwrap(), ProviderStore::Managed);

    invocation.thread = json!({"working_conversation":{"state":{"thread_id":"old"}}});
    assert_eq!(provider_store(&invocation).unwrap(), ProviderStore::Legacy);

    invocation.thread["working_conversation"]["state"]["provider_store"] = json!("managed");
    assert_eq!(provider_store(&invocation).unwrap(), ProviderStore::Managed);

    invocation.thread = json!({"requesting_context":{"working_conversation":{"state":{"provider_store":"managed"}}}});
    assert_eq!(provider_store(&invocation).unwrap(), ProviderStore::Managed);

    invocation.continuation = Some(json!({"thread_id":"old-checkpoint"}));
    assert!(provider_store(&invocation).is_err());
    invocation.continuation = Some(json!({"thread_id":"managed-checkpoint","provider_store":"managed"}));
    assert_eq!(provider_store(&invocation).unwrap(), ProviderStore::Managed);

    invocation.thread = json!({});
    invocation.continuation = Some(json!({"thread_id":"old-checkpoint"}));
    assert_eq!(provider_store(&invocation).unwrap(), ProviderStore::Legacy);
    invocation.continuation = Some(json!({"provider_store":"other"}));
    assert!(provider_store(&invocation).is_err());
}

#[test]
fn managed_provider_homes_are_private_and_reject_symlinks() {
    let temporary = tempfile::tempdir().unwrap();
    let home = managed_provider_home_at(temporary.path(), "codex").unwrap();
    assert_eq!(home, temporary.path().join("provider-homes/codex"));
    assert_eq!(home.metadata().unwrap().permissions().mode() & 0o777, 0o700);
    assert_eq!(home.parent().unwrap().metadata().unwrap().permissions().mode() & 0o777, 0o700);

    std::os::unix::fs::symlink(temporary.path(), temporary.path().join("provider-homes/claude")).unwrap();
    assert!(managed_provider_home_at(temporary.path(), "claude").is_err());
}

#[test]
fn claude_process_user_uses_os_account_when_user_is_absent() {
    let user = claude_process_user(None, || Some(OsString::from("local-os-user")));
    assert_eq!(user, Some(OsString::from("local-os-user")));
}

#[test]
fn claude_process_path_preserves_the_hermetic_fallback() {
    assert_eq!(claude_process_path(None), OsString::from("/usr/bin:/bin"));
    assert_eq!(
        claude_process_path(Some(OsString::from("/bundled/tools:/usr/bin"))),
        OsString::from("/bundled/tools:/usr/bin")
    );
}

#[test]
fn local_os_username_resolves_the_effective_account() {
    assert!(local_os_username().is_some_and(|user| !user.is_empty()));
}

#[test]
fn adapter_executables_require_execute_permission() {
    let temporary = tempfile::tempdir().unwrap();
    let candidate = temporary.path().join("adapter");
    std::fs::write(&candidate, "adapter").unwrap();
    let mut permissions = candidate.metadata().unwrap().permissions();
    permissions.set_mode(0o644);
    std::fs::set_permissions(&candidate, permissions).unwrap();
    assert!(!is_executable_file(&candidate));

    let mut permissions = candidate.metadata().unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&candidate, permissions).unwrap();
    assert!(is_executable_file(&candidate));
}

#[test]
fn resolved_shell_adapters_receive_a_graceful_shutdown_window() {
    let process = resolved_shell_adapter(PathBuf::from("/bin/sh"), Path::new("/work"));
    assert_eq!(process.shutdown_grace_period(), Duration::from_secs(2));
}

#[test]
fn shell_smolvm_opt_in_refuses_host_resource_bindings() {
    let route = ResolvedProcessAdapterRoute::new(
        SH_ADAPTER_REF,
        "/work",
        "/role",
        json!({}),
        json!({"runner":["/bin/bash","-c"]}),
        json!({"execution_environment":"smolvm"}),
    );
    assert!(validate_smolvm_shell_route(&route).is_ok());
    let mut selected = route.clone();
    selected.actor_integration_params["execution_environment"] = json!("host");
    assert!(validate_smolvm_shell_route(&selected).is_err());
    assert_ne!(route.provider_fingerprint(), selected.provider_fingerprint());

    for (target, value) in [
        ("host_gateway", json!(true)),
        ("host_env_actions", json!({"receive--message":"WORKLOOP_SMS_BRIDGE_SOCKET"})),
        ("activated_volumes", json!({"data":{"path":"/private/data","access":"read"}})),
    ] {
        let mut denied = route.clone();
        denied.actor_integration_params[target] = value;
        assert!(validate_smolvm_shell_route(&denied).is_err());
    }
    let mut denied = route;
    denied.integration_params["env"] = json!({"TOKEN":"private"});
    assert!(validate_smolvm_shell_route(&denied).is_err());
}

#[test]
fn shell_host_environment_is_selected_per_sms_action() {
    let params = json!({"host_env_actions": {
        "receive--message": "WORKLOOP_SMS_BRIDGE_SOCKET"
    }});
    assert_eq!(
        shell_host_environment_for_action(&params, "receive--message", |_| {
            Some(OsString::from("/private/workloop-sms.sock"))
        }).unwrap(),
        Some(OsString::from("/private/workloop-sms.sock")),
    );
    assert_eq!(
        shell_host_environment_for_action(&params, "run--command", |_| {
            panic!("unconfigured action must not read host environment")
        }).unwrap(),
        None,
    );
    for socket in [None, Some(""), Some("relative.sock")] {
        assert!(shell_host_environment_for_action(&params, "receive--message", |_| {
            socket.map(OsString::from)
        }).is_err());
    }
    assert!(shell_host_environment_for_action(
        &json!({"host_env_actions": {"run--command": "WORKLOOP_SMS_BRIDGE_SOCKET"}}),
        "run--command",
        |_| Some(OsString::from("/private/workloop-sms.sock")),
    ).is_err());
}

#[test]
fn live_action_grants_do_not_reset_provider_state() {
    let route = ResolvedProcessAdapterRoute::new(
        "codex",
        "/work",
        "/role",
        json!({}),
        json!({}),
        json!({"sandbox":"read-only","cwr_action_actor_refs":["alice"]}),
    );
    let mut revoked = route.clone();
    revoked.actor_integration_params["cwr_action_actor_refs"] = json!([]);
    revoked.authority = vec!["some-current-grant".into()];
    assert_ne!(route.fingerprint(), revoked.fingerprint());
    assert_eq!(route.provider_fingerprint(), revoked.provider_fingerprint());
    revoked.adapter_params["cwr_native_tools"] = json!([{"name":"cwp.alice--draft--brief"}]);
    assert_eq!(route.provider_fingerprint(), revoked.provider_fingerprint());
    revoked.actor_integration_params["sandbox"] = json!("workspace-write");
    assert_ne!(route.provider_fingerprint(), revoked.provider_fingerprint());
}

#[test]
fn codex_history_delivery_preference_does_not_change_provider_fingerprint() {
    let route = ResolvedProcessAdapterRoute::new(
        CODEX_ADAPTER_REF,
        "/work",
        "/role",
        json!({"role_contexts":[{"params":{"entrypoint":"./PROMPT.md","selective_history":true}}],"selective_history":true}),
        json!({}),
        json!({}),
    );
    let mut disabled = route.clone();
    disabled.adapter_params["selective_history"] = json!(false);
    disabled.adapter_params["role_contexts"][0]["params"]["selective_history"] = json!(false);
    assert_ne!(route.fingerprint(), disabled.fingerprint());
    assert_eq!(route.provider_fingerprint(), disabled.provider_fingerprint());
    assert_ne!(route.legacy_selective_provider_fingerprint(), route.provider_fingerprint());
}

#[test]
fn admitted_route_retains_activated_sources_for_retry() {
    let mut route = ResolvedProcessAdapterRoute::new(
        "codex",
        "/work",
        "/role",
        json!({"activated_sources":{ "/role/PROMPT.md":"A"},"activated_entrypoints":["/role/PROMPT.md"]}),
        json!({}),
        json!({}),
    );
    let admitted = route.project_invocation(
        "invocation",
        "respond_to--message",
        json!({}),
        json!({}),
        json!({}),
    );
    route.adapter_params["activated_sources"]["/role/PROMPT.md"] = json!("B");
    assert_eq!(
        admitted.adapter_params["activated_sources"]["/role/PROMPT.md"],
        "A"
    );
    assert_ne!(admitted.route_fingerprint, Some(route.fingerprint()));
    assert_ne!(
        admitted.provider_fingerprint,
        Some(route.provider_fingerprint())
    );
}

#[test]
fn decisions_host_key_is_only_available_to_the_openrouter_https_origin() {
    for endpoint in [
        "https://openrouter.ai/api/alpha/decisions",
        "https://openrouter.ai:443/api/alpha/decisions",
    ] {
        assert_eq!(
            decisions_api_key(&json!({"endpoint":endpoint}), true, || Some(
                "host-key".into()
            )),
            Some("host-key".into())
        );
    }
    for endpoint in [
        "https://example.com/decisions",
        "https://openrouter.ai.evil.example/decisions",
        "https://openrouter.ai:444/decisions",
        "http://openrouter.ai/decisions",
        "http://localhost:8080/decisions",
        "http://127.0.0.1:8080/decisions",
        "https://openrouter.ai@evil.example/decisions",
        "https://user@openrouter.ai/decisions",
        "invalid-url",
    ] {
        assert_eq!(
            decisions_api_key(&json!({"endpoint":endpoint}), true, || panic!(
                "untrusted endpoint must not read ambient key"
            )),
            None,
            "{endpoint}"
        );
    }
    let trusted = json!({"endpoint":"https://openrouter.ai/api/alpha/decisions"});
    assert_eq!(
        decisions_api_key(&trusted, false, || panic!(
            "managed delivery disables host fallback"
        )),
        None
    );
    assert_eq!(decisions_api_key(&trusted, true, || None), None);
    assert_eq!(
        decisions_api_key(&trusted, true, || Some("  ".into())),
        None
    );
}

#[test]
fn decisions_explicit_private_key_authorizes_the_selected_endpoint() {
    for endpoint in [
        "https://example.com/decisions",
        "http://127.0.0.1:8080/decisions",
    ] {
        assert_eq!(
            decisions_api_key(
                &json!({"endpoint":endpoint,"api_key":"explicit-key"}),
                false,
                || panic!("explicit keys never read ambient credentials")
            ),
            Some("explicit-key".into())
        );
    }
}

#[test]
fn codex_new_work_uses_system_home_but_retained_homes_are_preserved() {
    let mut invocation = AdapterInvocation::new("inv", "respond_to--message", json!({}), json!({}));
    assert_eq!(codex_provider_store(&invocation).unwrap(), ProviderStore::System);
    invocation.continuation = Some(json!({"provider_store":"system"}));
    assert_eq!(codex_provider_store(&invocation).unwrap(), ProviderStore::System);
    invocation.continuation = Some(json!({"provider_store":"managed"}));
    assert_eq!(codex_provider_store(&invocation).unwrap(), ProviderStore::Managed);
    invocation.continuation = Some(json!({"provider_store":"shared"}));
    assert_eq!(codex_provider_store(&invocation).unwrap(), ProviderStore::Shared);
    invocation.continuation = Some(json!({"thread_id":"old"}));
    assert_eq!(codex_provider_store(&invocation).unwrap(), ProviderStore::Legacy);
}
