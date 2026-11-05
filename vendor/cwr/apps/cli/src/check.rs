use std::{env, error::Error, ffi::OsStr, path::PathBuf};

use crate::workspace::{selected_workspace_manifest, CWR_WORKSPACE_ENV};
use cwr::v2026_09::graph::{self, EmptyResourceResolver, GraphLoadOptions};

pub fn run_gateway_fingerprint_command(
    connection_ref: &OsStr,
    runtime_ref: &OsStr,
    working_path: &OsStr,
    workspace: Option<&OsStr>,
    context: bool,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let connection_ref = connection_ref
        .to_str()
        .ok_or("connection ref must be UTF-8")?;
    let runtime_ref = runtime_ref.to_str().ok_or("runtime ref must be UTF-8")?;
    let working_path = PathBuf::from(working_path).canonicalize()?;
    if !working_path.is_dir() {
        return Err("working path must be a directory".into());
    }
    let graph = if let Some(composition_path) = workspace
        .map(PathBuf::from)
        .filter(|path| path.file_name() == Some(OsStr::new("WORKSPACE.toml")))
    {
        let composition = cwr::v2026_09::local::LocalWorkspaceComposition::load(composition_path)?;
        let selected = composition
            .runtimes
            .iter()
            .find(|configured| configured.runtime_ref == runtime_ref)
            .ok_or("runtime is not declared in this workspace")?;
        if selected.working_directory.canonicalize()? != working_path {
            return Err("working path does not match the configured runtime".into());
        }
        selected.resolve_graph()?
    } else {
        let manifest = selected_workspace_manifest(workspace)?;
        let mut options = cwr::v2026_09::local::local_graph_options(&manifest)?;
        options.runtime_ref = Some(runtime_ref.into());
        options.working_path = Some(working_path);
        graph::resolve_graph_with_options(&manifest, &EmptyResourceResolver, &options)?
    };
    let mut matches = graph
        .gateways
        .iter()
        .filter(|gateway| gateway.connection_ref.as_deref() == Some(connection_ref));
    let gateway = matches
        .next()
        .ok_or("gateway connection is not configured")?;
    if matches.next().is_some() {
        return Err("gateway connection selects more than one definition".into());
    }
    if context {
        println!(
            "{}",
            serde_json::json!({
                "approval_fingerprint": gateway.approval_fingerprint()?,
                "actors": graph.actors.iter().map(|actor| serde_json::json!({
                    "actor_ref": actor.actor_ref,
                    "default_conversation_target_ref": actor.default_conversation_target_ref,
                    "message_bindings": actor.message_binding_preference.iter().chain(actor.default_actor_integration_ref.iter()).filter(|reference|
                        graph.actor_integrations.iter().any(|binding| binding.actor_integration_ref == **reference && binding.is_available())
                    ).collect::<Vec<_>>()
                })).collect::<Vec<_>>()
            })
        );
    } else {
        println!("{}", gateway.approval_fingerprint()?);
    }
    Ok(())
}

pub fn run_check_command(
    input: &OsStr,
    workspace: Option<&OsStr>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let text = input.to_str().ok_or("check input must be valid UTF-8")?;
    let checked = if text.starts_with("cwp://") {
        let manifest = selected_workspace_manifest(workspace)?;
        let mut options = cwr::v2026_09::local::local_graph_options(&manifest)?;
        options.allow_partial = false;
        let graph = graph::resolve_graph_with_options(&manifest, &EmptyResourceResolver, &options)?;
        let resource = graph
            .resources
            .iter()
            .find(|resource| resource.reference == text)
            .ok_or_else(|| format!("resource {text} is not loaded in the selected runtime"))?;
        serde_json::json!({"type":resource.type_ref, "ref":resource.reference, "source":resource.source})
    } else {
        check_path(input, workspace)?
    };
    println!("{}", serde_json::to_string_pretty(&checked)?);
    Ok(())
}

fn check_path(
    input: &OsStr,
    workspace: Option<&OsStr>,
) -> Result<serde_json::Value, Box<dyn Error + Send + Sync>> {
    let path = resolve_check_input(input, workspace)?.canonicalize()?;
    let explicit_workspace = workspace.is_some()
        || env::var_os(CWR_WORKSPACE_ENV).is_some_and(|value| !value.is_empty());
    let manifest = match selected_workspace_manifest(workspace) {
        Ok(manifest) => Some(manifest),
        Err(error) if explicit_workspace => return Err(error),
        Err(_) => None,
    };
    let mut options = GraphLoadOptions::default();
    if let Some(manifest) = manifest {
        let root = manifest
            .parent()
            .ok_or("workspace manifest has no directory")?;
        if path.starts_with(root) {
            options.source_roots = vec![root.to_owned()];
        }
        // A loaded resource was validated with its locator's effective inputs.
        // Rechecking it as a new root would discard those forwarded values.
        if path != manifest && path.starts_with(root) {
            let mut workspace_options = cwr::v2026_09::local::local_graph_options(&manifest)?;
            workspace_options.source_roots = vec![root.to_owned()];
            workspace_options.allow_partial = false;
            let graph = graph::resolve_graph_with_options(
                &manifest,
                &EmptyResourceResolver,
                &workspace_options,
            )?;
            if let Some(resource) = graph
                .resources
                .iter()
                .find(|resource| resource.source == path)
            {
                return Ok(serde_json::json!({
                    "type": resource.type_ref, "ref": resource.reference, "source": resource.source
                }));
            }
        }
    }
    if path.file_name().is_some_and(|name| name == "CWR.toml") {
        let selected_source_roots = std::mem::take(&mut options.source_roots);
        options = cwr::v2026_09::local::local_graph_options(&path)?;
        if !selected_source_roots.is_empty() {
            options.source_roots = selected_source_roots;
        }
    }
    graph::check_with_options(path, &EmptyResourceResolver, &options)
}

fn resolve_check_input(
    input: &OsStr,
    workspace: Option<&OsStr>,
) -> Result<PathBuf, Box<dyn Error + Send + Sync>> {
    let text = input.to_str().ok_or("check input must be valid UTF-8")?;
    let mut path = if matches!(text, "~@" | "~@/" | "~@/CWR.toml") {
        return selected_workspace_manifest(workspace);
    } else if text.starts_with("~@/") {
        let manifest = selected_workspace_manifest(workspace)?;
        let mut options = cwr::v2026_09::local::local_graph_options(&manifest)?;
        options.allow_partial = false;
        return cwr::v2026_09::graph::resolve_resource_source_with_options(
            manifest,
            text,
            &EmptyResourceResolver,
            &options,
        );
    } else if text.starts_with("cwp:") {
        return Err(format!(
            "cwr check cannot resolve {text}; only mount-relative ~@ references are available"
        )
        .into());
    } else {
        PathBuf::from(input)
    };
    if path.is_dir() {
        path.push("CWR.toml");
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Mutex;

    static STATE_ENV_LOCK: Mutex<()> = Mutex::new(());

    struct StateHomeGuard(Option<std::ffi::OsString>);

    impl StateHomeGuard {
        fn set(path: &std::path::Path) -> Self {
            let previous = env::var_os("WORKLOOP_STATE_HOME");
            // SAFETY: tests using this process-wide setting are serialized by STATE_ENV_LOCK.
            unsafe { env::set_var("WORKLOOP_STATE_HOME", path) };
            Self(previous)
        }
    }

    impl Drop for StateHomeGuard {
        fn drop(&mut self) {
            if let Some(previous) = &self.0 {
                // SAFETY: the guard is dropped while STATE_ENV_LOCK is held.
                unsafe { env::set_var("WORKLOOP_STATE_HOME", previous) };
            } else {
                // SAFETY: the guard is dropped while STATE_ENV_LOCK is held.
                unsafe { env::remove_var("WORKLOOP_STATE_HOME") };
            }
        }
    }

    fn volume_workspace(root: &std::path::Path, name: &str) -> (PathBuf, PathBuf) {
        let workspace = root.join(name);
        let loop_path = workspace.join("loop/CWR.toml");
        let memory = workspace.join("memory");
        fs::create_dir_all(loop_path.parent().unwrap()).unwrap();
        fs::create_dir(&memory).unwrap();
        fs::write(
            workspace.join("CWR.toml"),
            format!(
                "type = {:?}\n[[imports]]\npath = './loop/CWR.toml'\nas = 'loop'\n",
                graph::ROOT_TYPE
            ),
        )
        .unwrap();
        fs::write(
            &loop_path,
            format!(
                "type = {:?}\n[[volumes]]\nas = 'memory'\naccess = 'read_write'\n",
                graph::RUNTIME_TYPE
            ),
        )
        .unwrap();
        let bindings_path =
            cwr::v2026_09::local::imported_volumes::bindings_path(&workspace.join("CWR.toml"))
                .unwrap();
        fs::create_dir_all(bindings_path.parent().unwrap()).unwrap();
        fs::write(
            bindings_path,
            serde_json::to_vec(&serde_json::json!({
                "version": 1,
                "bindings": [{"source": loop_path, "alias": "memory", "path": memory, "access": "read_write"}]
            }))
            .unwrap(),
        )
        .unwrap();
        (workspace.join("CWR.toml"), loop_path)
    }

    #[test]
    fn unrelated_path_check_uses_target_project_grants() {
        let _lock = STATE_ENV_LOCK.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        let _state = StateHomeGuard::set(root.path());
        let (workspace, _) = volume_workspace(root.path(), "workspace");
        let (project, _) = volume_workspace(root.path(), "other");
        assert!(check_path(project.as_os_str(), Some(workspace.as_os_str())).is_ok());
    }

    #[test]
    fn nested_manifest_check_keeps_selected_workspace_source_root() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("CWR.toml");
        let nested = root.path().join("loop/CWR.toml");
        fs::create_dir(root.path().join("loop")).unwrap();
        fs::create_dir(root.path().join("shared")).unwrap();
        fs::write(&workspace, format!("type = {:?}\n", graph::ROOT_TYPE)).unwrap();
        fs::write(
            &nested,
            format!(
                "type = {:?}\n[[imports]]\npath = '../shared/COWORKER.toml'\n",
                graph::RUNTIME_TYPE
            ),
        )
        .unwrap();
        fs::write(
            root.path().join("shared/COWORKER.toml"),
            "type = 'cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname = 'Shared'\nslug = 'shared'\n",
        )
        .unwrap();
        assert!(check_path(nested.as_os_str(), Some(workspace.as_os_str())).is_ok());
    }

    #[test]
    fn mount_relative_check_uses_selected_workspace_grants() {
        let _lock = STATE_ENV_LOCK.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        let _state = StateHomeGuard::set(root.path());
        let (workspace, loop_path) = volume_workspace(root.path(), "workspace");
        assert_eq!(
            resolve_check_input(OsStr::new("~@/loop"), Some(workspace.as_os_str())).unwrap(),
            loop_path.canonicalize().unwrap()
        );
        assert!(check_path(OsStr::new("~@/loop"), Some(workspace.as_os_str())).is_ok());
    }

    #[test]
    fn root_check_uses_approved_local_inputs_with_volume_grants() {
        let _lock = STATE_ENV_LOCK.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        let _state = StateHomeGuard::set(root.path());
        let (workspace, _) = volume_workspace(root.path(), "workspace");
        fs::write(
            &workspace,
            fs::read_to_string(&workspace).unwrap()
                + "[[inputs]]\nas = 'profile'\ntype = 'cwp://org.coworkerprotocol/2026-09/schemas/plaintext'\npurpose = 'Local profile'\n",
        )
        .unwrap();
        let report =
            cwr::v2026_09::local::connections::inspect_project_connections(&workspace).unwrap();
        cwr::v2026_09::local::connections::save_plaintext_connection(
            &workspace,
            &report.fingerprint,
            &report.grants_revision,
            "profile",
            serde_json::json!({"name": "Randall"}),
        )
        .unwrap();
        assert!(check_path(workspace.as_os_str(), None).is_ok());
    }

    #[test]
    fn gateway_fingerprint_uses_saved_inputs_and_imported_volume_grants() {
        let _lock = STATE_ENV_LOCK.lock().unwrap();
        let root = tempfile::tempdir().unwrap();
        let _state = StateHomeGuard::set(root.path());
        let (workspace, _) = volume_workspace(root.path(), "workspace");
        let directory = workspace.parent().unwrap();
        fs::write(
            &workspace,
            fs::read_to_string(&workspace).unwrap()
                + "[[inputs]]\nas = 'profile'\ntype = 'cwp://org.coworkerprotocol/2026-09/schemas/plaintext'\npurpose = 'Local profile'\n[[imports]]\npath = './gateway.toml'\nas = 'gateway'\n",
        ).unwrap();
        fs::write(directory.join("gateway.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/gateway'\nrole = { ref = '~@/behavior' }\nintegration = { ref = '~@/execution' }\naction_name = 'handle--request'\nconnection_ref = 'test-gateway'\n[[imports]]\npath = './role.toml'\nas = 'behavior'\n[[imports]]\npath = './integration.toml'\nas = 'execution'\n").unwrap();
        fs::write(directory.join("role.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/role'\n[[adapters]]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/sh'\n[[actions]]\nverb = 'handle'\nobject = 'request'\ntitle = 'Handle request'\ndescription = 'Test gateway'\n[actions.input_schema]\npath = './schema.json'\n[actions.output_schema]\npath = './schema.json'\n").unwrap();
        fs::write(directory.join("integration.toml"), "type = 'cwp://org.coworkerprotocol/2026-09/schemas/integration'\n[adapter]\nref = 'cwp://org.coworkerprotocol/2026-09/adapters/sh'\n").unwrap();
        fs::write(directory.join("schema.json"), r#"{"type":"object"}"#).unwrap();
        let report =
            cwr::v2026_09::local::connections::inspect_project_connections(&workspace).unwrap();
        cwr::v2026_09::local::connections::save_plaintext_connection(
            &workspace,
            &report.fingerprint,
            &report.grants_revision,
            "profile",
            serde_json::json!({"name":"Test user"}),
        )
        .unwrap();
        assert!(graph::resolve_graph_with_options(
            &workspace,
            &EmptyResourceResolver,
            &GraphLoadOptions::default()
        )
        .is_err());
        run_gateway_fingerprint_command(
            OsStr::new("test-gateway"),
            OsStr::new("cwp://example.test/runtime"),
            directory.as_os_str(),
            Some(workspace.as_os_str()),
            false,
        )
        .unwrap();
    }
}
