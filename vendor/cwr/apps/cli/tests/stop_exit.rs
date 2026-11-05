use std::{fs, path::Path, process::Command};

fn copy_directory(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("create copied fixture directory");
    for entry in fs::read_dir(source).expect("read fixture directory") {
        let entry = entry.expect("fixture entry");
        let target = destination.join(entry.file_name());
        if entry.file_type().expect("fixture entry type").is_dir() {
            copy_directory(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).expect("copy fixture file");
        }
    }
}

struct TestWorkspace {
    _temporary: tempfile::TempDir,
    path: std::path::PathBuf,
    state_home: std::path::PathBuf,
    runtime_home: std::path::PathBuf,
}

impl Drop for TestWorkspace {
    fn drop(&mut self) {
        let _ = Command::new(env!("CARGO_BIN_EXE_cwr"))
            .current_dir(&self.path)
            .env("XDG_STATE_HOME", &self.state_home)
            .env("XDG_RUNTIME_DIR", &self.runtime_home)
            .args(["runtime", "stop"])
            .output();
    }
}

fn copied_workspace() -> TestWorkspace {
    let temporary = tempfile::tempdir_in("/tmp").expect("temporary CLI workspace");
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/cwr-core/fixtures/contracts/agent-recovery-turn/workspace");
    let workspace = temporary.path().join("workspace");
    copy_directory(&source, &workspace);
    for name in ["workspace.json", "workspace.toml"] {
        match fs::remove_file(workspace.join(".cwr").join(name)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("remove the copied workspace identity: {error}"),
        }
    }
    TestWorkspace {
        state_home: temporary.path().join("state-home"),
        runtime_home: temporary.path().join("runtime-home"),
        _temporary: temporary,
        path: workspace,
    }
}

#[test]
fn failed_cwr_stop_is_reported_as_a_retained_coworker_result() {
    for (arguments, succeeds) in [
        (vec!["exec", "@cwr stop missing"], true),
        (
            vec![
                "actions",
                "invoke",
                "@cwr--stop--invocation",
                "--invocation-ref=missing",
            ],
            false,
        ),
    ] {
        let workspace = copied_workspace();
        let output = Command::new(env!("CARGO_BIN_EXE_cwr"))
            .current_dir(&workspace.path)
            .env("XDG_STATE_HOME", &workspace.state_home)
            .env("XDG_RUNTIME_DIR", &workspace.runtime_home)
            .args(&arguments)
            .output()
            .expect("run CWR CLI");

        let rendered = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            output.status.success(),
            succeeds,
            "{arguments:?} returned an unexpected exit status: {rendered}"
        );
        assert!(
            rendered.contains("invocation was not found") || rendered.contains("not found"),
            "{arguments:?} did not print the structured failure: {rendered}"
        );
    }
}
