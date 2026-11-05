use super::importer::{ImportReport, SkippedImport};
use crate::CwrResult;
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

pub(super) fn merge(
    source_text: &str,
    destination_text: Option<&str>,
    source: &Path,
    destination: &Path,
    replace: bool,
    report: &mut ImportReport,
) -> CwrResult<Option<Vec<u8>>> {
    let mut source_value: toml::Value = toml::from_str(source_text)?;
    source_value
        .as_table()
        .ok_or("Codex configuration must be a TOML table")?;
    remove_global_model(&mut source_value, "config.toml", report);
    remove_runtime_settings(&mut source_value, "config.toml", report);
    if let Some(profiles) = source_value
        .get_mut("profiles")
        .and_then(toml::Value::as_table_mut)
    {
        for (name, profile) in profiles {
            remove_runtime_settings(profile, &format!("config.toml:profiles:{name}"), report);
        }
    }
    rebase(&mut source_value, &source.canonicalize()?, destination);
    let mut destination_value: toml::Value = match destination_text {
        Some(text) => toml::from_str(text)?,
        None => toml::Value::Table(Default::default()),
    };
    let before = destination_value.clone();
    remove_global_model(&mut destination_value, "config.toml", report);
    remove_runtime_settings(&mut destination_value, "config.toml", report);
    if let Some(profiles) = destination_value
        .get_mut("profiles")
        .and_then(toml::Value::as_table_mut)
    {
        for (name, profile) in profiles {
            remove_runtime_settings(profile, &format!("config.toml:profiles:{name}"), report);
        }
    }
    merge_value(
        &mut destination_value,
        source_value,
        "config.toml",
        replace,
        report,
    );
    if destination_value == before {
        return Ok(None);
    }
    Ok(Some(
        toml::to_string_pretty(&destination_value)?.into_bytes(),
    ))
}

fn remove_runtime_settings(value: &mut toml::Value, path: &str, report: &mut ImportReport) {
    if let Some(table) = value.as_table_mut() {
        for key in ["sqlite_home", "log_dir", "state_db", "codex_home"] {
            if table.remove(key).is_some() {
                report.skipped.push(SkippedImport {
                    path: format!("{path}:{key}").into(),
                    reason: "runtime setting".into(),
                });
            }
        }
    }
}

fn remove_global_model(value: &mut toml::Value, path: &str, report: &mut ImportReport) {
    // Desktop-selected Codex releases can have different model catalogs. Let
    // the selected provider choose its supported global default. Named profiles
    // remain portable and can keep their explicit model choices.
    if value
        .as_table_mut()
        .is_some_and(|table| table.remove("model").is_some())
    {
        report.skipped.push(SkippedImport {
            path: format!("{path}:model").into(),
            reason: "runtime setting".into(),
        });
    }
}

fn merge_value(
    destination: &mut toml::Value,
    source: toml::Value,
    path: &str,
    replace: bool,
    report: &mut ImportReport,
) {
    if let (Some(destination), Some(source)) = (destination.as_table_mut(), source.as_table()) {
        for (key, value) in source {
            let field = format!("{path}:{key}");
            if let Some(existing) = destination.get_mut(key) {
                merge_value(existing, value.clone(), &field, replace, report);
            } else {
                destination.insert(key.clone(), value.clone());
            }
        }
    } else if *destination != source {
        report.conflicts.push(path.into());
        if replace {
            *destination = source;
        }
    }
}

fn rebase(value: &mut toml::Value, source: &Path, destination: &Path) {
    match value {
        toml::Value::String(text) => {
            if let Some(path) = rebased_path(Path::new(text), source, destination) {
                *text = path.to_string_lossy().into_owned();
            }
        }
        toml::Value::Array(values) => {
            for value in values {
                rebase(value, source, destination);
            }
        }
        toml::Value::Table(values) => {
            let original = std::mem::take(values);
            for (key, mut value) in original {
                rebase(&mut value, source, destination);
                let key = rebased_path(Path::new(&key), source, destination)
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or(key);
                values.insert(key, value);
            }
        }
        _ => {}
    }
}

/// Resolve existing components before processing `..`: lexical normalization
/// alone changes the meaning of a parent traversal through a symlink. Missing
/// suffixes can still be rebased, but a dangling link or inaccessible component
/// leaves the original reference intact instead of guessing its destination.
fn rebased_path(path: &Path, source: &Path, destination: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut resolved = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                resolved.pop();
            }
            Component::CurDir => {}
            _ => resolved.push(component),
        }
        match fs::symlink_metadata(&resolved) {
            Ok(_) => resolved = resolved.canonicalize().ok()?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return None,
        }
    }
    resolved
        .strip_prefix(source)
        .ok()
        .map(|relative| destination.join(relative))
}
