#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ShellCommandRunner {
    #[serde(default = "default_shell_runner")]
    runner: Vec<String>,
    #[serde(default)]
    env: std::collections::BTreeMap<String, String>,
    #[serde(skip)]
    action_host_env: Option<std::ffi::OsString>,
    #[serde(skip)]
    activated_volume_env: Option<std::ffi::OsString>,
}

fn default_shell_runner() -> Vec<String> {
    vec!["/bin/bash".into(), "-c".into()]
}

impl ShellCommandRunner {
    fn from_params(params: Value) -> CwrResult<Self> {
        let runner: Self = serde_json::from_value(params)
            .map_err(|_| protocol_error("Shell integration params are invalid"))?;
        if runner.runner.first().is_none_or(String::is_empty) {
            return protocol_failure("Shell runner must start with a nonempty executable");
        }
        if runner.env.iter().any(|(name, value)| {
            name.is_empty() || name.starts_with("CWR_") || name.starts_with("LD_") || name.starts_with("DYLD_")
                || matches!(name.as_str(), "PATH" | "HOME" | "SHELL" | "ENV" | "BASH_ENV" | WORKLOOP_SMS_BRIDGE_SOCKET_ENV)
                || !name.bytes().enumerate().all(|(index, ch)| ch == b'_' || ch.is_ascii_alphabetic() || (index > 0 && ch.is_ascii_digit()))
                || value.contains('\0')
        }) {
            return protocol_failure("Shell integration environment contains an invalid or reserved name or value");
        }
        Ok(runner)
    }

    fn with_action_host_environment(
        self,
        params: &ShActorIntegrationParams,
        action_name: &str,
    ) -> CwrResult<Self> {
        self.with_action_host_environment_from(params, action_name, |name| std::env::var_os(name))
    }

    fn with_action_host_environment_from(
        mut self,
        params: &ShActorIntegrationParams,
        action_name: &str,
        host_env: impl FnOnce(&str) -> Option<std::ffi::OsString>,
    ) -> CwrResult<Self> {
        if params.host_env_actions.iter().any(|(action, name)| {
            !WORKLOOP_SMS_ACTIONS.contains(&action.as_str())
                || name != WORKLOOP_SMS_BRIDGE_SOCKET_ENV
        }) {
            return protocol_failure("Shell host_env_actions contains an unsupported action or host value");
        }
        if params.host_env_actions.contains_key(action_name) {
            let socket = host_env(WORKLOOP_SMS_BRIDGE_SOCKET_ENV)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| protocol_error("Workloop SMS bridge is unavailable"))?;
            if !std::path::Path::new(&socket).is_absolute() {
                return protocol_failure("Workloop SMS bridge path must be absolute");
            }
            self.action_host_env = Some(socket);
        }
        if !params.activated_volumes.is_empty() {
            for (alias, volume) in &params.activated_volumes {
                if alias.is_empty()
                    || !alias.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
                    })
                    || !std::path::Path::new(&volume.path).is_absolute()
                    || !matches!(volume.access.as_str(), "read" | "read_write")
                {
                    return protocol_failure("Shell activated volume mapping is invalid");
                }
            }
            self.activated_volume_env = Some(
                serde_json::to_string(&params.activated_volumes)?.into(),
            );
        }
        Ok(self)
    }

    fn command(&self, command: &str) -> Command {
        let mut child = Command::new(&self.runner[0]);
        child.args(&self.runner[1..]).arg(command);
        child.env_remove(WORKLOOP_SMS_BRIDGE_SOCKET_ENV);
        child.env_remove("CWR_LOCAL_VOLUMES");
        child.envs(&self.env);
        if let Some(socket) = &self.action_host_env {
            child.env(WORKLOOP_SMS_BRIDGE_SOCKET_ENV, socket);
        }
        if let Some(volumes) = &self.activated_volume_env {
            child.env("CWR_LOCAL_VOLUMES", volumes);
        }
        child
    }
}
