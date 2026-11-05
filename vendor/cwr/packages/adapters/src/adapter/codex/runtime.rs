//! Codex process ownership for one bounded adapter action.
use super::app_server::{
    run_bounded_action, spawn_provider_reader, BoundedActionConfig, BoundedActionHandler,
    BoundedActionResult,
};
use crate::CwrResult;
use std::{
    ffi::OsString,
    io,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};
pub(crate) struct CodexRuntime {
    codex_executable: PathBuf,
    home: OsString,
    codex_home: Option<OsString>,
    user: Option<OsString>,
    path: OsString,
    isolate_process_group: bool,
    provider_inactivity_timeout: Duration,
}
impl CodexRuntime {
    /// Use the process group already owned by the standalone adapter process.
    pub(crate) fn inherited_process_group(
        codex_executable: impl Into<PathBuf>,
        home: impl Into<OsString>,
        codex_home: Option<OsString>,
        user: Option<OsString>,
        path: impl Into<OsString>,
        provider_inactivity_timeout: Duration,
    ) -> Self {
        Self {
            codex_executable: codex_executable.into(),
            home: home.into(),
            codex_home,
            user,
            path: path.into(),
            isolate_process_group: false,
            provider_inactivity_timeout,
        }
    }

    fn provider_command(&self, workspace: &Path) -> Command {
        let mut command = Command::new(&self.codex_executable);
        command
            .env_clear()
            .env("HOME", &self.home)
            .env_remove("CODEX_SESSION_ID")
            .env("PATH", &self.path)
            .arg("app-server")
            .arg("--stdio")
            .current_dir(workspace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        if let Some(codex_home) = &self.codex_home {
            command.env("CODEX_HOME", codex_home);
        }
        if let Some(user) = &self.user {
            command.env("USER", user);
        }
        command
    }
}
pub(crate) fn run_codex_action<H>(
    runtime: &CodexRuntime,
    config: &mut BoundedActionConfig,
    handler: &mut H,
) -> CwrResult<BoundedActionResult>
where
    H: BoundedActionHandler,
{
    let mut command = runtime.provider_command(&config.workspace);
    let provider_started = std::time::Instant::now();
    let mut provider = ProviderProcess::start(&mut command, runtime.isolate_process_group)?;
    handler.on_timing(crate::adapter::AdapterTiming::single(
        crate::adapter::TimingCheckpoint::ProviderSpawned,
        crate::adapter::TimingDuration::ProviderSpawn,
        provider_started.elapsed().as_millis() as u64,
    ));
    let mut provider_input = provider
        .child
        .stdin
        .take()
        .ok_or_else(|| runtime_error("Codex app-server stdin is unavailable"))?;
    let provider_output = provider
        .child
        .stdout
        .take()
        .ok_or_else(|| runtime_error("Codex app-server stdout is unavailable"))?;
    let provider_lines = spawn_provider_reader(provider_output);
    run_bounded_action(
        &mut provider_input,
        &provider_lines,
        config,
        provider_started,
        handler,
        || Ok(provider.child.try_wait()?.is_some()),
        runtime.provider_inactivity_timeout,
    )
}
struct ProviderProcess {
    child: Child,
    isolated_process_group: Option<i32>,
}
impl ProviderProcess {
    fn start(command: &mut Command, isolate_process_group: bool) -> CwrResult<Self> {
        if isolate_process_group {
            // SAFETY: the pre-exec closure performs only the async-signal-safe `setsid` syscall
            // and constructs no state shared with the parent process.
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() == -1 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        let child = command.spawn()?;
        let isolated_process_group = isolate_process_group
            .then(|| {
                i32::try_from(child.id())
                    .map_err(|_| runtime_error("Codex app-server process identity is unavailable"))
            })
            .transpose()?;
        Ok(Self {
            child,
            isolated_process_group,
        })
    }
}
impl Drop for ProviderProcess {
    fn drop(&mut self) {
        if let Some(process_group) = self.isolated_process_group {
            let _ = stop_process_group(process_group);
        } else {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}
fn stop_process_group(process_group: i32) -> io::Result<()> {
    // SAFETY: `process_group` is the positive child pid captured after `setsid`; negating it
    // intentionally addresses that process group, and `kill` retains no Rust references.
    let result = unsafe { libc::kill(-process_group, libc::SIGKILL) };
    if result == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        return Ok(());
    }
    Err(error)
}
fn runtime_error(message: impl Into<String>) -> Box<dyn std::error::Error + Send + Sync> {
    io::Error::new(io::ErrorKind::InvalidData, message.into()).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn provider_command_uses_managed_codex_home_only_when_supplied() {
        for (codex_home, expected) in [
            (
                Some(OsString::from("/workloop/provider-homes/codex")),
                Some(OsStr::new("/workloop/provider-homes/codex")),
            ),
            (None, None),
        ] {
            let runtime = CodexRuntime::inherited_process_group(
                "/bin/codex",
                "/user",
                codex_home,
                None,
                "/usr/bin:/bin",
                Duration::from_secs(1),
            );
            let command = runtime.provider_command(Path::new("/workspace"));
            let actual = command
                .get_envs()
                .find(|(name, _)| name == &OsStr::new("CODEX_HOME"))
                .and_then(|(_, value)| value);
            assert_eq!(actual, expected);
        }
    }
}
