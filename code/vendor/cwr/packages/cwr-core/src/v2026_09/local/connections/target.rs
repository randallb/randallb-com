use super::*;

pub(in crate::v2026_09::local) struct ConnectionTarget {
    pub(super) path: PathBuf,
    pub(super) workspace: PathBuf,
    pub(super) grants_path: PathBuf,
    pub(in crate::v2026_09::local) options: graph::GraphLoadOptions,
}

pub(super) fn connection_target(path: &Path) -> CwrResult<ConnectionTarget> {
    if path.file_name().and_then(|name| name.to_str()) == Some("WORKSPACE.toml") {
        let composition = LocalWorkspaceComposition::load(path)?;
        let [configured] = composition.runtimes.as_slice() else {
            return invalid("Connections require a workspace with exactly one configured runtime.");
        };
        let (options, _supplied_inputs) = configured.connection_options()?;
        return Ok(ConnectionTarget {
            path: configured.cwr_toml.clone(),
            workspace: composition
                .path
                .parent()
                .ok_or("Workspace composition has no containing directory.")?
                .to_owned(),
            grants_path: composition.path,
            options,
        });
    }
    let path = canonical_cwr_path(path)?;
    let workspace = path
        .parent()
        .ok_or("Project has no workspace directory.")?
        .to_owned();
    Ok(ConnectionTarget {
        grants_path: path.clone(),
        path: path.clone(),
        workspace,
        options: graph::GraphLoadOptions {
            allow_partial: true,
            working_path: path.parent().map(Path::to_owned),
            imported_volumes: super::super::imported_volumes::load(&path)?,
            ..Default::default()
        },
    })
}

pub(in crate::v2026_09::local) fn configured_target(
    configured: &LocalConfiguredRuntime,
) -> CwrResult<ConnectionTarget> {
    let (options, _supplied_inputs) = configured.connection_options()?;
    Ok(ConnectionTarget {
        path: configured.cwr_toml.clone(),
        workspace: configured
            .workspace_path()
            .parent()
            .ok_or("Missing workspace directory.")?
            .into(),
        grants_path: configured.workspace_path().into(),
        options,
    })
}

pub(super) fn target_for_context(context: &InputContext) -> CwrResult<ConnectionTarget> {
    if context.runtime_ref.is_some()
        && context
            .grants_path
            .file_name()
            .is_some_and(|n| n == "WORKSPACE.toml")
    {
        let workspace = LocalWorkspaceComposition::load(&context.grants_path)?;
        let configured = workspace
            .runtimes
            .iter()
            .find(|r| Some(&r.runtime_ref) == context.runtime_ref.as_ref())
            .ok_or("Configured runtime changed.")?;
        let target = configured_target(configured)?;
        if target.path != context.path || target.workspace != context.workspace {
            return invalid("Workspace settings changed. Start this work again.");
        }
        Ok(target)
    } else {
        connection_target(&context.path)
    }
}
