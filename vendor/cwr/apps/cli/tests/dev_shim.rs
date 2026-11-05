#![cfg(unix)]

use std::{
    env, fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

struct ShimFixture {
    _temporary: tempfile::TempDir,
    repository_root: PathBuf,
    invocation_directory: PathBuf,
    path: std::ffi::OsString,
    cargo_log: PathBuf,
    execution_cwd: PathBuf,
    execution_arguments: PathBuf,
    execution_target_directory: PathBuf,
    runtime_generation: PathBuf,
    runtime_executable: PathBuf,
    runtime_generation_file: PathBuf,
    shim_state: PathBuf,
    target_directory: PathBuf,
}

impl ShimFixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().expect("create shim fixture");
        let fake_bin = temporary.path().join("bin");
        let invocation_directory = temporary.path().join("workspace with spaces");
        fs::create_dir_all(&fake_bin).expect("create fake executable directory");
        fs::create_dir_all(&invocation_directory).expect("create invocation directory");
        let invocation_directory = invocation_directory
            .canonicalize()
            .expect("canonicalize invocation directory");

        let cargo_log = temporary.path().join("cargo-arguments");
        let execution_cwd = temporary.path().join("execution-cwd");
        let execution_arguments = temporary.path().join("execution-arguments");
        let execution_target_directory = temporary.path().join("execution-target-directory");
        let runtime_generation = temporary.path().join("runtime-generation");
        let runtime_executable = temporary.path().join("runtime-executable");
        let runtime_generation_file = temporary.path().join("runtime-generation-file");
        let shim_state = temporary.path().join("shim-state");
        let target_directory = invocation_directory.join("clean-target");
        let fake_cargo = fake_bin.join("cargo");
        fs::write(
            &fake_cargo,
            r#"#!/bin/sh
set -eu
printf '%s\n' "$@" > "$SHIM_TEST_CARGO_LOG"
mkdir -p "$SHIM_TEST_TARGET_DIR/debug"
cat > "$SHIM_TEST_TARGET_DIR/debug/cwr" <<'CWR'
#!/bin/sh
set -eu
pwd -P > "$SHIM_TEST_EXEC_CWD"
printf '%s\n' "$@" > "$SHIM_TEST_EXEC_ARGS"
printf '%s\n' "$CARGO_TARGET_DIR" > "$SHIM_TEST_EXEC_TARGET_DIR"
printf '%s\n' "${CWR_DEV_RUNTIME_GENERATION-}" > "$SHIM_TEST_RUNTIME_GENERATION"
printf '%s\n' "${CWR_DEV_RUNTIME_EXECUTABLE-}" > "$SHIM_TEST_RUNTIME_EXECUTABLE"
printf '%s\n' "${CWR_DEV_RUNTIME_GENERATION_FILE-}" > "$SHIM_TEST_RUNTIME_GENERATION_FILE"
CWR
printf '%s\n' "new runtime" > "$SHIM_TEST_TARGET_DIR/debug/cwp-runtime"
for adapter in \
  cwr-adapter-claude \
  cwr-adapter-codex \
  cwr-adapter-loopback \
  cwr-adapter-open-responses \
  cwr-adapter-decisions \
  cwr-adapter-rng \
  cwr-adapter-sh
do
  printf '%s %s\n' "$adapter" "$SHIM_TEST_ADAPTER_VERSION" \
    > "$SHIM_TEST_TARGET_DIR/debug/$adapter"
done
chmod +x "$SHIM_TEST_TARGET_DIR/debug/cwr" \
  "$SHIM_TEST_TARGET_DIR/debug/cwp-runtime" \
  "$SHIM_TEST_TARGET_DIR/debug"/cwr-adapter-*
"#,
        )
        .expect("write fake cargo executable");
        let mut permissions = fs::metadata(&fake_cargo)
            .expect("read fake cargo metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&fake_cargo, permissions).expect("make fake cargo executable runnable");

        let repository_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("canonicalize repository root");
        let inherited_path = env::var_os("PATH").unwrap_or_default();
        let path = env::join_paths(
            std::iter::once(fake_bin.clone()).chain(env::split_paths(&inherited_path)),
        )
        .expect("compose shim test PATH");

        Self {
            _temporary: temporary,
            repository_root,
            invocation_directory,
            path,
            cargo_log,
            execution_cwd,
            execution_arguments,
            execution_target_directory,
            runtime_generation,
            runtime_executable,
            runtime_generation_file,
            shim_state,
            target_directory,
        }
    }

    fn command(&self, adapter_version: &str) -> Command {
        let mut command = Command::new(self.repository_root.join("bin/cwr"));
        command
            .current_dir(&self.invocation_directory)
            .env("PATH", &self.path)
            .env("CARGO_TARGET_DIR", "clean-target")
            .env("SHIM_TEST_CARGO_LOG", &self.cargo_log)
            .env("SHIM_TEST_TARGET_DIR", &self.target_directory)
            .env("SHIM_TEST_EXEC_CWD", &self.execution_cwd)
            .env("SHIM_TEST_EXEC_ARGS", &self.execution_arguments)
            .env(
                "SHIM_TEST_EXEC_TARGET_DIR",
                &self.execution_target_directory,
            )
            .env("SHIM_TEST_RUNTIME_GENERATION", &self.runtime_generation)
            .env("SHIM_TEST_RUNTIME_EXECUTABLE", &self.runtime_executable)
            .env("SHIM_TEST_ADAPTER_VERSION", adapter_version)
            .env(
                "SHIM_TEST_RUNTIME_GENERATION_FILE",
                &self.runtime_generation_file,
            )
            .env("CWR_DEV_SHIM_STATE_DIRECTORY", &self.shim_state)
            .env_remove("CARGO_BUILD_TARGET");
        command
    }
}

#[test]
fn repository_cwr_shim_builds_siblings_and_preserves_invocation_context() {
    let fixture = ShimFixture::new();
    let output = fixture
        .command("first")
        .args(["check", "workspace argument"])
        .env("CWR_DEV_RUNTIME_GENERATION", "inherited")
        .env("CWR_DEV_RUNTIME_EXECUTABLE", "/inherited")
        .output()
        .expect("run repository cwr shim");

    assert!(
        output.status.success(),
        "shim failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(&fixture.cargo_log).expect("read cargo arguments"),
        format!(
            "build\n--quiet\n--locked\n--workspace\n--bins\n--manifest-path\n{}\n--target-dir\n{}\n",
            fixture.repository_root.join("Cargo.toml").display(),
            fixture.target_directory.display()
        )
    );
    assert_eq!(
        fs::read_to_string(&fixture.execution_cwd).expect("read cwr execution directory"),
        format!("{}\n", fixture.invocation_directory.display())
    );
    assert_eq!(
        fs::read_to_string(&fixture.execution_arguments).expect("read cwr execution arguments"),
        "check\nworkspace argument\n"
    );
    assert_eq!(
        fs::read_to_string(&fixture.execution_target_directory)
            .expect("read cwr execution target directory"),
        format!("{}\n", fixture.target_directory.display())
    );
    let first_generation =
        fs::read_to_string(&fixture.runtime_generation).expect("read runtime generation");
    let fields = first_generation
        .split_ascii_whitespace()
        .collect::<Vec<_>>();
    assert_eq!(fields.len(), 4);
    assert_eq!(fields[1], "1");
    assert_eq!(
        fields[2],
        "b834f82e8cc9af29cb40f2f4d7a607fc7e6460bf5677cd759bd0f65b6b881098"
    );
    assert_eq!(fields[3], "12");
    let snapshot_runtime = Path::new(
        fs::read_to_string(&fixture.runtime_executable)
            .expect("read runtime executable")
            .trim(),
    )
    .to_owned();
    let snapshot_directory = snapshot_runtime
        .parent()
        .expect("runtime snapshot directory");
    for artifact in [
        "cwp-runtime",
        "cwr-adapter-claude",
        "cwr-adapter-codex",
        "cwr-adapter-loopback",
        "cwr-adapter-open-responses",
        "cwr-adapter-decisions",
        "cwr-adapter-rng",
        "cwr-adapter-sh",
    ] {
        assert!(
            snapshot_directory.join(artifact).is_file(),
            "snapshot is missing {artifact}"
        );
    }
    assert_eq!(
        fs::read_to_string(&fixture.runtime_generation_file).expect("read runtime generation file"),
        format!(
            "{}\n",
            fixture
                .shim_state
                .canonicalize()
                .expect("canonicalize shim state")
                .join("runtime-generation")
                .display()
        )
    );

    let unchanged = fixture
        .command("first")
        .arg("--help")
        .env("CWR_DEV_RUNTIME_GENERATION", "inherited")
        .env("CWR_DEV_RUNTIME_EXECUTABLE", "/inherited")
        .output()
        .expect("rerun repository cwr shim");
    assert!(unchanged.status.success());
    assert_eq!(
        fs::read_to_string(&fixture.runtime_generation).expect("read unchanged runtime generation"),
        first_generation
    );

    let changed_adapter = fixture
        .command("second")
        .arg("--help")
        .output()
        .expect("rerun repository cwr shim after adapter change");
    assert!(changed_adapter.status.success());
    let changed_generation =
        fs::read_to_string(&fixture.runtime_generation).expect("read changed runtime generation");
    let changed_fields = changed_generation
        .split_ascii_whitespace()
        .collect::<Vec<_>>();
    assert_eq!(changed_fields[0], fields[0]);
    assert_eq!(changed_fields[1], "2");
    assert_eq!(changed_fields[2], fields[2]);
    assert_eq!(changed_fields[3], fields[3]);
    assert_ne!(
        fs::read_to_string(&fixture.runtime_executable).expect("read changed runtime executable"),
        format!("{}\n", snapshot_runtime.display())
    );
}

#[test]
fn repository_cwr_shim_rejects_a_non_native_build_target() {
    let repository_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonicalize repository root");
    let output = Command::new(repository_root.join("bin/cwr"))
        .env("CARGO_BUILD_TARGET", "wasm32-unknown-unknown")
        .output()
        .expect("run repository cwr shim");

    assert!(!output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "cwr: the repository shim requires a native build; unset CARGO_BUILD_TARGET\n"
    );
}
