use std::{fs, process::Command};

const BASE_CWR_TOML: &str = r#"type = "cwp://org.coworkerprotocol/2026-09/schemas"
"#;

#[test]
fn init_creates_a_valid_base_workspace_in_the_current_directory() {
    let temporary = tempfile::tempdir().expect("temporary workspace");
    let existing = temporary.path().join("README.md");
    fs::write(&existing, "existing project\n").expect("write existing project file");

    let output = Command::new(env!("CARGO_BIN_EXE_cwr"))
        .arg("init")
        .current_dir(temporary.path())
        .output()
        .expect("run cwr init");

    assert!(output.status.success(), "{output:?}");
    let manifest = temporary.path().join("CWR.toml");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!(
            "Initialized CWR workspace at {}\n",
            fs::canonicalize(&manifest)
                .expect("canonical manifest")
                .display()
        )
    );
    assert!(output.stderr.is_empty());
    assert_eq!(
        fs::read_to_string(&manifest).expect("read initialized manifest"),
        BASE_CWR_TOML
    );
    assert_eq!(
        fs::read_to_string(existing).expect("read existing project file"),
        "existing project\n"
    );
    cwr::v2026_09::graph::check(manifest).expect("initialized manifest is valid");
    assert!(!temporary.path().join(".cwr").exists());
}

#[test]
fn init_creates_an_explicit_workspace_directory() {
    let temporary = tempfile::tempdir().expect("temporary parent directory");
    let workspace = temporary.path().join("nested").join("workspace");

    let output = Command::new(env!("CARGO_BIN_EXE_cwr"))
        .args(["init", "nested/workspace"])
        .current_dir(temporary.path())
        .output()
        .expect("run cwr init with a directory");

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(workspace.join("CWR.toml")).expect("read initialized manifest"),
        BASE_CWR_TOML
    );
}

#[test]
fn init_refuses_to_overwrite_an_existing_manifest() {
    let temporary = tempfile::tempdir().expect("temporary workspace");
    let manifest = temporary.path().join("CWR.toml");
    let existing = "existing manifest\n";
    fs::write(&manifest, existing).expect("write existing manifest");

    let output = Command::new(env!("CARGO_BIN_EXE_cwr"))
        .arg("init")
        .current_dir(temporary.path())
        .output()
        .expect("run cwr init against an existing manifest");

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        format!(
            "cwr: {} already exists\n",
            fs::canonicalize(&manifest)
                .expect("canonical existing manifest")
                .display()
        )
    );
    assert_eq!(
        fs::read_to_string(manifest).expect("read preserved manifest"),
        existing
    );
}
