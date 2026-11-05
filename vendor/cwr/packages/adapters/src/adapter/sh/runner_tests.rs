#[test]
fn smolvm_opt_in_requires_the_vm_proxy_before_shell_runs() {
    let schema: Value = serde_json::from_str(include_str!(
        "../../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/sh.actor-integration-params.schema.json"
    )).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    let params = json!({"execution_environment": "smolvm", "command": "printf ready"});
    assert!(validator.is_valid(&params));
    // The host Shell binary must not execute an opted-in action directly.
    // The VM proxy strips this routing key only after it creates the guest.
    assert!(serde_json::from_value::<ShActorIntegrationParams>(params.clone()).is_err());

    let mut invocation = AdapterInvocation::new(
        "~@/invocations/smolvm-shell-params",
        "run--task",
        json!({}),
        json!({"type": "object"}),
    );
    invocation.integration_params = fixture_runner();
    invocation.actor_integration_params = json!({"command": "printf ready"});
    let event = run_invocation(invocation);
    assert!(matches!(event, AdapterEvent::Result {
        status: 200, result: Some(result), ..
    } if result["stdout"] == "ready"));

    for invalid in [
        json!({"execution_environment": "host"}),
        json!({"execution_environment": "smolvm", "unexpected": true}),
    ] {
        assert!(!validator.is_valid(&invalid));
        assert!(serde_json::from_value::<ShActorIntegrationParams>(invalid).is_err());
    }
}

#[test]
fn integration_runner_keeps_command_text_in_one_argument_for_every_action_path() {
    let command =
        "printf '%s' '$HOME $(false) `false` \"quoted\"';\nprintf '\\n%s' \"$RUNNER_MARKER\"";
    let expected = "$HOME $(false) `false` \"quoted\"\nready";
    let source_ref = "~@/conversations/one/messages/one";
    for (action, actor_params, input, message) in [
        (
            SH_RUN_COMMAND_ACTION,
            json!({}),
            json!({"command": command}),
            String::new(),
        ),
        (
            "run--task",
            json!({"command": command}),
            json!({}),
            String::new(),
        ),
        (
            "run--task",
            json!({"commands": {"run--task": command}}),
            json!({}),
            String::new(),
        ),
        (
            RESPOND_TO_MESSAGE_ACTION,
            json!({}),
            json!({"source_message_ref": source_ref}),
            format!("@sh {command}"),
        ),
        (
            RESPOND_TO_MESSAGE_ACTION,
            json!({"message_commands": {"start": command}}),
            json!({"source_message_ref": source_ref}),
            "@helper start".into(),
        ),
        (
            RESPOND_TO_MESSAGE_ACTION,
            json!({"command": command, "message_context_json_stdin": true}),
            json!({"source_message_ref": source_ref}),
            "@helper".into(),
        ),
    ] {
        let mut invocation = AdapterInvocation::new(
            "~@/invocations/runner",
            action,
            input,
            json!({"type": "object"}),
        );
        invocation.integration_params = fixture_runner();
        invocation.actor_integration_params = actor_params;
        invocation.thread = json!({"messages": [{"message_ref": source_ref, "text": message}]});
        let event = run_invocation(invocation);
        let AdapterEvent::Result {
            status: 200,
            result: Some(result),
            ..
        } = event
        else {
            panic!("unexpected event: {event:?}");
        };
        if action == RESPOND_TO_MESSAGE_ACTION {
            assert!(
                result["text"]
                    .as_str()
                    .unwrap()
                    .contains(&format!("stdout:\n```{expected}\n```")),
                "{result}"
            );
        } else {
            assert_eq!(result["command"], command);
            assert_eq!(result["stdout"], expected);
            assert_eq!(result["exit_code"], 0);
        }
    }
}

#[test]
fn integration_runner_preserves_action_stdin_and_failure_diagnostics() {
    let mut invocation = AdapterInvocation::new(
        "~@/invocations/runner",
        "run--task",
        json!({"text": "quotes ' \" $HOME\nnext"}),
        json!({"type": "object"}),
    );
    invocation.integration_params = fixture_runner();
    invocation.actor_integration_params = json!({
        "command": "read -r input; printf '%s' \"$input\"; printf diagnostic >&2; exit 7",
        "fail_on_nonzero": true,
    });
    assert!(matches!(run_invocation(invocation), AdapterEvent::Result {
        status: 500, error: Some(error), ..
    } if error.details.as_ref().is_some_and(|details|
        details["stdout"] == "{\"text\":\"quotes ' \\\" $HOME\\nnext\"}"
            && details["stderr"] == "diagnostic"
            && details["exit_code"] == 7)));
}

#[test]
fn shell_runner_defaults_to_noninteractive_nonlogin_bash() {
    let runner = ShellCommandRunner::from_params(json!({})).unwrap();
    let output = runner.command("test -n \"$BASH_VERSION\" || exit 1; case $- in *i*) exit 2;; esac; shopt -q login_shell && exit 3; printf default").output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"default");
}

#[test]
fn shell_runner_validates_executable_and_argument_types() {
    let schema: Value = serde_json::from_str(include_str!(
        "../../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/sh.integration-params.schema.json"
    ))
    .unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    for params in [
        json!({"runner": []}),
        json!({"runner": [""]}),
        json!({"runner": "/bin/bash"}),
        json!({"runner": [42]}),
        json!({"unknown": true}),
    ] {
        assert!(!validator.is_valid(&params), "{params}");
        assert!(ShellCommandRunner::from_params(params).is_err());
    }
    let params = json!({"runner": ["/bin/bash", "-c", ""]});
    assert!(validator.is_valid(&params));
    assert!(ShellCommandRunner::from_params(params).is_ok());
}

fn fixture_runner() -> Value {
    json!({"runner": ["/bin/bash", "-c", "test \"$#\" = 1 || exit 91; export RUNNER_MARKER=ready; exec /bin/bash -c \"$1\"", "fixture"]})
}
#[test]
fn integration_environment_is_passed_privately_not_in_command_arguments() {
    let runner = ShellCommandRunner::from_params(json!({"env": {"SERVICE_API_KEY": "disposable-value"}})).unwrap();
    let command = runner.command("test \"$SERVICE_API_KEY\" = disposable-value");
    assert!(!format!("{:?}", command.get_args().collect::<Vec<_>>()).contains("SERVICE_API_KEY="));
    assert!(runner.command("test \"$SERVICE_API_KEY\" = disposable-value").status().unwrap().success());
    let schema: Value = serde_json::from_str(include_str!(
        "../../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/sh.integration-params.schema.json"
    ))
    .unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    for name in [
        "PATH",
        "HOME",
        "BASH_ENV",
        "CWR_INVOCATION_REF",
        "WORKLOOP_SMS_BRIDGE_SOCKET",
        "LD_PRELOAD",
        "DYLD_INSERT_LIBRARIES",
        "BAD=NAME",
    ] {
        let params = json!({"env": {name: "never-in-error"}});
        assert!(!validator.is_valid(&params), "{name}");
        assert!(ShellCommandRunner::from_params(params).is_err());
    }
}

#[test]
fn configured_shell_gateway_receives_activated_volume_paths_privately() {
    let mut invocation = AdapterInvocation::new(
        "~@/invocations/volume",
        "run--gateway",
        json!({}),
        json!({"type": "object"}),
    );
    invocation.integration_params = fixture_runner();
    invocation.actor_integration_params = json!({
        "commands": {"run--gateway": "printf '%s' \"$CWR_LOCAL_VOLUMES\""},
        "activated_volumes": {
            "memory": {"path": "/workspace/memory", "access": "read_write"}
        }
    });
    let event = run_invocation(invocation);
    let AdapterEvent::Result {
        status: 200,
        result: Some(result),
        ..
    } = event
    else {
        panic!("unexpected event: {event:?}");
    };
    assert_eq!(
        serde_json::from_str::<Value>(result["stdout"].as_str().unwrap()).unwrap(),
        json!({"memory": {"path": "/workspace/memory", "access": "read_write"}})
    );
}

#[test]
fn resolved_gateway_volume_mapping_reaches_shell_execution() {
    use cwr::v2026_09::graph::{
        resolve_graph_with_options, EmptyResourceResolver, GraphLoadOptions,
        ImportedVolumeBinding,
    };
    use std::path::Path;

    fn write(root: &Path, relative: &str, contents: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "CWR.toml", "type = 'cwp://org.coworkerprotocol/2026-09/schemas'\n[[imports]]\npath = './runtime/CWR.toml'\nas = 'runtime'\n");
    write(root, "runtime/CWR.toml", "type = 'cwp://org.coworkerprotocol/2026-09/schemas/runtime'\n[[volumes]]\nas = 'memory'\naccess = 'read_write'\n[[imports]]\npath = './gateway/GATEWAY.toml'\nas = 'gateway'\n");
    write(root, "runtime/gateway/GATEWAY.toml", "type = 'cwp://org.coworkerprotocol/2026-09/schemas/gateway'\nrole = { ref = '~@/handler' }\nintegration = { ref = '~@/provider' }\naction_name = 'handle--request'\n[[imports]]\npath = './ROLE.toml'\nas = 'handler'\n[[imports]]\npath = './integration.toml'\nas = 'provider'\n[params.commands]\nhandle--request = 'printf %s $CWR_LOCAL_VOLUMES'\n");
    write(root, "runtime/gateway/ROLE.toml", "type = 'cwp://org.coworkerprotocol/2026-09/schemas/role'\n[[adapters]]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/sh'\n[[actions]]\nverb = 'handle'\nobject = 'request'\ntitle = 'Handle request'\ndescription = 'Handle gateway request'\n[actions.input_schema]\npath = './schema.json'\n[actions.output_schema]\npath = './schema.json'\n");
    write(root, "runtime/gateway/integration.toml", "type = 'cwp://org.coworkerprotocol/2026-09/schemas/integration'\n[adapter]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/sh'\n");
    write(root, "runtime/gateway/schema.json", "{\"type\":\"object\"}");
    let volume_path = root.join("memory");
    std::fs::create_dir_all(&volume_path).unwrap();

    let mut options = GraphLoadOptions::default();
    options.imported_volumes.push(ImportedVolumeBinding {
        source: root.join("runtime/CWR.toml"),
        alias: "memory".into(),
        path: volume_path.clone(),
        access: "read_write".into(),
    });
    let graph = resolve_graph_with_options(
        root.join("CWR.toml"),
        &EmptyResourceResolver,
        &options,
    )
    .unwrap();
    let gateway = graph.gateways.first().unwrap();
    let invocation = gateway
        .route
        .project_adapter_invocation("~@/invocations/gateway", json!({}), json!({}))
        .unwrap();
    let event = run_invocation(invocation);
    let AdapterEvent::Result {
        status: 200,
        result: Some(result),
        ..
    } = event
    else {
        panic!("unexpected event: {event:?}");
    };
    let volumes: Value = serde_json::from_str(result["stdout"].as_str().unwrap()).unwrap();
    assert_eq!(
        volumes["memory"]["path"],
        volume_path.canonicalize().unwrap().to_string_lossy().as_ref()
    );
    assert_eq!(volumes["memory"]["access"], "read_write");
}

#[test]
fn host_socket_reaches_only_the_configured_sms_action() {
    let params: ShActorIntegrationParams = serde_json::from_value(json!({
        "host_env_actions": {"receive--message": "WORKLOOP_SMS_BRIDGE_SOCKET"}
    })).unwrap();
    let runner = ShellCommandRunner::from_params(json!({})).unwrap()
        .with_action_host_environment_from(&params, "receive--message", |_| {
            Some(std::ffi::OsString::from("/private/workloop-sms.sock"))
        }).unwrap();
    let output = runner.command("printf '%s' \"$WORKLOOP_SMS_BRIDGE_SOCKET\"")
        .output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"/private/workloop-sms.sock");

    let unrelated = ShellCommandRunner::from_params(json!({})).unwrap()
        .with_action_host_environment_from(&params, "run--command", |_| {
            panic!("unconfigured action must not read host environment")
        }).unwrap();
    assert!(unrelated.command("true").get_envs().any(|(name, value)| {
        name == WORKLOOP_SMS_BRIDGE_SOCKET_ENV && value.is_none()
    }));
}

#[test]
fn selected_sms_action_fails_without_an_approved_absolute_socket() {
    let params: ShActorIntegrationParams = serde_json::from_value(json!({
        "host_env_actions": {"deliver--message": "WORKLOOP_SMS_BRIDGE_SOCKET"}
    })).unwrap();
    for socket in [None, Some(""), Some("relative.sock")] {
        assert!(ShellCommandRunner::from_params(json!({})).unwrap()
            .with_action_host_environment_from(&params, "deliver--message", |_| {
                socket.map(std::ffi::OsString::from)
            }).is_err());
    }
}

#[test]
fn host_environment_config_accepts_only_sms_actions_and_the_bridge_name() {
    let schema: Value = serde_json::from_str(include_str!(
        "../../../../../packages/cwr-core/contracts/cwr/adapters/2026-09/sh.actor-integration-params.schema.json"
    )).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    for value in [
        json!({"host_env_actions": {"run--command": "WORKLOOP_SMS_BRIDGE_SOCKET"}}),
        json!({"host_env_actions": {"receive--message": "HOME"}}),
    ] {
        assert!(!validator.is_valid(&value));
        let params: ShActorIntegrationParams = serde_json::from_value(value).unwrap();
        assert!(ShellCommandRunner::from_params(json!({})).unwrap()
            .with_action_host_environment_from(&params, "receive--message", |_| {
                panic!("invalid mapping must not read host environment")
            }).is_err());
    }
    assert!(validator.is_valid(&json!({"host_env_actions": {
        "receive--message": "WORKLOOP_SMS_BRIDGE_SOCKET",
        "deliver--message": "WORKLOOP_SMS_BRIDGE_SOCKET",
        "handle--request": "WORKLOOP_SMS_BRIDGE_SOCKET"
    }})));
}
