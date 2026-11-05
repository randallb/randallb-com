use super::{CwrResult, EnvironmentEntryKind};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct EntryRecord {
    pub(super) name: String,
    pub(super) kind: EnvironmentEntryKind,
    #[serde(default)]
    pub(super) value: Option<String>,
    #[serde(default)]
    pub(super) credential_id: Option<String>,
}

pub(super) fn validate_record(record: &EntryRecord) -> CwrResult<()> {
    validate_name(&record.name)?;
    match record.kind {
        EnvironmentEntryKind::Variable
            if record.value.is_some() && record.credential_id.is_none() =>
        {
            validate_value(record.value.as_deref().unwrap_or_default(), record.kind)?;
        }
        EnvironmentEntryKind::Secret
            if record.value.is_none() && record.credential_id.is_some() =>
        {
            crate::credentials::validate_id(record.credential_id.as_deref().unwrap_or_default())?;
        }
        _ => return Err("Environment entry has an unknown format.".into()),
    }
    Ok(())
}

pub(super) fn validate_name(name: &str) -> CwrResult<()> {
    let mut characters = name.chars();
    if name.len() > 128
        || !characters
            .next()
            .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        || !characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
    {
        return Err("Variable names must use letters, numbers, and underscores and cannot start with a number.".into());
    }
    Ok(())
}

pub(super) fn validate_value(value: &str, kind: EnvironmentEntryKind) -> CwrResult<()> {
    if value.len() > 16384
        || value.contains('\0')
        || (kind == EnvironmentEntryKind::Secret && value.is_empty())
    {
        return Err(
            "Values must contain at most 16 KiB and no null characters; secrets cannot be empty."
                .into(),
        );
    }
    Ok(())
}
