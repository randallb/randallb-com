use super::{config::Config, error, mcp, trace, Input};
use crate::{adapter::read_adapter_message, CwrResult};
use serde_json::{json, Value};
use std::{
    env, fs,
    io::{BufReader, Write},
    os::unix::{net::UnixListener, process::CommandExt},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::SyncSender,
    },
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

static STOPPING: AtomicBool = AtomicBool::new(false);

extern "C" fn request_stop(_: libc::c_int) {
    STOPPING.store(true, Ordering::Relaxed);
}

pub(super) fn install_stop_handler() -> CwrResult<()> {
    // SAFETY: the handler only sets a lock-free atomic and retains no references.
    // This entrypoint runs in the dedicated adapter executable, before threads.
    if unsafe {
        libc::signal(
            libc::SIGTERM,
            request_stop as *const () as libc::sighandler_t,
        )
    } == libc::SIG_ERR
    {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

pub(super) fn stopping() -> bool {
    STOPPING.load(Ordering::Relaxed)
}

pub(super) struct Runtime {
    child: Child,
    _directory: TempDir,
    reaped: bool,
}

impl Runtime {
    pub fn spawn(config: &Config, events: SyncSender<Input>) -> CwrResult<Self> {
        // macOS Unix socket paths have a 104-byte limit; its per-user TMPDIR
        // often exceeds that. tempfile creates this directory with mode 0700.
        let directory = tempfile::Builder::new()
            .prefix("cwr-claude-")
            .tempdir_in("/tmp")?;
        let socket = directory.path().join("host.sock");
        let listener = UnixListener::bind(&socket)?;
        let prompt = directory.path().join("prompt.md");
        fs::write(&prompt, &config.prompt)?;
        let path = env::var_os("PATH")
            .filter(|value| !value.is_empty())
            .ok_or_else(|| error("Claude adapter requires an explicit PATH"))?;
        let mcp_config = json!({"mcpServers":{(config.tool_namespace.clone()):{"type":"stdio",
            "command":env::current_exe()?,"args":["--mcp",socket]}}});
        let mut command = Command::new(&config.integration.claude_executable);
        command
            .env_clear()
            .env("PATH", path)
            .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
            .env("ENABLE_TOOL_SEARCH", "false")
            .env("DISABLE_UPDATES", "1")
            .env("DISABLE_AUTOUPDATER", "1")
            .args([
                "-p",
                "--verbose",
                "--output-format",
                "stream-json",
                "--include-partial-messages",
                "--tools",
            ])
            .arg(config.native_tools.join(","))
            .args([
                "--strict-mcp-config",
                "--setting-sources",
                "",
                "--mcp-config",
            ])
            .arg(mcp_config.to_string())
            .arg("--append-system-prompt-file")
            .arg(prompt)
            .arg("--model")
            .arg(&config.params.model)
            .arg("--max-turns")
            .arg(config.params.max_turns.to_string())
            .current_dir(&config.workspace_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            // The provider and every MCP descendant belong to one cleanup unit.
            .process_group(0);
        if config.native_tools.iter().any(|name| name == "Bash") {
            command.arg("--dangerously-skip-permissions");
        } else {
            command.args(["--permission-mode", "dontAsk"]);
        }
        if config.integration.use_cli_login {
            // Bare mode skips the CLI's login/keychain. Login mode instead
            // disables ambient customizations while retaining authentication.
            command
                .args(["--disable-slash-commands", "--no-chrome"])
                .arg("--settings")
                .arg(
                    json!({
                        "disableAllHooks": true,
                        "autoMemoryEnabled": false,
                        "claudeMdExcludes": ["**"]
                    })
                    .to_string(),
                );
        } else {
            command.arg("--bare");
        }
        for name in [
            "HOME",
            "USER",
            "CLAUDE_CONFIG_DIR",
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "ANTHROPIC_BASE_URL",
            "CWR_CLAUDE_TRACE",
        ] {
            if let Some(value) = env::var_os(name) {
                command.env(name, value);
            }
        }
        if !config.tools.is_empty() || !config.native_tools.is_empty() {
            let mut names: Vec<_> = config
                .tools
                .iter()
                .map(|tool| {
                    format!(
                        "mcp__{}__{}",
                        config.tool_namespace,
                        tool["name"].as_str().expect("validated tool")
                    )
                })
                .collect();
            names.extend(config.native_tools.iter().cloned());
            command.arg("--allowedTools").arg(names.join(","));
        }
        if !config.native_text {
            command
                .arg("--json-schema")
                .arg(config.provider_return_schema.to_string());
        }
        if let Some(session) = &config.resume {
            command.arg("--resume").arg(session).arg("--fork-session");
        }
        let child = command.spawn()?;
        trace::emit("runtime", format!("provider spawned pid={}", child.id()));
        let mut runtime = Self {
            child,
            _directory: directory,
            reaped: false,
        };
        mcp::listen(listener, events.clone());
        trace::emit("runtime", "MCP listener started");
        runtime.start_io(config.observation.to_string(), events)?;
        Ok(runtime)
    }

    fn start_io(&mut self, observation: String, events: SyncSender<Input>) -> CwrResult<()> {
        let mut stdin = self
            .child
            .stdin
            .take()
            .ok_or_else(|| error("Claude stdin unavailable"))?;
        let stdout = self
            .child
            .stdout
            .take()
            .ok_or_else(|| error("Claude stdout unavailable"))?;
        let writer_events = events.clone();
        thread::spawn(move || {
            trace::emit("provider_stdin", "write started");
            if let Err(cause) = stdin.write_all(observation.as_bytes()) {
                trace::emit("provider_stdin", format!("write failed cause={cause}"));
                let _ = writer_events.send(Input::Failure(format!("Claude input failed: {cause}")));
            } else {
                trace::emit("provider_stdin", "write completed and stdin closed");
            }
        });
        thread::spawn(move || {
            trace::emit("provider_stdout", "reader started");
            let outcome = (|| -> CwrResult<()> {
                let mut reader = BufReader::new(stdout);
                while let Some(event) = read_adapter_message::<Value>(&mut reader)? {
                    events.send(Input::Provider(event))?;
                }
                trace::emit("provider_stdout", "EOF observed");
                events.send(Input::ProviderClosed)?;
                Ok(())
            })();
            if let Err(cause) = outcome {
                trace::emit("provider_stdout", format!("reader failed cause={cause}"));
                let _ = events.send(Input::Failure(format!("Claude output failed: {cause}")));
            }
        });
        Ok(())
    }

    pub fn finish(&mut self) -> CwrResult<()> {
        trace::emit(
            "runtime",
            format!("waiting for clean provider exit pid={}", self.child.id()),
        );
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if let Some(status) = self.child.try_wait()? {
                self.reaped = true;
                trace::emit(
                    "runtime",
                    format!("provider exited pid={} status={status}", self.child.id()),
                );
                return if status.success() {
                    Ok(())
                } else {
                    Err(error(format!("Claude exited with {status}")))
                };
            }
            if stopping() || Instant::now() >= deadline {
                trace::emit(
                    "runtime",
                    format!("provider clean-exit wait ended pid={}", self.child.id()),
                );
                return Err(error("Claude did not exit cleanly"));
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn signal_group(&self, signal: libc::c_int, name: &str) {
        // SAFETY: spawn places the provider in a group whose id is its positive pid.
        let result = unsafe { libc::kill(-(self.child.id() as i32), signal) };
        if result == 0 {
            trace::emit(
                "runtime",
                format!("{name} sent provider_group={}", self.child.id()),
            );
        } else {
            trace::emit(
                "runtime",
                format!(
                    "{name} provider_group={} result={}",
                    self.child.id(),
                    std::io::Error::last_os_error()
                ),
            );
        }
    }

    fn wait_after_signal(&mut self, signal: &str) -> bool {
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    trace::emit(
                        "runtime",
                        format!(
                            "provider reaped after {signal} pid={} status={status}",
                            self.child.id()
                        ),
                    );
                    return true;
                }
                Ok(None) => thread::sleep(Duration::from_millis(10)),
                Err(cause) => {
                    trace::emit(
                        "runtime",
                        format!(
                            "provider reap failed after {signal} pid={} cause={cause}",
                            self.child.id()
                        ),
                    );
                    return false;
                }
            }
        }
        trace::emit(
            "runtime",
            format!(
                "provider not reaped after {signal} pid={} within one second",
                self.child.id()
            ),
        );
        false
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        if self.reaped {
            trace::emit("runtime", "drop skipped for reaped provider");
            return;
        }
        trace::emit(
            "runtime",
            format!("drop starting provider cleanup pid={}", self.child.id()),
        );
        self.signal_group(libc::SIGTERM, "SIGTERM");
        if self.wait_after_signal("SIGTERM") {
            // The direct child may exit before a descendant handles SIGTERM.
            self.signal_group(libc::SIGKILL, "descendant SIGKILL");
            return;
        }
        self.signal_group(libc::SIGKILL, "SIGKILL");
        self.wait_after_signal("SIGKILL");
    }
}
