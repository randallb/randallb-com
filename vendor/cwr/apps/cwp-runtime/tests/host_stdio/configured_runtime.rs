use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Command, Stdio},
};

use cwr::v2026_09::host::{HostCommand, HostEvent, HOST_COMMAND_VERSION};

use super::support::runtime_home;

#[test]
fn only_runtime_name_selects_and_validates_the_workspace_loop() {
    let temporary = tempfile::tempdir().expect("temporary configured runtime");
    let workspace = write_workspace(temporary.path(), 1);
    let output = runtime_command(temporary.path(), &workspace)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "installed-0"
    );
}

#[test]
fn only_runtime_selection_captures_the_prepared_runtime_identity() {
    let temporary = tempfile::tempdir().expect("temporary configured runtime");
    let workspace = write_workspace(temporary.path(), 1);
    let workspace_bytes = fs::read(&workspace).unwrap();
    let source_path = temporary.path().join("package/CWR.toml");
    let source_bytes = fs::read(&source_path).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_cwp-runtime"))
        .arg(&workspace)
        .arg("--only-runtime-selection")
        .env("WORKLOOP_CONFIGURATION", r#"{}"#)
        .env("XDG_STATE_HOME", temporary.path().join("state-home"))
        .env("XDG_RUNTIME_DIR", runtime_home(temporary.path()))
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(fs::read(&workspace).unwrap(), workspace_bytes);
    assert_eq!(fs::read(&source_path).unwrap(), source_bytes);
    let selection: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(selection["name"], "installed-0");
    let composition = cwr::v2026_09::local::LocalWorkspaceComposition::load(&workspace).unwrap();
    assert_eq!(
        selection["runtime_ref"],
        composition.runtimes[0].runtime_ref
    );
    assert_eq!(
        Path::new(selection["working_directory"].as_str().unwrap()),
        temporary.path().join("customer").canonicalize().unwrap()
    );
    let runtime_directory = Path::new(selection["runtime_directory"].as_str().unwrap());
    assert_eq!(
        runtime_directory.parent().unwrap().file_name().unwrap(),
        "loops"
    );
    let identity = runtime_directory.file_name().unwrap().to_string_lossy();
    assert_eq!(identity.len(), 32);
    assert!(identity.bytes().all(|byte| byte.is_ascii_hexdigit()));
}

#[test]
fn only_runtime_name_rejects_a_workspace_with_multiple_loops() {
    let temporary = tempfile::tempdir().expect("temporary configured runtimes");
    let workspace = write_workspace(temporary.path(), 2);
    let output = runtime_command(temporary.path(), &workspace)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("exactly one configured runtime"));
}

#[test]
fn only_runtime_name_rejects_an_unavailable_acting_coworker() {
    let temporary = tempfile::tempdir().expect("temporary invalid loop");
    let workspace = write_workspace(temporary.path(), 1);
    fs::remove_file(temporary.path().join("package/COWORKER.toml")).unwrap();
    let output = runtime_command(temporary.path(), &workspace)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("acting_coworker"));
}

#[test]
fn configured_runtime_advertises_targets_but_accepts_only_its_acting_actor() {
    let temporary = tempfile::tempdir().expect("temporary configured runtime");
    let workspace = write_workspace(temporary.path(), 1);
    let mut child = Command::new(env!("CARGO_BIN_EXE_cwp-runtime"))
        .arg(&workspace)
        .args(["--runtime", "installed-0", "--host-stdio"])
        .env("WORKLOOP_CONFIGURATION", r#"{}"#)
        .env("XDG_STATE_HOME", temporary.path().join("state-home"))
        .env("XDG_RUNTIME_DIR", runtime_home(temporary.path()))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    serde_json::to_writer(
        &mut input,
        &HostCommand::InspectWorkspace {
            version: HOST_COMMAND_VERSION,
            request_id: "inspect-configured-loop".into(),
        },
    )
    .unwrap();
    input.write_all(b"\n").unwrap();
    input.flush().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    let event: HostEvent = serde_json::from_str(&line).unwrap();
    let HostEvent::WorkspaceInspected {
        workspace,
        default_actor_ref,
        ..
    } = event
    else {
        panic!("expected workspace inspection")
    };
    let acting_actor_ref = workspace
        .actors
        .iter()
        .find(|actor| actor.handle == "@helper")
        .unwrap()
        .actor_ref
        .clone();
    let target_actor_ref = workspace
        .actors
        .iter()
        .find(|actor| actor.handle == "@viewer")
        .unwrap()
        .actor_ref
        .clone();
    assert_eq!(
        Some(acting_actor_ref.as_str()),
        default_actor_ref.as_deref()
    );
    serde_json::to_writer(
        &mut input,
        &HostCommand::SelectActor {
            version: HOST_COMMAND_VERSION,
            request_id: "select-target".into(),
            actor_ref: target_actor_ref,
        },
    )
    .unwrap();
    input.write_all(b"\n").unwrap();
    input.flush().unwrap();
    line.clear();
    output.read_line(&mut line).unwrap();
    let event: HostEvent = serde_json::from_str(&line).unwrap();
    assert!(matches!(event, HostEvent::CommandFailed { .. }));
    drop(input);
    assert!(child.wait().unwrap().success());
}

fn runtime_command(root: &Path, workspace: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cwp-runtime"));
    command
        .arg(workspace)
        .arg("--only-runtime-name")
        .env("WORKLOOP_CONFIGURATION", r#"{}"#)
        .env("XDG_STATE_HOME", root.join("state-home"))
        .env("XDG_RUNTIME_DIR", runtime_home(root));
    command
}

pub(super) fn write_workspace(root: &Path, loop_count: usize) -> std::path::PathBuf {
    let package = root.join("package");
    let customer = root.join("customer");
    fs::create_dir(&package).unwrap();
    fs::create_dir(&customer).unwrap();
    fs::write(
        package.join("CWR.toml"),
        "type = 'cwp://org.coworkerprotocol/2026-09/schemas/loop'\n[[imports]]\npath = './COWORKER.toml'\nas = 'helper'\n[[imports]]\npath = './VIEWER.toml'\nas = 'viewer'\n",
    )
    .unwrap();
    fs::write(
        package.join("VIEWER.toml"),
        "type = 'cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname = 'Viewer'\nslug = 'viewer'\n",
    )
    .unwrap();
    fs::write(
        package.join("COWORKER.toml"),
        "type = 'cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname = 'Helper'\nslug = 'helper'\n",
    )
    .unwrap();
    let loops = (0..loop_count)
        .map(|index| {
            format!(
                "[[loops]]\nname = 'installed-{index}'\npath = './package/CWR.toml'\nsource_roots = ['./package']\nworking_directory = './customer'\nvolume = 'customer'\nacting_coworker = 'helper'\n"
            )
        })
        .collect::<String>();
    let workspace = root.join("WORKSPACE.toml");
    fs::write(
        &workspace,
        format!(
            "type = 'cwp://org.coworkerprotocol/2026-09/schemas/workspace'\n[[volumes]]\nname = 'customer'\npath = './customer'\n{loops}"
        ),
    )
    .unwrap();
    workspace
}

#[test]
fn runtime_selection_emits_typed_validation_errors_but_not_operational_errors() {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = write_workspace(temporary.path(), 2);
    for contents in [None, Some("invalid toml [")] {
        if let Some(contents) = contents {
            fs::write(&workspace, contents).unwrap();
        }
        let output = Command::new(env!("CARGO_BIN_EXE_cwp-runtime"))
            .arg(&workspace)
            .arg("--only-runtime-selection")
            .output()
            .unwrap();
        assert!(!output.status.success());
        let rejection: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(rejection["error"]["code"], "invalid_workspace");
    }
    // An unreadable file is an operational I/O error, not invalid TOML.
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&workspace, fs::Permissions::from_mode(0o000)).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_cwp-runtime"))
        .arg(&workspace)
        .arg("--only-runtime-selection")
        .output()
        .unwrap();
    assert!(!output.status.success());
    fs::set_permissions(&workspace, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(output.stdout.is_empty(), "{:?}", output);
}

#[test]
fn only_runtime_selection_distinguishes_invalid_input_from_io_failure() {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = write_workspace(temporary.path(), 2);
    let probe = || {
        Command::new(env!("CARGO_BIN_EXE_cwp-runtime"))
            .arg(&workspace)
            .arg("--only-runtime-selection")
            .output()
            .unwrap()
    };
    let rejected = probe();
    assert_eq!(rejected.status.code(), Some(1));
    let body: serde_json::Value = serde_json::from_slice(&rejected.stdout).unwrap();
    assert_eq!(body["error"]["code"], "invalid_workspace");
    fs::remove_file(&workspace).unwrap();
    let failed = probe();
    assert_eq!(failed.status.code(), Some(1));
    assert!(failed.stdout.is_empty());
}

#[test]
fn only_runtime_selection_rejects_an_unloadable_required_coworker() {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = write_workspace(temporary.path(), 1);
    let probe = || {
        Command::new(env!("CARGO_BIN_EXE_cwp-runtime"))
            .arg(&workspace)
            .arg("--only-runtime-selection")
            .env("WORKLOOP_CONFIGURATION", r#"{}"#)
            .env("XDG_STATE_HOME", temporary.path().join("state-home"))
            .env("XDG_RUNTIME_DIR", runtime_home(temporary.path()))
            .output()
            .unwrap()
    };
    // Imported read failures disable that coworker. A runtime that explicitly
    // requires it still reports a typed configuration rejection.
    fs::write(temporary.path().join("package/COWORKER.toml"), [0xff]).unwrap();
    let failed = probe();
    assert_eq!(failed.status.code(), Some(1));
    let body: serde_json::Value = serde_json::from_slice(&failed.stdout).unwrap();
    assert_eq!(body["error"]["code"], "invalid_workspace");
    // Malformed TOML is a configuration error, not an operational read error.
    fs::write(
        temporary.path().join("package/COWORKER.toml"),
        "invalid = [",
    )
    .unwrap();
    let rejected = probe();
    let body: serde_json::Value = serde_json::from_slice(&rejected.stdout).unwrap();
    assert_eq!(body["error"]["code"], "invalid_workspace");
    fs::remove_file(temporary.path().join("package/CWR.toml")).unwrap();
    let missing = probe();
    let body: serde_json::Value = serde_json::from_slice(&missing.stdout).unwrap();
    assert_eq!(body["error"]["code"], "invalid_workspace");
}

#[test]
fn graphql_bootstrap_reuses_only_the_selected_configured_resident() {
    use cwr::v2026_09::local::{
        configured_runtime_runtime_directory, LocalGraphqlEndpoint, LocalWorkspaceComposition,
    };
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let workspace = write_workspace(root, 2);
    let command = |name: &str, operation: &str| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cwp-runtime"));
        command
            .arg(&workspace)
            .args(["--runtime", name, operation])
            .env("WORKLOOP_CONFIGURATION", r#"{}"#)
            .env("XDG_STATE_HOME", root.join("state-home"))
            .env("XDG_RUNTIME_DIR", runtime_home(root))
            .env_remove("CWR_DEV_RUNTIME_GENERATION")
            .env_remove("CWR_DEV_RUNTIME_GENERATION_FILE");
        command
    };
    struct Cleanup<'a> {
        command: &'a dyn Fn(&str, &str) -> Command,
    }
    impl Drop for Cleanup<'_> {
        fn drop(&mut self) {
            for name in ["installed-0", "installed-1"] {
                let _ = (self.command)(name, "--stop-resident").output();
            }
        }
    }
    let _cleanup = Cleanup { command: &command };
    let composition = LocalWorkspaceComposition::load(&workspace).unwrap();
    let mut refs = Vec::new();
    for name in ["installed-0", "installed-1"] {
        let selected = composition.configured_runtime(name).unwrap();
        let key = configured_runtime_runtime_directory(&workspace, name).unwrap();
        let directory = runtime_home(root)
            .join("cwr/loops")
            .join(key.file_name().unwrap());
        let mut resident_pid = None;
        for _ in 0..2 {
            let result = command(name, "--ensure-resident")
                .args(["--ingress", "cwp://example.test/integrations/tui"])
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            assert!(result.stdout.is_empty());
            let pid = fs::read_to_string(directory.join("resident-runtime.pid")).unwrap();
            if let Some(previous) = &resident_pid {
                assert_eq!(&pid, previous);
            }
            resident_pid = Some(pid);
            let endpoint = LocalGraphqlEndpoint::read(&directory.join("graphql.json")).unwrap();
            assert_eq!(endpoint.runtime_ref, selected.runtime_ref);
            assert_eq!(
                endpoint.actor_ref,
                selected.prepare_resident().unwrap().acting_actor_ref
            );
            let client = reqwest::blocking::Client::builder()
                .unix_socket(endpoint.socket_path.clone())
                .no_proxy()
                .build()
                .unwrap();
            let opened: serde_json::Value = client
                .post(format!("{}/session", endpoint.url()))
                .bearer_auth(&endpoint.token)
                .json(&serde_json::json!({"actorRef":endpoint.actor_ref}))
                .send()
                .unwrap()
                .error_for_status()
                .unwrap()
                .json()
                .unwrap();
            let response: serde_json::Value = client
                .post(endpoint.url())
                .bearer_auth(opened["token"].as_str().unwrap())
                .json(&serde_json::json!({"query":"query {runtime{id}}"}))
                .send()
                .unwrap()
                .error_for_status()
                .unwrap()
                .json()
                .unwrap();
            assert_eq!(response["data"]["runtime"]["id"], selected.runtime_ref);
            client
                .delete(format!("{}/session", endpoint.url()))
                .bearer_auth(opened["token"].as_str().unwrap())
                .send()
                .unwrap()
                .error_for_status()
                .unwrap();
        }
        refs.push(selected.runtime_ref.clone());
    }
    assert_ne!(refs[0], refs[1]);
    // A changed selection must fail the resident's fingerprint handshake,
    // rather than silently connecting GraphQL to the previously loaded graph.
    fs::write(root.join("package/COWORKER.toml"),"type='cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname='Changed helper'\nslug='helper'\n").unwrap();
    let rejected = command("installed-0", "--ensure-resident")
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    for name in ["installed-0", "installed-1"] {
        assert!(command(name, "--stop-resident")
            .output()
            .unwrap()
            .status
            .success());
        let selected = composition.configured_runtime(name).unwrap();
        let diagnostics = crate::resident_diagnostic_text(
            selected.state_directory.join("diagnostics/wlpdb.sqlite3"),
        )
        .unwrap_or_default();
        assert!(
            !diagnostics.contains("host stdio closed before an acting actor was selected"),
            "{diagnostics}"
        );
    }
}

#[test]
#[ignore = "requires CWR_LEGACY_RUNTIME_EXECUTABLE built from revision ccaba8261f1c"]
fn graphql_bootstrap_replaces_an_older_configured_resident_without_resetting_state() {
    use cwr::v2026_09::local::configured_runtime_runtime_directory;
    let old = std::env::var_os("CWR_LEGACY_RUNTIME_EXECUTABLE").expect("old runtime executable");
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    let workspace = write_workspace(root, 1);
    let command = |executable: &std::ffi::OsStr| {
        let mut command = Command::new(executable);
        command
            .arg(&workspace)
            .args(["--runtime", "installed-0"])
            .env("WORKLOOP_CONFIGURATION", r#"{}"#)
            .env("XDG_STATE_HOME", root.join("state-home"))
            .env("XDG_RUNTIME_DIR", runtime_home(root))
            .env_remove("CWR_DEV_RUNTIME_GENERATION")
            .env_remove("CWR_RECONNECT_RUNTIME_GENERATION");
        command
    };
    let current = std::ffi::OsStr::new(env!("CARGO_BIN_EXE_cwp-runtime"));
    struct Cleanup<'a>(&'a dyn Fn(&std::ffi::OsStr) -> Command, &'a std::ffi::OsStr);
    impl Drop for Cleanup<'_> {
        fn drop(&mut self) {
            let _ = (self.0)(self.1).arg("--stop-resident").output();
        }
    }
    let _cleanup = Cleanup(&command, current);
    let mut bridge = command(&old)
        .arg("--host-stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = bridge.stdin.take().unwrap();
    writeln!(input, "{}", serde_json::json!({"kind":"inspect_workspace", "version":HOST_COMMAND_VERSION, "request_id":"old-inspect"})).unwrap();
    let mut output = BufReader::new(bridge.stdout.take().unwrap());
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    let HostEvent::WorkspaceInspected {
        default_actor_ref: Some(actor),
        workspace: inspected,
        ..
    } = serde_json::from_str::<HostEvent>(&line).unwrap()
    else {
        panic!("missing acting coworker")
    };
    writeln!(input, "{}", serde_json::json!({"kind":"select_actor", "version":HOST_COMMAND_VERSION, "request_id":"old-select", "actor_ref":actor})).unwrap();
    line.clear();
    output.read_line(&mut line).unwrap();
    assert!(
        matches!(
            serde_json::from_str::<HostEvent>(&line).unwrap(),
            HostEvent::ActorSelected { .. }
        ),
        "{line}"
    );
    let runtime_ref = inspected.runtime_ref;
    writeln!(input, "{}", serde_json::json!({"kind":"create_conversation", "version":HOST_COMMAND_VERSION, "request_id":"old-message", "source_ref":format!("{runtime_ref}/sources/conversations"), "author_actor_ref":actor, "text":"History retained across the upgrade"})).unwrap();
    let conversation_ref = loop {
        line.clear();
        assert_ne!(output.read_line(&mut line).unwrap(), 0);
        match serde_json::from_str::<HostEvent>(&line).unwrap() {
            HostEvent::CommandSucceeded {
                request_id,
                receipt,
                ..
            } if request_id == "old-message" => break receipt.conversation_ref,
            HostEvent::CommandFailed { error, .. } => panic!("{error}"),
            _ => {}
        }
    };
    let key = configured_runtime_runtime_directory(&workspace, "installed-0").unwrap();
    let directory = runtime_home(root)
        .join("cwr/loops")
        .join(key.file_name().unwrap());
    let before = fs::read_to_string(directory.join("resident-runtime.pid")).unwrap();
    // Persist a workspace input alongside the resident state; replacement must retain both.
    fs::write(root.join("customer/kept.txt"), "retained workspace input").unwrap();
    let database = root
        .join(if cfg!(target_os = "macos") {
            "state-home/CWR/loops"
        } else {
            "state-home/cwr/loops"
        })
        .join(key.file_name().unwrap())
        .join("cwpdb.sqlite3");
    let database = fs::read_dir(database.parent().unwrap().parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().path().join("cwpdb.sqlite3"))
        .find(|path| path.exists())
        .unwrap();
    let before_database = fs::metadata(&database).unwrap();
    use std::os::unix::fs::MetadataExt;
    let result = command(current).arg("--ensure-resident").output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("Replacing an older configured runtime")
    );
    drop(input);
    let _ = bridge.wait().unwrap();
    let after = fs::read_to_string(directory.join("resident-runtime.pid")).unwrap();
    assert_ne!(before, after);
    assert_eq!(
        before_database.ino(),
        fs::metadata(&database).unwrap().ino()
    );
    assert_eq!(
        fs::read_to_string(root.join("customer/kept.txt")).unwrap(),
        "retained workspace input"
    );
    // A second launch reuses the upgraded owner.
    assert!(command(current)
        .arg("--ensure-resident")
        .output()
        .unwrap()
        .status
        .success());
    assert_eq!(
        after,
        fs::read_to_string(directory.join("resident-runtime.pid")).unwrap()
    );
    drop(_cleanup);
    let retained = cwr::v2026_09::conversation::ConversationAdmissionStore::open_managed(
        &database,
        &runtime_ref,
    )
    .unwrap();
    assert!(retained
        .conversation(&conversation_ref)
        .unwrap()
        .messages
        .iter()
        .any(|message| message.text == "History retained across the upgrade"));
    drop(retained);
}
