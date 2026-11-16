use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn run(root: &Path, args: &[&str], credential: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cwr"));
    command
        .current_dir(root)
        .env("XDG_STATE_HOME", root.join("state"))
        .env_remove("CWR_TYPED_CLI_TEST_TOKEN")
        .args(args);
    if let Some(value) = credential {
        command.env("CWR_TYPED_CLI_TEST_TOKEN", value);
    }
    command.output().unwrap()
}

#[test]
fn report_and_execution_share_typed_selection_and_preserve_partial_readiness() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    fs::write(root.join("person.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname = 'Person'\nslug = 'person'\n").unwrap();
    let plain = "type = 'cwp://org.coworkerprotocol/2026-09/schemas/loop'\n[[imports]]\npath = './person.toml'\n";
    fs::write(root.join("ready.toml"), plain).unwrap();
    fs::write(root.join("secret.toml"), format!("{plain}[[inputs]]\nas = 'token'\ntype = 'cwp://org.coworkerprotocol/2026-09/schemas/secret'\n")).unwrap();
    fs::write(root.join("WORKSPACE.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/workspace'\n[[inputs]]\nas = 'credential'\ntype = 'cwp://org.coworkerprotocol/2026-09/schemas/secret'\n[bindings.credential]\nenv = 'CWR_TYPED_CLI_TEST_TOKEN'\n[[volumes]]\nname = 'files'\npath = '.'\n[[runtimes]]\nname = 'ready'\npath = './ready.toml'\nsource_roots = ['.']\nworking_directory = '.'\nvolume = 'files'\n[[runtimes]]\nname = 'secret'\npath = './secret.toml'\nsource_roots = ['.']\nworking_directory = '.'\nvolume = 'files'\n[[runtimes.inputs]]\nref = '~@/credential'\nas = 'token'\n").unwrap();
    let checked = run(
        root,
        &["workspace", "check", "WORKSPACE.toml", "--json"],
        Some("private-cli-value"),
    );
    assert!(!checked.status.success());
    let report: Value = serde_json::from_slice(&checked.stdout).unwrap();
    assert_eq!(report["readiness"], "partial");
    assert!(!String::from_utf8_lossy(&checked.stdout).contains("private-cli-value"));
    assert!(!root.join("state").exists());
    let invoke = |name, value| {
        run(
            root,
            &[
                "workspace",
                "invoke",
                "WORKSPACE.toml",
                name,
                "@cwr--list--coworkers",
                "{}",
            ],
            value,
        )
    };
    let missing = invoke("secret", None);
    assert!(!missing.status.success());
    assert!(!root.join("state").exists());
    let ready = invoke("ready", None);
    assert!(
        ready.status.success(),
        "{}",
        String::from_utf8_lossy(&ready.stderr)
    );
    let supplied = invoke("secret", Some("private-cli-value"));
    assert!(
        supplied.status.success(),
        "{}",
        String::from_utf8_lossy(&supplied.stderr)
    );
    assert!(!String::from_utf8_lossy(&supplied.stdout).contains("private-cli-value"));
}

#[test]
fn configured_graph_preserves_inputs_without_publishing_values() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    fs::write(root.join("CWR.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/runtime'\n[[inputs]]\nas = 'team'\ntype = 'cwp://org.coworkerprotocol/2026-09/schemas/plaintext'\nrequired = true\n").unwrap();
    fs::write(root.join("WORKSPACE.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/workspace'\n[[volumes]]\nname = 'files'\npath = '.'\n[[runtimes]]\nname = 'parent'\npath = './CWR.toml'\nsource_roots = ['.']\nworking_directory = '.'\nvolume = 'files'\n[runtimes.inputs]\nteam = 'private-configured-value'\n").unwrap();
    assert!(!run(root, &["check", "CWR.toml"], None).status.success());
    let graph = run(
        root,
        &["workspace", "graph", "WORKSPACE.toml", "parent"],
        None,
    );
    assert!(
        graph.status.success(),
        "{}",
        String::from_utf8_lossy(&graph.stderr)
    );
    let checked: Value = serde_json::from_slice(&graph.stdout).unwrap();
    assert!(checked["resources"].is_array());
    assert!(checked["actor_integrations"].is_array());
    assert!(!String::from_utf8_lossy(&graph.stdout).contains("private-configured-value"));
}
