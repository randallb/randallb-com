use std::{fs, path::PathBuf, process::Command};

fn repository_workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonical repository workspace")
}

#[test]
fn workspace_commands_discover_the_nearest_ancestor_manifest() {
    let workspace = repository_workspace();
    let nested = workspace.join("apps/cli/src");

    let checked = Command::new(env!("CARGO_BIN_EXE_cwr"))
        .args(["check", "~@"])
        .current_dir(&nested)
        .env_remove("CWR_WORKSPACE")
        .output()
        .expect("check discovered workspace");
    assert!(checked.status.success(), "{checked:?}");
    let graph = cwr::v2026_09::graph::resolve_graph(workspace.join("CWR.toml")).unwrap();
    assert!(String::from_utf8_lossy(&checked.stdout).contains(&graph.reference));

    let actions = Command::new(env!("CARGO_BIN_EXE_cwr"))
        .args(["actions", "list"])
        .current_dir(nested)
        .env_remove("CWR_WORKSPACE")
        .output()
        .expect("list actions from discovered workspace");
    assert!(actions.status.success(), "{actions:?}");
    assert!(String::from_utf8_lossy(&actions.stdout).contains("@deveng_ops--create--tmux_pane"));
}

#[test]
fn explicit_and_environment_workspace_selection_override_current_directory() {
    let workspace = repository_workspace();
    let outside = tempfile::tempdir().expect("outside directory");
    let graph = cwr::v2026_09::graph::resolve_graph(workspace.join("CWR.toml")).unwrap();
    let bff = &graph
        .actors
        .iter()
        .find(|actor| actor.coworker_slug == "bff")
        .unwrap()
        .actor_ref;

    let explicit = Command::new(env!("CARGO_BIN_EXE_cwr"))
        .args([
            "--workspace",
            workspace.to_str().expect("UTF-8 workspace"),
            "check",
            bff.as_str(),
        ])
        .current_dir(outside.path())
        .env_remove("CWR_WORKSPACE")
        .output()
        .expect("check explicit workspace");
    assert!(explicit.status.success(), "{explicit:?}");
    assert!(String::from_utf8_lossy(&explicit.stdout).contains(bff));

    let configured = Command::new(env!("CARGO_BIN_EXE_cwr"))
        .args(["check", bff.as_str()])
        .current_dir(outside.path())
        .env("CWR_WORKSPACE", &workspace)
        .output()
        .expect("check configured workspace");
    assert!(configured.status.success(), "{configured:?}");
    assert!(String::from_utf8_lossy(&configured.stdout).contains(bff));

    assert!(
        fs::read_dir(outside.path())
            .expect("read outside directory")
            .next()
            .is_none(),
        "workspace selection must not create state outside the selected workspace"
    );
}

#[test]
fn resource_checks_keep_workspace_roots_and_forwarded_inputs() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    fs::create_dir(root.join("coworkers")).unwrap();
    fs::write(root.join("CWR.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas'\n[[imports]]\npath = './coworkers/helper.toml'\nas = 'helper'\n[imports.inputs]\nteam = 'configured'\n").unwrap();
    fs::write(
        root.join("schema.json"),
        r#"{"type":"object","required":["team"],"properties":{"team":{"type":"string"}}}"#,
    )
    .unwrap();
    let person = "type = 'cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname = 'Helper'\n";
    let imports = "[[imports]]\npath = '../peer.toml'\nas = 'peer'\n";
    fs::write(
        root.join("coworkers/helper.toml"),
        person.to_owned() + "[input_schema]\npath = '../schema.json'\n" + imports,
    )
    .unwrap();
    fs::write(
        root.join("coworkers/unloaded.toml"),
        person.to_owned() + imports,
    )
    .unwrap();
    fs::write(root.join("peer.toml"), person.replace("Helper", "Peer")).unwrap();

    for input in [
        "coworkers/helper.toml",
        "coworkers/unloaded.toml",
        "~@/helper",
    ] {
        let checked = Command::new(env!("CARGO_BIN_EXE_cwr"))
            .args(["check", input])
            .current_dir(root)
            .env_remove("CWR_WORKSPACE")
            .output()
            .unwrap();
        assert!(checked.status.success(), "{input}: {checked:?}");
    }
    let outside = tempfile::tempdir().unwrap();
    let checked = Command::new(env!("CARGO_BIN_EXE_cwr"))
        .arg("--workspace")
        .arg(root)
        .arg("check")
        .arg(root.join("coworkers/helper.toml"))
        .current_dir(outside.path())
        .env_remove("CWR_WORKSPACE")
        .output()
        .unwrap();
    assert!(checked.status.success(), "{checked:?}");
    assert!(!root.join(".cwr").exists());
    assert!(fs::read_dir(outside.path()).unwrap().next().is_none());
}

#[test]
fn contextual_resource_checks_reject_invalid_bound_inputs_instead_of_using_empty_inputs() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    fs::write(root.join("CWR.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas'\n[[imports]]\npath = './helper.toml'\nas = 'helper'\n[imports.inputs]\nteam = 7\n").unwrap();
    fs::write(root.join("helper.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname = 'Helper'\n[input_schema]\npath = './schema.json'\n").unwrap();
    // Empty inputs pass this optional schema; the selected workspace's value does not.
    fs::write(
        root.join("schema.json"),
        r#"{"type":"object","properties":{"team":{"type":"string"}}}"#,
    )
    .unwrap();
    for (explicit, input) in [
        (false, "helper.toml"),
        (true, "helper.toml"),
        (false, "~@/helper"),
    ] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cwr"));
        command.current_dir(root).env_remove("CWR_WORKSPACE");
        if explicit {
            command.arg("--workspace").arg(root);
        }
        let checked = command.args(["check", input]).output().unwrap();
        assert!(
            !checked.status.success(),
            "explicit={explicit}: {checked:?}"
        );
        assert!(
            String::from_utf8_lossy(&checked.stderr)
                .contains("configuration inputs do not satisfy schema at /team"),
            "{checked:?}"
        );
    }
    assert!(!root.join(".cwr").exists());
}
