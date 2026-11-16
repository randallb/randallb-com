#[derive(Debug, Default)]
pub struct CodexModelCatalog {
    models: BTreeMap<String, BTreeSet<String>>,
}

pub fn codex_model_catalog(working_path: &Path) -> Result<CodexModelCatalog, AdapterReadiness> {
    codex_model_catalog_for_store(working_path, ProviderStore::System)
}

fn codex_model_catalog_for_store(
    working_path: &Path,
    store: ProviderStore,
) -> Result<CodexModelCatalog, AdapterReadiness> {
    let codex_home = match store {
        ProviderStore::Shared => {
            Some(shared_codex_home().map_err(|_| provider_home_unavailable("Codex"))?)
        }
        ProviderStore::Managed => {
            Some(managed_provider_home("codex").map_err(|_| provider_home_unavailable("Codex"))?)
        }
        ProviderStore::System | ProviderStore::Legacy => None,
    };
    let home = std::env::var_os("HOME").filter(|value| !value.is_empty());
    let path = std::env::var_os("PATH").filter(|value| !value.is_empty());
    let Some(home) = home else {
        return Err(adapter_readiness(
            AdapterReadinessStatus::SetupRequired,
            "provider_home_missing",
            "Codex needs a home directory before it can be used.",
        ));
    };
    let Some(executable) =
        codex_executable(std::env::var_os("CWR_CODEX_EXECUTABLE"), path.as_deref())
    else {
        return Err(codex_installation_required());
    };
    let mut command = Command::new(executable);
    command
        .env_clear()
        .env("HOME", &home)
        .env("PATH", path.unwrap_or_default())
        .args(["app-server", "--stdio"])
        .current_dir(working_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command.process_group(0);
    if let Some(codex_home) = codex_home {
        command.env("CODEX_HOME", codex_home);
    }
    if let Some(user) = std::env::var_os("USER").filter(|value| !value.is_empty()) {
        command.env("USER", user);
    }
    let mut child = command
        .spawn()
        .map_err(|_| codex_temporarily_unavailable())?;
    let result = read_codex_model_catalog(&mut child);
    terminate_probe_group(&mut child);
    result
}

fn read_codex_model_catalog(child: &mut Child) -> Result<CodexModelCatalog, AdapterReadiness> {
    let Some(mut input) = child.stdin.take() else {
        return Err(codex_temporarily_unavailable());
    };
    let Some(output) = child.stdout.take() else {
        return Err(codex_temporarily_unavailable());
    };
    let (sender, receiver) = mpsc::sync_channel(4);
    thread::spawn(move || {
        let reader = BufReader::new(output);
        for line in reader.lines() {
            if sender.send(line).is_err() {
                return;
            }
        }
    });
    if write_codex_request(&mut input, &json!({
        "id": "cwr-model-catalog-initialize",
        "method": "initialize",
        "params": {"clientInfo": {"name": "cwr", "title": "CWR", "version": env!("CARGO_PKG_VERSION")}, "capabilities": {"experimentalApi": true}}
    })).is_err() {
        return Err(codex_temporarily_unavailable());
    }
    let initialized = receive_codex_response(&receiver, "cwr-model-catalog-initialize")?;
    if initialized.get("error").is_some() {
        return Err(codex_temporarily_unavailable());
    }
    if write_codex_request(&mut input, &json!({"method": "initialized", "params": {}})).is_err() {
        return Err(codex_temporarily_unavailable());
    }

    let mut catalog = CodexModelCatalog::default();
    let mut cursor = None;
    let mut seen_cursors = BTreeSet::new();
    let mut page = 0;
    loop {
        page += 1;
        let request_id = format!("cwr-model-catalog-{page}");
        if write_codex_request(
            &mut input,
            &json!({
                "id": request_id,
                "method": "model/list",
                "params": {"cursor": cursor.as_deref(), "includeHidden": true}
            }),
        )
        .is_err()
        {
            return Err(codex_temporarily_unavailable());
        }
        let response = receive_codex_response(&receiver, &request_id)?;
        let Some(result) = response.get("result") else {
            return Err(codex_temporarily_unavailable());
        };
        let Some(data) = result.get("data").and_then(Value::as_array) else {
            return Err(codex_temporarily_unavailable());
        };
        for model in data {
            let Some(name) = model.get("model").and_then(Value::as_str) else {
                continue;
            };
            let efforts = model
                .get("supportedReasoningEfforts")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|effort| effort.get("reasoningEffort").and_then(Value::as_str))
                .map(str::to_owned)
                .collect();
            catalog.models.insert(name.to_owned(), efforts);
        }
        cursor = result
            .get("nextCursor")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if cursor
            .as_ref()
            .is_some_and(|cursor| !seen_cursors.insert(cursor.clone()))
        {
            return Err(codex_temporarily_unavailable());
        }
        if cursor.is_none() {
            return Ok(catalog);
        }
    }
}

fn write_codex_request(writer: &mut impl Write, value: &Value) -> io::Result<()> {
    serde_json::to_writer(&mut *writer, value)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

fn receive_codex_response(
    receiver: &mpsc::Receiver<io::Result<String>>,
    request_id: &str,
) -> Result<Value, AdapterReadiness> {
    let deadline = Duration::from_secs(5);
    let started = Instant::now();
    loop {
        let remaining = deadline
            .checked_sub(started.elapsed())
            .ok_or_else(codex_temporarily_unavailable)?;
        let line = receiver
            .recv_timeout(remaining)
            .map_err(|_| codex_temporarily_unavailable())?
            .map_err(|_| codex_temporarily_unavailable())?;
        let Ok(response) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if response.get("id").and_then(Value::as_str) == Some(request_id) {
            return Ok(response);
        }
    }
}

pub fn codex_model_readiness(
    route: &ResolvedProcessAdapterRoute,
    catalog: &CodexModelCatalog,
) -> AdapterReadiness {
    let Some((model, reasoning_effort)) = route.codex_model_configuration() else {
        return adapter_readiness(
            AdapterReadinessStatus::Ready,
            "ready",
            "Codex is installed and signed in.",
        );
    };
    let Some(efforts) = catalog.models.get(model) else {
        return adapter_readiness(
            AdapterReadinessStatus::SetupRequired,
            "provider_model_unavailable",
            format!("Codex is signed in, but {model} is not available to this account. Ask an administrator to enable it, then retry."),
        );
    };
    if !efforts.contains(reasoning_effort) {
        return adapter_readiness(
            AdapterReadinessStatus::SetupRequired,
            "provider_reasoning_effort_unsupported",
            format!("Codex model {model} does not support the configured {reasoning_effort} reasoning effort."),
        );
    }
    adapter_readiness(
        AdapterReadinessStatus::Ready,
        "ready",
        "Codex is installed, signed in, and supports this coworker’s model.",
    )
}
