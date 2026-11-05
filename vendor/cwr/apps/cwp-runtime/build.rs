use std::{
    env,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

fn sl(args: &[&str]) -> Option<String> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = Command::new("sl")
        .args(args)
        .current_dir(root)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn main() {
    // Refresh for source or checkout changes, not every Cargo invocation: an
    // unchanged binary must not trigger a development resident replacement.
    println!("cargo:rerun-if-changed=src");
    let dirstate = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.sl/dirstate");
    if dirstate.exists() {
        println!("cargo:rerun-if-changed={}", dirstate.display());
    }
    if let Some(files) = sl(&["files"]) {
        for file in files.lines() {
            println!("cargo:rerun-if-changed=../../{file}");
        }
    }
    println!("cargo:rerun-if-env-changed=WORKLOOP_BUILD_REVISION");
    println!("cargo:rerun-if-env-changed=WORKLOOP_BUILD_DIRTY");
    let release_revision = env::var("WORKLOOP_BUILD_REVISION").ok();
    let release_dirty = env::var("WORKLOOP_BUILD_DIRTY").ok();
    let (revision, dirty) = if release_revision.is_some() || release_dirty.is_some() {
        let revision = release_revision.expect("release identity requires a revision");
        assert!(
            revision.len() == 40
                && revision
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')),
            "release identity requires a full lowercase revision"
        );
        let dirty = release_dirty.expect("release identity requires a dirty state");
        assert!(
            matches!(dirty.as_str(), "true" | "false"),
            "invalid release dirty state"
        );
        (Some(revision), Some(dirty))
    } else {
        // Dirty means tracked source changes. Unknown files cannot reliably
        // invalidate Cargo's build script and are not part of compiled source.
        (
            sl(&["log", "-r", ".", "-T", "{node}"]),
            sl(&["status", "-mard"]).map(|status| (!status.is_empty()).to_string()),
        )
    };
    if let Some(revision) = revision {
        if revision.len() == 40 && revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            println!("cargo:rustc-env=CWR_BUILD_REVISION={revision}");
        }
    }
    if let Some(dirty) = dirty {
        println!("cargo:rustc-env=CWR_BUILD_DIRTY={dirty}");
    }
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    println!("cargo:rustc-env=CWR_BUILD_TIME={time}");
    println!(
        "cargo:rustc-env=CWR_BUILD_PROFILE={}",
        env::var("PROFILE").unwrap()
    );
}
