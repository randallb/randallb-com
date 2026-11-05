/// Opt-in integration: actual CWR admission, gateway route and built-in Shell
/// process, using the packaged Deno with a fresh cache and fixture credentials.
#[test]
#[ignore = "build target/mcp-fixture-runtime with workloop_install_typescript.ts first"]
fn mcp_gateway_bundled_shell_send_request_round_trip() {
    let _lock = SH_ADAPTER_ENV_LOCK.lock().unwrap();
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let package = repo.join("target/mcp-fixture-runtime");
    let runtime_binary = package.join("loop-typescript-runtime");
    assert!(runtime_binary.exists(), "build the bundled runtime first");
    let temp = tempfile::tempdir().unwrap();
    let endpoint_file = temp.path().join("endpoint");
    let wait_marker = temp.path().join("waiting");
    let mut server = std::process::Command::new(&runtime_binary)
        .args([
            "run",
            "--no-config",
            "--no-npm",
            "--no-remote",
            "--allow-net=127.0.0.1",
            "--allow-write",
        ])
        .arg(repo.join("coworkers/mcp_gateway/testdata/server.ts"))
        .arg(&endpoint_file)
        .arg(&wait_marker)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .unwrap();
    struct ServerGuard<'a>(&'a mut std::process::Child);
    impl Drop for ServerGuard<'_> {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let _server_guard = ServerGuard(&mut server);
    let until = Instant::now() + Duration::from_secs(10);
    while !endpoint_file.exists() {
        assert!(Instant::now() < until);
        thread::sleep(Duration::from_millis(20));
    }
    let endpoint = fs::read_to_string(&endpoint_file).unwrap();
    let token = temp.path().join("fixture-token");
    fs::write(&token, "fixture-access-token").unwrap();
    fs::set_permissions(&token, fs::Permissions::from_mode(0o600)).unwrap();
    let config = temp.path().join("host-config.json");
    fs::write(
        &config,
        json!({"endpoint":endpoint,"approvedTools":["echo"],"tokenFile":token,"timeoutMs":3000})
            .to_string(),
    )
    .unwrap();
    let manifest = gateway_test_workspace(temp.path());
    let dir = manifest.parent().unwrap().join("coworkers/gateway");
    for file in [
        "GATEWAY.toml",
        "ROLE.toml",
        "integration.toml",
        "input.json",
        "output.json",
        "main.js",
    ] {
        fs::copy(package.join("mcp-gateway").join(file), dir.join(file)).unwrap();
    }
    let cache = temp.path().join("empty-deno-cache");
    fs::create_dir(&cache).unwrap();
    // Fixed argv and trusted configuration only; the caller cannot alter any
    // command field. Empty cache + --no-npm/--no-remote proves no downloads.
    let quote = |p: &Path| format!("'{}'", p.display().to_string().replace('\'', "'\\''"));
    let command = format!("exec {} run --no-config --no-npm --no-remote --cached-only --no-prompt --allow-read={} --allow-net=127.0.0.1 {} {}",
        quote(&runtime_binary), temp.path().display(), quote(&dir.join("main.js")), quote(&config));
    let gateway = dir.join("GATEWAY.toml");
    let content = fs::read_to_string(&gateway).unwrap();
    fs::write(&gateway, format!("{content}\n[params]\nstream_output = false\nfail_on_nonzero = true\ntimeout_ms = 5000\n[params.commands]\nhandle--request = {}\n", serde_json::to_string(&command).unwrap())).unwrap();
    let integration = dir.join("integration.toml");
    let content = fs::read_to_string(&integration).unwrap();
    fs::write(
        &integration,
        format!(
            "{content}\n[params.env]\nDENO_DIR = {}\n",
            serde_json::to_string(&cache.display().to_string()).unwrap()
        ),
    )
    .unwrap();
    let _env = GatewayTestEnvironment(std::env::var_os("CWR_SH_ADAPTER_EXECUTABLE"));
    std::env::set_var(
        "CWR_SH_ADAPTER_EXECUTABLE",
        repo.join("target/debug/cwr-adapter-sh"),
    );
    let person = load_default_local_acting_actor_ref(&manifest).unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        &manifest,
        temp.path().join("history.sqlite"),
        LocalHostIdentity::new(person, "cwp://com.example/mcp-fixture"),
    )
    .unwrap();
    let caller = gateway_status_parent(&runtime, None);
    let graph = runtime.graph().unwrap();
    let resource = graph.gateways[0].resource_ref.clone();
    assert!(!graph
        .actors
        .iter()
        .any(|a| a.actor_ref == graph.gateways[0].route.actor_ref));
    drop(graph);
    let invoke = |id: &str, request: Value| {
        let AdapterHostCallOutcome::Returned(response) = runtime.handle_host_call(
            AdapterCall { invocation_ref: caller.invocation_ref.clone(), call_id: id.into(), operation: "cwr.send_request".into(), input: json!({"type":GATEWAY_REQUEST_TYPE,"resource_ref":resource,"request":request}) },
            &caller, &Rc::new(RefCell::new(VecDeque::new())), &AdapterModelOutputMailbox::discard(), &mut |_| {},
        ).unwrap();
        response
    };
    let list = invoke("mcp-list", json!({"operation":"list_tools","cursor":null}));
    assert_eq!(list["status"], 200, "{list}");
    let output: Value = serde_json::from_str(list["result"]["stdout"].as_str().unwrap()).unwrap();
    assert_eq!(output["ok"], true, "{output}");
    assert_eq!(output["result"]["tools"].as_array().unwrap().len(), 1);
    assert_eq!(output["result"]["tools"][0]["name"], "echo");
    let call = invoke(
        "mcp-call",
        json!({"operation":"call_tool","name":"echo","arguments":{"text":"through actual CWR"}}),
    );
    assert_eq!(call["status"], 200, "{call}");
    let output: Value = serde_json::from_str(call["result"]["stdout"].as_str().unwrap()).unwrap();
    assert_eq!(
        output["result"]["structuredContent"]["echoed"], "through actual CWR",
        "{output}"
    );
    assert_eq!(output["result"]["content"][0]["text"], "through actual CWR");
    assert!(call["request_ref"].as_str().is_some());
    println!("CWR → configured gateway → cwr-adapter-sh → bundled loop-typescript-runtime → fixture MCP: {}", json!({"status":call["status"],"request_ref":call["request_ref"],"result":output}));
    let denied = invoke(
        "mcp-denied",
        json!({"operation":"call_tool","name":"denied","arguments":{}}),
    );
    assert!(denied["result"]["stdout"]
        .as_str()
        .unwrap()
        .contains("tool_not_approved"));
    let forged = invoke(
        "mcp-forged",
        json!({"operation":"list_tools","url":"https://attacker.invalid"}),
    );
    assert_ne!(forged["status"], 200, "{forged}");
    // A stopped admitted parent must terminate its real Shell gateway child.
    let stopped_runtime = runtime.clone();
    let parent = caller.invocation_ref.clone();
    let marker = wait_marker.clone();
    let stopper = thread::spawn(move || {
        let until = Instant::now() + Duration::from_secs(10);
        while !marker.exists() {
            assert!(Instant::now() < until);
            thread::sleep(Duration::from_millis(10));
        }
        stopped_runtime.stop_invocation(&parent, None).unwrap();
    });
    let stopped = invoke(
        "mcp-stop",
        json!({"operation":"call_tool","name":"echo","arguments":{"text":"pending","mode":"wait"}}),
    );
    stopper.join().unwrap();
    assert_eq!(stopped["status"], 499, "{stopped}");
    assert!(!list.to_string().contains("fixture-access-token"));
    assert!(!call.to_string().contains("fixture-access-token"));
}
