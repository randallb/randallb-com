//! Explicit host-local configuration for experimental person delivery.
use super::*;
use sha2::{Digest, Sha256};

const PERSON: &str = "cwp://org.coworkerprotocol/2026-09/adapters/person";
const SETTING_KEY: &str = "desktop.human_reply_inbox";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub path: PathBuf,
    pub actor_ref: String,
    pub enabled: Option<bool>,
    pub revision: Option<String>,
}

#[derive(Serialize, Deserialize, schemars::JsonSchema)]
pub struct Report {
    pub key: String,
    pub actor_ref: String,
    pub person_name: String,
    pub title: String,
    pub description: String,
    pub enabled: bool,
    pub revision: String,
}

pub fn manage(request: Request) -> CwrResult<Report> {
    let setting = crate::configuration::boolean_setting(SETTING_KEY)?;
    let parameter = setting.workspace_parameter.as_str();
    let path = canonical_cwr_path(&request.path)?;
    let graph = connections::resolve_local_graph(&path)?;
    let actor = graph
        .actors
        .iter()
        .find(|a| a.actor_ref == request.actor_ref)
        .ok_or("The selected person is no longer in this workspace.")?;
    let bindings: Vec<_> = graph
        .actor_integrations
        .iter()
        .filter(|b| b.actor_ref == actor.actor_ref)
        .collect();
    if bindings.iter().any(|b| {
        b.adapter_ref != PERSON
            || !b.is_available()
            || b.message_mode != crate::v2026_09::graph::ActorIntegrationMessageMode::Conversational
    }) || bindings.len() > 1
    {
        return Err(
            "This person has custom delivery bindings. Configure the inbox in the workspace files."
                .into(),
        );
    }
    let binding = bindings.first();
    if let Some(binding) = binding {
        if graph.actor_integrations.iter().any(|b| {
            b.integration_source == binding.integration_source && b.actor_ref != actor.actor_ref
        }) {
            return Err("This inbox configuration is shared by several people. Give this person a separate integration first.".into());
        }
    }
    let enabled = configured_binding_enabled(
        binding.map(|b| b.integration_params.get(parameter).and_then(Value::as_bool)),
        setting.default,
    );
    // Include every declaration: stale Settings must not overwrite external edits.
    let mut sources: BTreeMap<PathBuf, String> = BTreeMap::new();
    for resource in &graph.resources {
        sources.insert(
            resource.source.clone(),
            fs::read_to_string(&resource.source)?,
        );
    }
    let revision = hex::encode(Sha256::digest(serde_json::to_vec(&sources)?));
    let report = Report {
        key: SETTING_KEY.to_owned(),
        actor_ref: actor.actor_ref.clone(),
        person_name: actor
            .actor_name
            .clone()
            .or(actor.coworker_name.clone())
            .unwrap_or(actor.coworker_slug.clone()),
        title: setting.title,
        description: setting.description,
        enabled,
        revision,
    };
    let Some(desired) = request.enabled else {
        return Ok(report);
    };
    if request.revision.as_deref() != Some(&report.revision) {
        return Err(
            "Workspace configuration changed. Reopen Experimental features and try again.".into(),
        );
    }
    if enabled == desired {
        return Ok(report);
    }
    let root = path.parent().ok_or("Workspace has no directory")?;
    let mut writes: BTreeMap<PathBuf, String> = BTreeMap::new();
    if let Some(binding) = binding {
        let mut document: toml_edit::Document = sources
            .get(&binding.integration_source)
            .ok_or("Missing integration source")?
            .parse()?;
        document["params"][parameter] = toml_edit::value(desired);
        writes.insert(binding.integration_source.clone(), document.to_string());
    } else {
        if actor.actor_source != actor.coworker_source {
            return Err("This person uses a separate actor declaration. Configure its person binding in the workspace files.".into());
        }
        let directory = actor
            .coworker_source
            .parent()
            .ok_or("Missing person directory")?;
        let integration = directory.join("inbox-integration.toml");
        if integration.exists() {
            return Err(
                "Inbox configuration files already exist. Review them before enabling.".into(),
            );
        }
        for role_ref in &actor.role_refs {
            let role = graph
                .resources
                .iter()
                .find(|r| &r.reference == role_ref)
                .ok_or("Missing person role")?;
            let mut document: toml_edit::Document = sources
                .get(&role.source)
                .ok_or("Missing role source")?
                .parse()?;
            let has_person = document
                .get("adapters")
                .and_then(toml_edit::Item::as_array_of_tables)
                .is_some_and(|adapters| {
                    adapters.iter().any(|adapter| {
                        adapter.get("ref").and_then(toml_edit::Item::as_str) == Some(PERSON)
                    })
                });
            if has_person {
                continue;
            }
            if graph
                .actors
                .iter()
                .any(|a| a.actor_ref != actor.actor_ref && a.role_refs.contains(role_ref))
            {
                return Err("This person's role is shared. Configure its person adapter in the workspace files.".into());
            }
            if document.get("adapters").is_none() {
                document["adapters"] = toml_edit::Item::ArrayOfTables(Default::default());
            }
            let mut adapter = toml_edit::Table::new();
            adapter["ref"] = toml_edit::value(PERSON);
            document["adapters"]
                .as_array_of_tables_mut()
                .ok_or("Custom role adapter format; configure this inbox in the workspace files.")?
                .push(adapter);
            writes.insert(role.source.clone(), document.to_string());
        }
        writes.insert(integration, format!("type = \"cwp://org.coworkerprotocol/2026-09/schemas/integration\"\n[adapter]\nref = {PERSON:?}\n[params]\n{parameter} = true\n"));
        let original = sources
            .get(&actor.coworker_source)
            .ok_or("Missing person declaration")?;
        writes.insert(actor.coworker_source.clone(), format!("{original}\n[[integrations]]\npath = \"./inbox-integration.toml\"\nas = \"human_reply_inbox\"\ndefault = true\n"));
    }
    for destination in writes.keys() {
        if !destination.starts_with(root)
            || destination
                .parent()
                .ok_or("Missing directory")?
                .canonicalize()?
                .strip_prefix(root)
                .is_err()
        {
            return Err("Inbox configuration must be inside the open workspace.".into());
        }
        if destination.exists() && fs::symlink_metadata(destination)?.file_type().is_symlink() {
            return Err("Cannot edit a linked inbox configuration.".into());
        }
    }
    // Recheck before publishing; restore only our own writes if validation fails.
    for (source, text) in &sources {
        if fs::read_to_string(source)? != *text {
            return Err("Workspace configuration changed. Try again.".into());
        }
    }
    let mut published = Vec::new();
    let result = (|| -> CwrResult<Report> {
        // Dependencies precede the coworker declaration.
        let mut ordered: Vec<_> = writes.iter().collect();
        ordered.sort_by_key(|(p, _)| *p == &actor.coworker_source);
        for (destination, content) in ordered {
            atomic_write(destination, content)?;
            published.push(destination.clone());
        }
        manage(Request {
            path: path.clone(),
            actor_ref: request.actor_ref.clone(),
            enabled: None,
            revision: None,
        })
    })();
    if result.is_err() {
        for destination in published.iter().rev() {
            if fs::read_to_string(destination).ok().as_ref() != writes.get(destination) {
                continue;
            }
            if let Some(original) = sources.get(destination) {
                atomic_write(destination, original)?;
            } else {
                fs::remove_file(destination)?;
            }
        }
    }
    result
}

fn atomic_write(path: &Path, content: &str) -> CwrResult<()> {
    use std::io::Write;
    let temporary = path.with_extension(format!("{}.tmp", Uuid::new_v4()));
    let result = (|| -> CwrResult<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        if let Ok(metadata) = fs::metadata(path) {
            file.set_permissions(metadata.permissions())?;
        }
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    let _ = fs::remove_file(temporary);
    result
}

fn configured_binding_enabled(value: Option<Option<bool>>, default: bool) -> bool {
    value.map(|value| value.unwrap_or(default)).unwrap_or(false)
}

impl LocalRuntime {
    pub(super) fn apply_experimental_features(&self, report: &Report) -> CwrResult<()> {
        let setting = crate::configuration::boolean_setting(SETTING_KEY)?;
        self.reload_graph_for_connected_actor_validated(&report.actor_ref, |graph, _| {
            let enabled = configured_binding_enabled(
                graph
                .actor_integrations
                .iter()
                .find(|binding| {
                    binding.actor_ref == report.actor_ref
                        && binding.adapter_ref == PERSON
                        && binding.is_available()
                        && binding.message_mode
                            == crate::v2026_09::graph::ActorIntegrationMessageMode::Conversational
                })
                .map(|binding| {
                    binding
                        .integration_params
                        .get(&setting.workspace_parameter)
                        .and_then(Value::as_bool)
                }),
                setting.default,
            );
            if enabled != report.enabled {
                return invalid("Workspace configuration changed before the inbox setting could be applied. Try again.");
            }
            Ok(())
        })?;
        Ok(())
    }

    pub(super) fn execute_experimental_features(
        &self,
        record: &ActionInvocationRecord,
        invocation: &AdapterInvocation,
    ) -> CwrResult<AdapterTerminal> {
        let path = {
            let state = self.lock()?;
            if record.parent_invocation_ref.is_some() {
                require_reconfiguration_authority(&state, &record.requester_actor_ref)?;
            }
            if !state.graph_options.source_roots.is_empty() {
                return Err(
                    "Experimental features are not available for composed workspaces yet.".into(),
                );
            }
            state.cwr_toml.clone()
        };
        let enabled = if invocation.action_name == "set--experimental_features" {
            Some(
                invocation
                    .input
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .ok_or("Missing inbox setting")?,
            )
        } else {
            None
        };
        let report = manage(Request {
            path,
            actor_ref: record.requester_actor_ref.clone(),
            enabled,
            revision: invocation
                .input
                .get("revision")
                .and_then(Value::as_str)
                .map(str::to_owned),
        })?;
        self.apply_experimental_features(&report)
            .map_err(|error| invalid_error(format!("The inbox setting is saved, but could not be applied. Reopen the workspace and try again: {error}")))?;
        Ok(AdapterTerminal {
            invocation_ref: record.invocation_ref.clone(),
            sequence: 1,
            status: 200,
            summary: format!(
                "Human reply inbox is {} for {}.",
                if report.enabled { "on" } else { "off" },
                report.person_name
            ),
            result: Some(serde_json::to_value(report)?),
            error: None,
            continuation: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::configured_binding_enabled;

    #[test]
    fn an_actor_without_a_binding_stays_disabled() {
        assert!(!configured_binding_enabled(None, true));
        assert!(configured_binding_enabled(Some(None), true));
        assert!(!configured_binding_enabled(Some(Some(false)), true));
    }
}
