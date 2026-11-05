// The explicit Shell opt-in is deliberately small: an empty guest workspace,
// no host volume, bridge socket, or environment binding. Those require a
// separate, reviewed file and credential transfer policy.
fn validate_smolvm_shell_route(route: &ResolvedProcessAdapterRoute) -> CwrResult<()> {
    if route.actor_integration_params["execution_environment"] != "smolvm" {
        return protocol_failure("Unsupported Shell execution environment");
    }
    if route.actor_integration_params["host_gateway"] == true {
        return protocol_failure("SmolVM Shell execution does not support host gateway routing");
    }
    for name in ["host_env_actions", "activated_volumes"] {
        if route.actor_integration_params[name]
            .as_object()
            .is_some_and(|values| !values.is_empty())
        {
            return protocol_failure(format!(
                "SmolVM Shell execution does not support {name}"
            ));
        }
    }
    if route.integration_params["env"]
        .as_object()
        .is_some_and(|values| !values.is_empty())
    {
        return protocol_failure("SmolVM Shell execution does not support integration environment values");
    }
    if let Some(runner) = route.integration_params.get("runner") {
        if runner != &json!(["/bin/bash", "-c"]) {
            return protocol_failure("SmolVM Shell execution requires the guest /bin/bash runner");
        }
    }
    Ok(())
}

fn resolve_smolvm_shell_route(
    route: &ResolvedProcessAdapterRoute,
) -> CwrResult<(PathBuf, PathBuf, String)> {
    validate_smolvm_shell_route(route)?;
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        return protocol_failure("SmolVM Shell execution currently requires Apple Silicon macOS");
    }
    let executable = std::env::var_os("CWR_SMOLVM_EXECUTABLE")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| protocol_error("CWR_SMOLVM_EXECUTABLE is required for SmolVM Shell execution"))?;
    let executable = std::fs::canonicalize(&executable).map_err(|error| {
        protocol_error(format!("SmolVM executable is unavailable: {error}"))
    })?;
    if !is_executable_file(&executable) {
        return protocol_failure("SmolVM executable is not an executable regular file");
    }
    let version = smolvm_cli_version(&executable)?;
    // The product pilot uses the upstream binary exercised with the current
    // image. The experiment's older 1.7.1 path requires a separate patch.
    if version != "smolvm 1.32.2" {
        return protocol_failure(format!("Unsupported SmolVM version: {version}"));
    }
    let image = std::env::var_os("CWR_SMOLVM_IMAGE")
        .filter(|path| !path.is_empty())
        .ok_or_else(|| protocol_error("CWR_SMOLVM_IMAGE is required for SmolVM Shell execution"))?;
    let image = std::fs::canonicalize(PathBuf::from(image)).map_err(|error| {
        protocol_error(format!("SmolVM guest image is unavailable: {error}"))
    })?;
    if !image.is_file() {
        return protocol_failure("SmolVM guest image must be a regular file");
    }
    let image_sha256 = std::env::var("CWR_SMOLVM_IMAGE_SHA256").map_err(|_| {
        protocol_error("CWR_SMOLVM_IMAGE_SHA256 is required for SmolVM Shell execution")
    })?;
    if image_sha256.len() != 64 || !image_sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return protocol_failure("CWR_SMOLVM_IMAGE_SHA256 must be a 64-digit hex digest");
    }
    Ok((executable, image, image_sha256.to_ascii_lowercase()))
}

fn smolvm_cli_version(executable: &Path) -> CwrResult<String> {
    let mut child = Command::new(executable)
        .arg("--version")
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match child.try_wait()? {
            Some(status) if status.success() => {
                let mut output = String::new();
                child
                    .stdout
                    .take()
                    .ok_or_else(|| protocol_error("SmolVM version output is unavailable"))?
                    .take(128)
                    .read_to_string(&mut output)?;
                return Ok(output.trim().to_owned());
            }
            Some(_) => return protocol_failure("SmolVM version check failed"),
            None if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return protocol_failure("SmolVM version check timed out");
            }
        }
    }
}
