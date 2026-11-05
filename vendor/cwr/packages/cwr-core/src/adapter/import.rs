//! Dispatch an explicit import without initializing storage first.
use crate::CwrResult;
use std::{
    io::{Read as _, Seek as _},
    os::unix::process::CommandExt as _,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// The provider owns import semantics and its JSON report. A preview must not
/// call configure_adapter: that would initialize the home before planning.
pub fn import_adapter_home(
    adapter_ref: &str,
    source: &Path,
    dry_run: bool,
    replace: bool,
) -> CwrResult<serde_json::Value> {
    if adapter_ref != super::CODEX_ADAPTER_REF {
        return Err("Adapter import is unavailable".into());
    }
    if !source.is_absolute() {
        return Err("Codex import source must be absolute".into());
    }
    let directory = super::adapter_data_directory_path("codex")?;
    let executable =
        super::sibling_adapter_executable("CWR_CODEX_ADAPTER_EXECUTABLE", "cwr-adapter-codex")?;
    let mut output = tempfile::tempfile()?;
    let mut errors = tempfile::tempfile()?;
    let mut command = Command::new(executable);
    command.arg("--import-home").arg(source);
    if dry_run {
        command.arg("--dry-run");
    }
    if replace {
        command.arg("--replace");
    }
    let mut child = command
        .env("CWR_ADAPTER_DATA_DIRECTORY", &directory)
        .stdin(Stdio::null())
        .stdout(output.try_clone()?)
        .stderr(errors.try_clone()?)
        .process_group(0)
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(300);
    const OUTPUT_LIMIT: u64 = 4 * 1024 * 1024;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline
            || output.metadata()?.len() > OUTPUT_LIMIT
            || errors.metadata()?.len() > 16 * 1024
        {
            super::terminate_probe_group(&mut child);
            return Err("Adapter import timed out or exceeded its output limit".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    if !status.success() {
        errors.rewind()?;
        let mut bytes = Vec::new();
        errors.take(16 * 1024).read_to_end(&mut bytes)?;
        let diagnostic = String::from_utf8_lossy(&bytes);
        // Preserve one sanitized diagnostic line, never a multiline source
        // excerpt from malformed configuration or arbitrary adapter output.
        let diagnostic: String = diagnostic
            .lines()
            .next()
            .unwrap_or("no diagnostic")
            .chars()
            .take(1000)
            .collect();
        return Err(format!("Adapter import failed: {diagnostic}").into());
    }
    if output.metadata()?.len() > OUTPUT_LIMIT || errors.metadata()?.len() > 16 * 1024 {
        return Err("Adapter import exceeded its output limit".into());
    }
    output.rewind()?;
    let mut bytes = Vec::new();
    output.take(OUTPUT_LIMIT).read_to_end(&mut bytes)?;
    let report: serde_json::Value = serde_json::from_slice(&bytes)?;
    if !report.is_object() {
        return Err("Adapter returned an invalid import report".into());
    }
    Ok(report)
}
