fn read_toml_json(path: &Path) -> CwrResult<Value> {
    let contents = read_declaration_text(path)?;
    let value: toml::Value = toml::from_str(&contents)
        .map_err(|error| invalid_error(format!("{}: {error}", path.display())))?;
    serde_json::to_value(value).map_err(Into::into)
}

fn read_declaration_text(path: &Path) -> CwrResult<String> {
    let source = pin_regular_file_parent(path)?;
    // Synced folders can briefly expose an empty file before its contents arrive.
    // Reopen relative to the pinned parent on each attempt, since the provider
    // may replace the file while an ancestor must not redirect the read.
    let mut delays_ms = [100, 200, 400, 800].into_iter();
    let mut retry_missing = false;
    loop {
        let failure = match source.open().and_then(|mut file| {
            let mut contents = String::new();
            std::io::Read::read_to_string(&mut file, &mut contents)?;
            Ok(contents)
        }) {
            Ok(contents) if !contents.trim().is_empty() => return Ok(contents),
            Ok(_) => {
                retry_missing = true;
                ("declaration file is empty after retrying the read".to_owned(), None)
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::Interrupted
                        | std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                ) || (retry_missing && error.kind() == std::io::ErrorKind::NotFound) =>
            {
                (format!("declaration file could not be read after retrying: {error}"), Some(error.kind()))
            }
            Err(error) => return Err(crate::error::configuration_file_error(path, error)),
        };
        let Some(delay_ms) = delays_ms.next() else {
            let message = format!(
                "{}: {}. If this file is in a synced folder, wait for it to finish downloading and try again.",
                path.display(), failure.0
            );
            return Err(match failure.1 {
                Some(kind) => Box::new(std::io::Error::new(kind, message)),
                None => invalid_error(message),
            });
        };
        std::thread::sleep(std::time::Duration::from_millis(delay_ms));
    }
}

fn document_type<'a>(path: &Path, document: &'a Value) -> CwrResult<&'a str> {
    document.get("type").and_then(Value::as_str).ok_or_else(|| {
        invalid_error(format!(
            "{}: a 2026-09 declaration must contain a string type",
            path.display()
        ))
    })
}

fn resolve_mount_ref(root_ref: &str, reference: &str) -> String {
    if !reference.starts_with("~@") || root_ref == "~@" {
        return reference.to_owned();
    }
    format!("{root_ref}{}", &reference[2..])
}

fn resolve_graph_ref(graph: &ResolvedGraph, scope_id: usize, reference: &str) -> String {
    let scope = &graph.scopes[scope_id];
    let contextual = resolve_mount_ref(&scope.root_ref, reference);
    match scope.lookup.get(&contextual) {
        Some(ResolvedTarget::Resource(index)) => graph.resources[*index].effective_ref.clone(),
        Some(ResolvedTarget::External(reference)) => reference.clone(),
        None => contextual,
    }
}

fn contextual_alias(root_ref: &str, alias: &str) -> String {
    resolve_mount_ref(root_ref, &format!("~@/{alias}"))
}

fn resource_of_type<'a>(
    source: &Path,
    reference: &str,
    expected: &str,
    graph: &'a ResolvedGraph,
    scope_id: usize,
    predicate: impl FnOnce(&ResourceKind) -> bool,
) -> CwrResult<&'a LoadedResource> {
    let target = graph.scopes[scope_id]
        .lookup
        .get(reference)
        .cloned()
        .or_else(|| {
            graph
                .resources
                .iter()
                .position(|resource| resource.effective_ref == reference)
                .map(ResolvedTarget::Resource)
        });
    let Some(target) = target else {
        return invalid(format!(
            "{}: unresolved {expected} ref {reference}",
            source.display()
        ));
    };
    let ResolvedTarget::Resource(index) = target else {
        return invalid(format!(
            "{}: {reference} resolves to an external resource, not a {expected}",
            source.display()
        ));
    };
    let resource = &graph.resources[index];
    if graph.disabled.contains(&resource.source) || !predicate(&resource.kind) {
        return invalid(format!(
            "{}: {reference} resolves to {}, not a {expected}",
            source.display(),
            resource.type_ref
        ));
    }
    Ok(resource)
}

fn empty_object() -> Value {
    Value::Object(serde_json::Map::new())
}

fn decode<T: for<'de> Deserialize<'de>>(path: &Path, value: Value) -> CwrResult<T> {
    serde_json::from_value(value)
        .map_err(|error| invalid_error(format!("{}: {error}", path.display())))
}

fn invalid<T>(message: impl Into<String>) -> CwrResult<T> {
    Err(invalid_error(message))
}

fn invalid_error(message: impl Into<String>) -> Box<dyn std::error::Error + Send + Sync> {
    Box::new(crate::ConfigurationError(message.into()))
}

// Authored identities are honored for types whose contract gives a stable ref.
// Other declaration kinds retain their existing source identity behavior.
fn declared_resource_ref(source: &Path, type_ref: &str, document: &Value) -> String {
    if matches!(type_ref, ADAPTER_TYPE | CONVERSATION_TYPE | ACTION_INVOCATION_TYPE | crate::entities::DEFINITION_TYPE | crate::entities::relationships::DEFINITION_TYPE) {
        if let Some(reference) = document.get("ref").and_then(Value::as_str) {
            return reference.to_owned();
        }
    }
    resolver::canonical_source_ref(source)
}
