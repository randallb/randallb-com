use super::*;
pub(super) fn options(options: &OpenOptions) -> CwpdbResult<()> {
    if options.runtime_state_version != STATE && options.runtime_state_version != TIMELINE_STATE {
        return Err("unsupported CWR runtime state version".into());
    }
    if let Some(binding) = &options.workspace_binding {
        if !binding.workspace_directory.is_absolute() {
            return Err("CWR workspace binding path must be absolute".into());
        }
    }
    if let Some(stage) = &options.timeline_rollout {
        order(stage)?;
    }
    Ok(())
}
fn order(stage: &str) -> CwpdbResult<u8> {
    match stage {
        "disabled" => Ok(0),
        "new_conversations" => Ok(1),
        "migrate_legacy" => Ok(2),
        _ => Err("unsupported timeline rollout".into()),
    }
}
pub(super) fn workspace(
    db: &Connection,
    scope: &Scope,
    binding: &WorkspaceBinding,
) -> CwpdbResult<bool> {
    if !binding.workspace_directory.is_absolute() {
        return Err("CWR workspace binding path must be absolute".into());
    }
    let reference = format!("{}/workspace", root_ref(scope));
    let Some(stored) = crate::node_props(db, &reference)? else {
        return Ok(true);
    };
    if stored["workspace_id"] != binding.workspace_id {
        return Err("runtime database belongs to another workspace".into());
    }
    let bytes: Vec<u8> = serde_json::from_value(stored["workspace_path"].clone())?;
    #[cfg(unix)]
    let previous = {
        use std::os::unix::ffi::OsStringExt;
        PathBuf::from(std::ffi::OsString::from_vec(bytes))
    };
    #[cfg(not(unix))]
    let previous = PathBuf::from(String::from_utf8(bytes)?);
    if previous == binding.workspace_directory {
        return Ok(false);
    }
    if previous.try_exists()? {
        return Err(format!("workspace {} is already bound to its original directory {}; this appears to be a copied workspace and needs a new workspace_id", binding.workspace_id, previous.display()).into());
    }
    Ok(true)
}
pub(super) fn rollout(db: &Connection, scope: &Scope, stage: &str) -> CwpdbResult<bool> {
    let metadata = runtime_metadata(db, scope)?;
    let retained = metadata["conversation_timeline_rollout"]
        .as_str()
        .unwrap_or("disabled");
    if order(stage)? <= order(retained)? {
        return Ok(false);
    }
    let repeated:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM edges e JOIN nodes n ON n.ref_id=e.target_id WHERE e.kind='conversation_timeline_invocation' AND json_extract(n.props,'$.record.forked_from_invocation_ref') IS NULL GROUP BY e.target_id HAVING count(*)>1)",[],|row|row.get(0))?;
    if repeated {
        return Err("timeline rollout cannot manage repeated invocation placements".into());
    }
    Ok(true)
}
pub(super) fn needed(db: &Connection, scope: &Scope, options: &OpenOptions) -> CwpdbResult<bool> {
    validate_identity(db, scope, options)?;
    let metadata = runtime_metadata(db, scope)?;
    let mut needed = metadata.get("cloud_workspace_ref").is_none();
    if let Some(binding) = &options.workspace_binding {
        needed |= workspace(db, scope, binding)?;
    }
    if let Some(stage) = &options.timeline_rollout {
        needed |= rollout(db, scope, stage)?;
    }
    let fingerprint:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM nodes WHERE kind='admission' AND json_type(props,'$.record.source_input_fingerprint')='text')",[],|row|row.get(0))?;
    let attachment:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM edges WHERE kind='timeline_activity' AND json_extract(props,'$.record.event.kind')='work_update' AND json_type(props,'$.record.event.attachment') IS NOT NULL AND EXISTS(SELECT 1 FROM edges owned WHERE owned.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND owned.kind='conversation' AND owned.target_id=edges.source_id))",[root_ref(scope)],|row|row.get(0))?;
    Ok(needed || fingerprint || attachment)
}
