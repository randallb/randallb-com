fn parse_conversational_debug_invocation(remainder: &str) -> Option<Option<String>> {
    const PREFIX: &str = "debug invocation";
    let leading = remainder.get(..PREFIX.len())?;
    if !leading.eq_ignore_ascii_case(PREFIX) {
        return None;
    }
    let suffix = &remainder[PREFIX.len()..];
    if !suffix.is_empty() && !suffix.starts_with(char::is_whitespace) {
        return None;
    }
    Some(invocation_ref(suffix.trim()))
}

fn parse_conversational_stop_invocation(remainder: &str) -> Option<Option<String>> {
    for prefix in ["stop", "cancel"] {
        let Some(leading) = remainder.get(..prefix.len()) else {
            continue;
        };
        if !leading.eq_ignore_ascii_case(prefix) {
            continue;
        }
        let suffix = &remainder[prefix.len()..];
        if !suffix.is_empty() && !suffix.starts_with(char::is_whitespace) {
            continue;
        }
        return Some(invocation_ref(suffix.trim()));
    }
    None
}

fn parse_conversational_rename(remainder: &str) -> Option<String> {
    const PREFIX: &str = "rename conversation";
    let leading = remainder.get(..PREFIX.len())?;
    if !leading.eq_ignore_ascii_case(PREFIX) {
        return None;
    }
    let suffix = &remainder[PREFIX.len()..];
    if !suffix.starts_with(char::is_whitespace) {
        return None;
    }
    Some(unquote(suffix.trim()).to_owned())
}

fn parse_conversational_read_conversation(remainder: &str) -> Option<Option<String>> {
    const PREFIX: &str = "read conversation";
    let leading = remainder.get(..PREFIX.len())?;
    if !leading.eq_ignore_ascii_case(PREFIX) {
        return None;
    }
    let suffix = &remainder[PREFIX.len()..];
    if !suffix.is_empty() && !suffix.starts_with(char::is_whitespace) {
        return None;
    }
    Some(nonblank(suffix.trim()))
}

fn invocation_ref(value: &str) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    serde_json::from_str::<serde_json::Value>(value)
        .ok()
        .and_then(|value| {
            value
                .get("invocation_ref")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .or_else(|| Some(value.to_owned()))
}

fn coworkers_by_handle(
    catalog: &IntrinsicCwrCatalog,
) -> BTreeMap<&str, &IntrinsicCwrCatalogCoworker> {
    catalog
        .coworkers
        .iter()
        .map(|coworker| (coworker.handle.as_str(), coworker))
        .collect()
}

fn catalog_actions(catalog: &IntrinsicCwrCatalog) -> BTreeSet<String> {
    catalog
        .coworkers
        .iter()
        .flat_map(|coworker| coworker.actions.iter().cloned())
        .collect()
}
