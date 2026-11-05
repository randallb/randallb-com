impl ResolvedProcessAdapterRoute {
    /// Checks whether the selected provider can accept work in the current
    /// host environment without starting an invocation.
    pub fn readiness(&self) -> AdapterReadiness {
        self.readiness_for_store(if self.adapter_ref == CODEX_ADAPTER_REF {
            ProviderStore::System
        } else {
            ProviderStore::Managed
        })
    }

    /// A retained invocation can still depend on the provider home where its
    /// private session was created. New admissions use each provider's current
    /// default store.
    pub fn readiness_for_invocation(
        &self,
        invocation: &AdapterInvocation,
    ) -> CwrResult<AdapterReadiness> {
        if self.adapter_ref == CODEX_ADAPTER_REF || self.adapter_ref == CLAUDE_ADAPTER_REF {
            return Ok(
                self.readiness_for_store(if self.adapter_ref == CODEX_ADAPTER_REF {
                    codex_provider_store(invocation)?
                } else {
                    provider_store(invocation)?
                }),
            );
        }
        Ok(self.readiness())
    }

    fn readiness_for_store(&self, store: ProviderStore) -> AdapterReadiness {
        if self.actor_integration_params["execution_environment"] == "workloop_cloud" {
            return cloud_transport_readiness();
        }
        if self.adapter_ref == PERSON_ADAPTER_REF {
            return adapter_readiness(
                AdapterReadinessStatus::Ready,
                "awaiting_person",
                "Replies are answered in Workloop Desktop.",
            );
        }
        if !self.is_supported_by_local_host() {
            return adapter_readiness(
                AdapterReadinessStatus::SetupRequired,
                "adapter_implementation_missing",
                "This coworker is not available in this version of Workloop.",
            );
        }
        if self.adapter_ref == SH_ADAPTER_REF
            && self
                .actor_integration_params
                .get("execution_environment")
                .is_some()
        {
            return match resolve_smolvm_shell_route(self).and_then(|_| {
                sibling_adapter_executable(
                    "CWR_SMOLVM_SH_ADAPTER_EXECUTABLE",
                    "cwr-adapter-smolvm-sh",
                )
            }) {
                Ok(_) if std::env::var_os("HOME").is_some_and(|home| !home.is_empty()) => {
                    adapter_readiness(
                        AdapterReadinessStatus::Ready,
                        "ready",
                        "The SmolVM Shell provider is ready.",
                    )
                }
                Ok(_) => adapter_readiness(
                    AdapterReadinessStatus::SetupRequired,
                    "smolvm_home_missing",
                    "SmolVM needs a host home directory.",
                ),
                Err(error) => adapter_readiness(
                    AdapterReadinessStatus::SetupRequired,
                    "smolvm_setup_required",
                    format!("SmolVM Shell setup is incomplete: {error}"),
                ),
            };
        }
        if let Some(readiness) = adapter_implementation_readiness(&self.adapter_ref) {
            return readiness;
        }
        if self.adapter_ref == CODEX_ADAPTER_REF {
            let login = codex_login_readiness_for_store(store);
            return if login.is_ready() {
                if self.codex_model_configuration().is_none() {
                    return login;
                }
                match codex_model_catalog_for_store(self.working_path(), store) {
                    Ok(catalog) => codex_model_readiness(self, &catalog),
                    Err(error) => error,
                }
            } else {
                login
            };
        }
        if self.adapter_ref == CLAUDE_ADAPTER_REF {
            return claude_readiness(store);
        }
        if self.adapter_ref == DECISIONS_ADAPTER_REF {
            return decisions_readiness(&self.integration_params, || {
                std::env::var_os("OPENROUTER_API_KEY")
            });
        }
        if self.adapter_ref == SH_ADAPTER_REF {
            return shell_runner_readiness(&self.integration_params, &self.working_path);
        }
        adapter_readiness(
            AdapterReadinessStatus::Ready,
            "ready",
            "The provider is ready.",
        )
    }
}

pub fn codex_login_readiness() -> AdapterReadiness {
    codex_login_readiness_for_store(ProviderStore::System)
}

fn codex_login_readiness_for_store(store: ProviderStore) -> AdapterReadiness {
    let codex_home = match store {
        ProviderStore::Shared => match shared_codex_home() {
            Ok(home) => Some(home),
            Err(_) => return provider_home_unavailable("Codex"),
        },
        ProviderStore::Managed => match managed_provider_home("codex") {
            Ok(home) => Some(home),
            Err(_) => return provider_home_unavailable("Codex"),
        },
        ProviderStore::System | ProviderStore::Legacy => None,
    };
    let executable_override = std::env::var_os("CWR_CODEX_EXECUTABLE");
    let path = std::env::var_os("PATH");
    let readiness = codex_readiness(
        std::env::var_os("HOME"),
        std::env::var_os("USER"),
        executable_override.clone(),
        path.clone(),
        codex_home.as_deref(),
        Duration::from_secs(3),
    );
    codex_store_login_recovery(
        readiness,
        store,
        codex_home.as_deref(),
        codex_executable(executable_override, path.as_deref()),
    )
}

fn codex_store_login_recovery(
    mut readiness: AdapterReadiness,
    store: ProviderStore,
    home: Option<&Path>,
    executable: Option<PathBuf>,
) -> AdapterReadiness {
    match (store, home, executable) {
        (ProviderStore::System, None, Some(_)) if readiness.code == "provider_login_required" => {
            readiness.repair = Some(AdapterRepairOffer {
                kind: AdapterRepairKind::ProviderLogin,
                label: "Sign in".into(),
            });
            readiness.message =
                "Codex is installed but not signed in. Open AI provider settings to sign in, then retry."
                    .into();
        }
        (ProviderStore::Managed | ProviderStore::Shared, Some(home), Some(executable))
            if readiness.code == "provider_login_required" =>
        {
            // Global Settings repairs the system home, which cannot restore
            // authentication for a retained session in another home.
            readiness.repair = None;
            readiness.message = format!(
                "This conversation needs its original Codex sign-in. Run {} in Terminal, then retry.",
                retained_codex_login_command(home, &executable),
            );
        }
        _ => {}
    }
    readiness
}

fn retained_codex_login_command(home: &Path, executable: &Path) -> String {
    let quote = |path: &Path| format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"));
    format!("CODEX_HOME={} {} login", quote(home), quote(executable))
}

fn provider_home_unavailable(provider: &str) -> AdapterReadiness {
    adapter_readiness(
        AdapterReadinessStatus::TemporarilyUnavailable,
        "provider_home_unavailable",
        format!("Workloop couldn’t prepare its {provider} sign-in directory. Retry in a moment."),
    )
}

fn shell_runner_readiness(integration_params: &Value, working_path: &Path) -> AdapterReadiness {
    let runner = match integration_params.get("runner") {
        Some(value) => value
            .as_array()
            .and_then(|values| values.first())
            .and_then(Value::as_str),
        None => Some("/bin/bash"),
    };
    let available = runner.is_some_and(|executable| {
        if executable.contains('/') {
            let path = Path::new(executable);
            is_executable_file(&if path.is_absolute() {
                path.to_owned()
            } else {
                working_path.join(path)
            })
        } else {
            std::env::var_os("PATH")
                .and_then(|path| find_executable_on_path(executable, &path))
                .is_some()
        }
    });
    if !available {
        return adapter_readiness(
            AdapterReadinessStatus::SetupRequired,
            "provider_executable_missing",
            "The configured shell runner is missing or not executable. Update the connection, then retry.",
        );
    }
    adapter_readiness(
        AdapterReadinessStatus::Ready,
        "ready",
        "The provider is ready.",
    )
}

fn adapter_implementation_readiness(adapter_ref: &str) -> Option<AdapterReadiness> {
    let executable = match adapter_ref {
        CODEX_ADAPTER_REF => ("CWR_CODEX_ADAPTER_EXECUTABLE", "cwr-adapter-codex"),
        CLAUDE_ADAPTER_REF => ("CWR_CLAUDE_ADAPTER_EXECUTABLE", "cwr-adapter-claude"),
        DECISIONS_ADAPTER_REF => ("CWR_DECISIONS_ADAPTER_EXECUTABLE", "cwr-adapter-decisions"),
        OPEN_RESPONSES_ADAPTER_REF => (
            "CWR_OPEN_RESPONSES_ADAPTER_EXECUTABLE",
            "cwr-adapter-open-responses",
        ),
        LOOPBACK_ADAPTER_REF => ("CWR_LOOPBACK_ADAPTER_EXECUTABLE", "cwr-adapter-loopback"),
        RNG_ADAPTER_REF => ("CWR_RNG_ADAPTER_EXECUTABLE", "cwr-adapter-rng"),
        SH_ADAPTER_REF => ("CWR_SH_ADAPTER_EXECUTABLE", "cwr-adapter-sh"),
        _ => return None,
    };
    sibling_adapter_executable(executable.0, executable.1)
        .is_err()
        .then(|| {
            adapter_readiness(
                AdapterReadinessStatus::SetupRequired,
                "adapter_executable_missing",
                "This Workloop installation is missing provider support. Reinstall or update Workloop, then retry.",
            )
        })
}

pub fn codex_adapter_implementation_readiness() -> Option<AdapterReadiness> {
    adapter_implementation_readiness(CODEX_ADAPTER_REF)
}

/// The setup surface uses the same managed Claude home and implementation gate
/// as new local adapter admissions, without requiring a workspace selection.
pub fn claude_managed_readiness() -> AdapterReadiness {
    adapter_implementation_readiness(CLAUDE_ADAPTER_REF)
        .unwrap_or_else(|| claude_readiness(ProviderStore::Managed))
}

fn claude_readiness(store: ProviderStore) -> AdapterReadiness {
    if std::env::var_os("HOME").is_none_or(|value| value.is_empty()) {
        return adapter_readiness(
            AdapterReadinessStatus::SetupRequired,
            "provider_home_missing",
            "Claude needs a home directory before it can be used.",
        );
    }
    let executable = claude_executable(
        std::env::var_os("CWR_CLAUDE_EXECUTABLE"),
        std::env::var_os("PATH"),
    );
    let Some(executable) = executable else {
        return adapter_readiness(
            AdapterReadinessStatus::SetupRequired,
            "provider_executable_missing",
            "Claude is required for this coworker but isn’t installed. Install Claude, sign in, then restart Workloop.",
        );
    };
    let claude_home = match store {
        ProviderStore::Shared | ProviderStore::System => {
            return provider_home_unavailable("Claude")
        }
        ProviderStore::Managed => match managed_provider_home("claude") {
            Ok(home) => Some(home),
            Err(_) => return provider_home_unavailable("Claude"),
        },
        ProviderStore::Legacy => std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from),
    };
    claude_auth_readiness(&executable, claude_home.as_deref(), Duration::from_secs(3))
}

fn claude_executable(override_path: Option<OsString>, path: Option<OsString>) -> Option<PathBuf> {
    match override_path {
        Some(path) => Some(PathBuf::from(path)).filter(|path| is_executable_file(path)),
        None => path.and_then(|path| find_executable_on_path("claude", &path)),
    }
}

fn adapter_readiness(
    status: AdapterReadinessStatus,
    code: impl Into<String>,
    message: impl Into<String>,
) -> AdapterReadiness {
    AdapterReadiness {
        status,
        code: code.into(),
        message: message.into(),
        repair: None,
    }
}

fn codex_login_required(message: impl Into<String>) -> AdapterReadiness {
    AdapterReadiness {
        status: AdapterReadinessStatus::SetupRequired,
        code: "provider_login_required".into(),
        message: message.into(),
        repair: Some(AdapterRepairOffer {
            kind: AdapterRepairKind::ProviderLogin,
            label: "Sign in".into(),
        }),
    }
}

fn codex_readiness(
    home: Option<OsString>,
    user: Option<OsString>,
    override_path: Option<OsString>,
    path: Option<OsString>,
    codex_home: Option<&Path>,
    timeout: Duration,
) -> AdapterReadiness {
    codex_readiness_with_group(
        home,
        user,
        override_path,
        path,
        codex_home,
        timeout,
        ProbeGroup::Own,
    )
}

/// Repair's final status check stays in the detached setup CLI's process
/// group, so a forced host stop also reaches a hung `codex login status`.
fn codex_readiness_inherited_group(
    home: Option<OsString>,
    user: Option<OsString>,
    override_path: Option<OsString>,
    path: Option<OsString>,
    codex_home: Option<&Path>,
    timeout: Duration,
) -> AdapterReadiness {
    codex_readiness_with_group(
        home,
        user,
        override_path,
        path,
        codex_home,
        timeout,
        ProbeGroup::Inherited,
    )
}

#[derive(Clone, Copy)]
enum ProbeGroup {
    Own,
    Inherited,
}

fn codex_readiness_with_group(
    home: Option<OsString>,
    user: Option<OsString>,
    override_path: Option<OsString>,
    path: Option<OsString>,
    codex_home: Option<&Path>,
    timeout: Duration,
    group: ProbeGroup,
) -> AdapterReadiness {
    let Some(home) = home.filter(|value| !value.is_empty()) else {
        return adapter_readiness(
            AdapterReadinessStatus::SetupRequired,
            "provider_home_missing",
            "Codex needs a home directory before it can be used.",
        );
    };
    let Some(executable) = codex_executable(override_path, path.as_deref()) else {
        return codex_installation_required();
    };
    let mut command = Command::new(executable);
    command
        .env_clear()
        .env("HOME", &home)
        .env("PATH", path.unwrap_or_default())
        .args(["login", "status"])
        .current_dir(&home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if matches!(group, ProbeGroup::Own) {
        command.process_group(0);
    }
    if let Some(codex_home) = codex_home {
        command.env("CODEX_HOME", codex_home);
    }
    if let Some(user) = user.filter(|value| !value.is_empty()) {
        command.env("USER", user);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => return codex_temporarily_unavailable(),
    };
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                return adapter_readiness(
                    AdapterReadinessStatus::Ready,
                    "ready",
                    "Codex is installed and signed in.",
                );
            }
            Ok(Some(status)) if status.code() == Some(1) => {
                return if codex_home.is_some() {
                    codex_login_required(
                        "Codex is installed but not signed in to Workloop. Open AI provider settings to sign in, then retry.",
                    )
                } else {
                    adapter_readiness(
                        AdapterReadinessStatus::SetupRequired,
                        "provider_login_required",
                        "This older Codex invocation needs its original sign-in. Run codex login in Terminal, then retry.",
                    )
                };
            }
            Ok(Some(_)) => return codex_temporarily_unavailable(),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                terminate_codex_status_probe(&mut child, group);
                return codex_temporarily_unavailable();
            }
            Err(_) => {
                terminate_codex_status_probe(&mut child, group);
                return codex_temporarily_unavailable();
            }
        }
    }
}

fn terminate_codex_status_probe(child: &mut Child, group: ProbeGroup) {
    match group {
        ProbeGroup::Own => terminate_probe_group(child),
        ProbeGroup::Inherited => {
            // This probe shares the setup CLI's group. Stop only this child;
            // the host owns the group if it has to force-stop the CLI.
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn codex_executable(override_path: Option<OsString>, path: Option<&OsStr>) -> Option<PathBuf> {
    match override_path {
        Some(path) => Some(PathBuf::from(path)).filter(|path| is_executable_file(path)),
        None => path.and_then(|path| find_executable_on_path("codex", path)),
    }
}

fn terminate_probe_group(child: &mut Child) {
    if let Ok(process_group) = i32::try_from(child.id()) {
        // SAFETY: the probe is started as the leader of its own process group.
        unsafe { libc::kill(-process_group, libc::SIGKILL) };
    }
    let _ = child.wait();
}

fn codex_installation_required() -> AdapterReadiness {
    adapter_readiness(
        AdapterReadinessStatus::SetupRequired,
        "provider_executable_missing",
        "Codex is required for this coworker but isn’t installed. Install Codex, sign in, then restart Workloop.",
    )
}

fn codex_temporarily_unavailable() -> AdapterReadiness {
    adapter_readiness(
        AdapterReadinessStatus::TemporarilyUnavailable,
        "provider_readiness_failed",
        "Workloop couldn’t check whether Codex is signed in. Retry in a moment.",
    )
}

fn decisions_readiness(
    params: &Value,
    host_key: impl FnOnce() -> Option<OsString>,
) -> AdapterReadiness {
    if params
        .get("endpoint")
        .and_then(Value::as_str)
        .is_none_or(|endpoint| decisions_endpoint(endpoint).is_err())
    {
        return adapter_readiness(
            AdapterReadinessStatus::SetupRequired,
            "decisions_endpoint_invalid",
            "Configure a valid Decisions endpoint, then retry.",
        );
    }
    if !decisions_loopback_endpoint(params) && decisions_api_key(params, true, host_key).is_none() {
        return adapter_readiness(
            AdapterReadinessStatus::SetupRequired,
            "decisions_credentials_missing",
            "Configure a private API key for this connection, then retry.",
        );
    }
    adapter_readiness(
        AdapterReadinessStatus::Ready,
        "ready",
        "The provider is ready.",
    )
}
