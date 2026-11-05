use std::{fs, path::Path, process::Command};

fn fixture(root: &Path) {
    fs::create_dir(root.join("source")).unwrap();
    fs::create_dir(root.join("volume")).unwrap();
    fs::write(root.join("source/COWORKER.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname = 'Root'\nslug = 'root'\n").unwrap();
    fs::write(root.join("source/research.loop.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/loop'\n[[imports]]\npath = './COWORKER.toml'\n").unwrap();
    fs::write(root.join("WORKSPACE.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/workspace'\n[[volumes]]\nname = 'state'\npath = './volume'\n[[runtimes]]\nname = 'team'\npath = './source/research.loop.toml'\nsource_roots = ['./source']\nworking_directory = '.'\nvolume = 'state'\n").unwrap();
}

fn inspect(root: &Path) {
    for command in ["check", "coworkers", "actions"] {
        let mut process = Command::new(env!("CARGO_BIN_EXE_cwr"));
        process
            .current_dir(root)
            .env("XDG_STATE_HOME", root.join("state"))
            .args(["workspace", command, "WORKSPACE.toml"]);
        if command != "check" {
            process.arg("team");
        }
        let output = process.output().unwrap();
        assert!(output.status.success(), "{command}: {output:?}");
        if command == "coworkers" {
            assert!(String::from_utf8_lossy(&output.stdout).contains("@root"));
        }
        if command == "actions" {
            assert!(String::from_utf8_lossy(&output.stdout).contains("@cwr--list--coworkers"));
        }
    }
}

#[test]
fn workspace_inspection_does_not_create_or_require_writable_runtime_state() {
    let temporary = tempfile::tempdir().unwrap();
    fixture(temporary.path());
    inspect(temporary.path());
    let state = temporary.path().join("state");
    assert!(!state.exists());

    // A file prevents opening a database beneath .cwr even when tests run as root.
    fs::write(&state, "runtime state is unavailable").unwrap();
    inspect(temporary.path());
    assert_eq!(
        fs::read_to_string(state).unwrap(),
        "runtime state is unavailable"
    );
}

#[test]
fn workspace_cli_opens_a_named_loop_with_private_host_state() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    fixture(root);
    let output = Command::new(env!("CARGO_BIN_EXE_cwr"))
        .current_dir(root)
        .env("XDG_STATE_HOME", root.join("state"))
        .args([
            "workspace",
            "invoke",
            "WORKSPACE.toml",
            "team",
            "@cwr--list--coworkers",
            "{}",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(root.join("state").is_dir());
    assert!(!root.join("volume/.cwr").exists());
    inspect(root);
    let stopped = Command::new(Path::new(env!("CARGO_BIN_EXE_cwr")).with_file_name("cwp-runtime"))
        .current_dir(root)
        .env("XDG_STATE_HOME", root.join("state"))
        .args(["WORKSPACE.toml", "--runtime", "team", "--stop-resident"])
        .output()
        .unwrap();
    assert!(stopped.status.success(), "{stopped:?}");
}

#[path = "composition/resident.rs"]
mod resident;
