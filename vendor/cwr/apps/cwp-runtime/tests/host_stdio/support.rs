use std::{
    fs,
    io::{BufRead, BufReader},
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, ExitStatus, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

use super::WORKSPACE_ID;

pub(super) fn start_runtime(directory: &Path) -> (RuntimeProcess, ChildStdin, Receiver<String>) {
    start_runtime_with_codex(directory, None)
}

pub(super) fn start_runtime_with_codex(
    directory: &Path,
    codex_executable: Option<&Path>,
) -> (RuntimeProcess, ChildStdin, Receiver<String>) {
    start_runtime_with_codex_and_storage(directory, codex_executable, None)
}

pub(super) fn start_runtime_with_codex_and_storage(
    directory: &Path,
    codex_executable: Option<&Path>,
    adapter_data_root: Option<&Path>,
) -> (RuntimeProcess, ChildStdin, Receiver<String>) {
    let workspace = directory.join("workspace");
    if !workspace.join("CWR.toml").exists() {
        copy_directory(fixture_workspace().parent().unwrap(), &workspace);
    }
    let workspace = workspace.join("CWR.toml");
    prepare_workspace_identity(&workspace);
    start_runtime_from_input(
        directory,
        &workspace,
        None,
        false,
        codex_executable,
        adapter_data_root,
    )
}

pub(super) fn start_runtime_for(
    directory: &Path,
    workspace: &Path,
    adapter: Option<&Path>,
) -> (RuntimeProcess, ChildStdin, Receiver<String>) {
    prepare_workspace_identity(workspace);
    let config = directory.join("runtime.toml");
    fs::write(
        &config,
        format!(
            "version = 1\nworkspace = {:?}\nexpected_communication_adapters = 0\n",
            workspace.to_string_lossy(),
        ),
    )
    .expect("write actor-free runtime configuration");
    start_runtime_from_input(directory, &config, adapter, false, None, None)
}

pub(super) fn start_configured_runtime_for(
    directory: &Path,
    workspace: &Path,
    communication_adapter: &Path,
    marker: &Path,
) -> (RuntimeProcess, ChildStdin, Receiver<String>) {
    prepare_workspace_identity(workspace);
    let config = directory.join("configured-runtime.toml");
    fs::write(
        &config,
        format!(
            "version = 1\nworkspace = {:?}\nstate_directory = {:?}\nruntime_directory = {:?}\nruntime_root_ref = \"cwp://example.test/configured-runtime\"\nacting_actor_ref = {:?}\n[[communication_adapters]]\ncommand = {:?}\nargs = [{:?}]\n",
            workspace.to_string_lossy(),
            directory.join("configured-state").to_string_lossy(),
            runtime_home(directory).to_string_lossy(),
            crate::test_refs::actor_ref("root"),
            communication_adapter.to_string_lossy(),
            marker.to_string_lossy(),
        ),
    )
    .expect("write configured runtime configuration");
    start_runtime_from_input(directory, &config, None, true, None, None)
}

pub(super) fn prepare_workspace_identity(workspace: &Path) {
    crate::test_refs::select_workspace(workspace);
    let workspace_directory = workspace.parent().expect("workspace manifest directory");
    fs::create_dir_all(workspace_directory.join(".cwr")).expect("create workspace identity dir");
    fs::write(
        workspace_directory.join(".cwr/workspace.json"),
        format!("{{\"version\":1,\"workspace_id\":\"{WORKSPACE_ID}\"}}\n"),
    )
    .expect("write test workspace identity");
}

fn start_runtime_from_input(
    directory: &Path,
    input: &Path,
    adapter: Option<&Path>,
    configured_runtime: bool,
    codex_executable: Option<&Path>,
    adapter_data_root: Option<&Path>,
) -> (RuntimeProcess, ChildStdin, Receiver<String>) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cwp-runtime"));
    command.arg(input).arg("--host-stdio");
    if configured_runtime {
        command.arg("--configured-runtime");
    }
    command
        .args(["--ingress", "cwp://example.test/integrations/test-host"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .env("WORKLOOP_CONFIGURATION", r#"{}"#)
        .env("XDG_STATE_HOME", directory.join("state-home"))
        .env("XDG_RUNTIME_DIR", runtime_home(directory));
    if let Some(adapter) = adapter {
        command.env("CWR_SH_ADAPTER_EXECUTABLE", adapter);
    }
    if let Some(codex_executable) = codex_executable {
        command.env("CWR_CODEX_EXECUTABLE", codex_executable);
    }
    if let Some(adapter_data_root) = adapter_data_root {
        command.env("CWR_ADAPTER_DATA_ROOT", adapter_data_root);
    }
    let mut child = RuntimeProcess(
        command
            .spawn()
            .expect("launch real runtime host entry point"),
    );
    let input = child.0.stdin.take().expect("runtime stdin");
    let output = child.0.stdout.take().expect("runtime stdout");
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(output).lines() {
            if sender.send(line.expect("read runtime output")).is_err() {
                break;
            }
        }
    });
    (child, input, receiver)
}

pub(super) fn stop_resident(directory: &Path) {
    let runtime = runtime_directory(directory);
    stop_resident_at(&runtime);
    fs::remove_dir_all(runtime_home(directory)).expect("remove test runtime directory");
}

pub(super) fn stop_configured_resident(directory: &Path) {
    let runtime = runtime_home(directory);
    stop_resident_at(&runtime);
    fs::remove_dir_all(runtime).expect("remove configured runtime directory");
}

pub(super) fn configured_runtime_directory(directory: &Path) -> PathBuf {
    runtime_home(directory)
}

fn stop_resident_at(runtime: &Path) {
    let pid = fs::read_to_string(runtime.join("resident-runtime.pid"))
        .expect("read resident PID")
        .trim()
        .to_owned();
    let status = Command::new("kill")
        .args(["-TERM", &pid])
        .status()
        .expect("request resident shutdown");
    assert!(status.success(), "resident must accept managed shutdown");
    let deadline = Instant::now() + Duration::from_secs(10);
    while runtime.join("runtime.sock").exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(!runtime.join("runtime.sock").exists());
}

pub(super) fn runtime_state_directory(directory: &Path) -> PathBuf {
    let application = if cfg!(any(target_os = "macos", target_os = "windows")) {
        "CWR"
    } else {
        "cwr"
    };
    directory
        .join("state-home")
        .join(application)
        .join("workspaces")
        .join(WORKSPACE_ID)
}

pub(super) fn runtime_directory(directory: &Path) -> PathBuf {
    runtime_home(directory)
        .join("cwr/workspaces")
        .join(WORKSPACE_ID)
}

pub(super) fn runtime_home(directory: &Path) -> PathBuf {
    PathBuf::from("/tmp").join(format!(
        "cwr-host-{}-{}",
        std::process::id(),
        directory
            .file_name()
            .expect("temporary directory name")
            .to_string_lossy()
    ))
}

fn fixture_workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/cwr-core/fixtures/contracts/agent-recovery-turn/workspace/CWR.toml")
}

fn copy_directory(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("create copied workspace directory");
    fs::set_permissions(destination, fs::Permissions::from_mode(0o700))
        .expect("make copied workspace private");
    for entry in fs::read_dir(source).expect("read fixture workspace") {
        let entry = entry.expect("read fixture entry");
        if entry.file_name() == ".cwr" {
            continue;
        }
        let target = destination.join(entry.file_name());
        if entry.file_type().expect("inspect fixture entry").is_dir() {
            copy_directory(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).expect("copy fixture file");
        }
    }
}

pub(super) struct RuntimeProcess(pub(super) Child);

impl RuntimeProcess {
    pub(super) fn wait_for_exit(&mut self, context: &str) -> ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(status) = self.0.try_wait().expect("inspect runtime process") {
                return status;
            }
            if Instant::now() >= deadline {
                let pid = self.0.id();
                let _ = self.0.kill();
                let status = self.0.wait().expect("reap timed-out runtime process");
                panic!(
                    "{context}: cwp-runtime pid {pid} did not exit within 15 seconds after stdin closed; killed with {status}"
                );
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for RuntimeProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
