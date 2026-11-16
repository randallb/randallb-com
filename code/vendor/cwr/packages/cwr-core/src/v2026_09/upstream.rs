//! Connection-owned coworkers supplied by a host. Registration grants no host operations.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{refs::is_action_name, CwrRef, CwrResult};
mod command;
mod delegation;
pub use command::{InstalledScheduleActivation, UpstreamCommand};
pub use delegation::DelegationSource;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UpstreamAction {
    pub action_name: String,
    pub title: String,
    pub description: String,
    pub input_schema: Value,
    pub output_schema: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UpstreamCoworker {
    pub actor_ref: String,
    pub coworker_ref: String,
    pub handle: String,
    pub name: String,
    pub actions: Vec<UpstreamAction>,
}

/// Host execution for an exact configured gateway contract; never a conversational actor.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UpstreamGatewayHandler {
    pub resource_ref: String,
    pub action_name: String,
    pub input_schema: Value,
    pub output_schema: Value,
}

/// An admitted route names a particular connection and declaration. Re-enabling
/// or reconnecting the same actor must never change this selection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UpstreamRoute {
    pub provider_session_ref: String,
    /// Runtime-stamped provenance for a configured native gateway executor.
    /// Ordinary action payloads cannot select this execution classification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway_resource_ref: Option<String>,
    pub coworker: UpstreamCoworker,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UpstreamWork {
    pub invocation_ref: String,
    pub root_invocation_ref: String,
    pub conversation_ref: String,
    pub requester_actor_ref: String,
    pub route: UpstreamRoute,
    pub action_name: String,
    pub input: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addressed_message: Option<super::conversation::ConversationMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum UpstreamEvent {
    ValidateResult {
        invocation_ref: String,
        validation_ref: String,
    },
    WorkRequested {
        work: Box<UpstreamWork>,
    },
    StopRequested {
        invocation_ref: String,
    },
}

#[derive(Debug, Clone)]
struct EnabledCoworker {
    declaration: UpstreamCoworker,
    enabled: bool,
}

#[derive(Debug, Clone)]
struct Provider {
    owner: String,
    actors: BTreeMap<String, EnabledCoworker>,
    hidden: bool,
}

/// Ephemeral availability only. The caller retains accepted declarations and
/// selected routes before publishing registration or admitting work.
#[derive(Debug, Clone, Default)]
pub struct UpstreamRegistry {
    providers: BTreeMap<String, Provider>,
}

fn invalid<T>(message: impl Into<String>) -> CwrResult<T> {
    Err(std::io::Error::new(std::io::ErrorKind::InvalidData, message.into()).into())
}

impl UpstreamCoworker {
    pub fn validate(&self) -> CwrResult<()> {
        for reference in [&self.actor_ref, &self.coworker_ref] {
            let parsed = CwrRef::parse(reference)?;
            if reference.starts_with("~@") || parsed.fragment().is_some() {
                return invalid("upstream identities must be absolute and unfragmented");
            }
        }
        let Some(handle) = self.handle.strip_prefix('@') else {
            return invalid("upstream handle must begin with @");
        };
        if handle.is_empty()
            || !handle
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_')
            || handle.eq_ignore_ascii_case("cwr")
        {
            return invalid("upstream handle must be an unreserved actor handle");
        }
        if self.name.trim().is_empty() {
            return invalid("upstream coworker needs a name");
        }
        let mut names = BTreeSet::new();
        for action in &self.actions {
            if !is_action_name(&action.action_name) || !names.insert(&action.action_name) {
                return invalid("upstream action names must be unique verb--object names");
            }
            // Compile schemas on admission to the catalog, before callers can discover them.
            validate_schema(&action.input_schema)?;
            validate_schema(&action.output_schema)?;
        }
        Ok(())
    }
}

impl UpstreamRegistry {
    pub fn register(
        &mut self,
        runtime_ref: &str,
        owner: &str,
        coworkers: Vec<UpstreamCoworker>,
        known: &[(String, String)],
    ) -> CwrResult<String> {
        self.register_inner(runtime_ref, owner, coworkers, known, false)
    }

    pub(crate) fn register_gateway(
        &mut self,
        runtime_ref: &str,
        owner: &str,
        coworkers: Vec<UpstreamCoworker>,
    ) -> CwrResult<String> {
        self.register_inner(runtime_ref, owner, coworkers, &[], true)
    }

    fn register_inner(
        &mut self,
        runtime_ref: &str,
        owner: &str,
        coworkers: Vec<UpstreamCoworker>,
        known: &[(String, String)],
        hidden: bool,
    ) -> CwrResult<String> {
        CwrRef::parse(runtime_ref)?;
        if owner.trim().is_empty() {
            return invalid("provider connection owner must not be blank");
        }
        if coworkers.is_empty() {
            return invalid("provider must advertise at least one coworker");
        }
        let mut actors = BTreeMap::new();
        let mut handles = BTreeSet::new();
        for coworker in coworkers {
            coworker.validate()?;
            if known
                .iter()
                .any(|(actor, handle)| actor == &coworker.actor_ref || handle == &coworker.handle)
                || self
                    .providers
                    .values()
                    .any(|provider| !hidden && provider.actors.contains_key(&coworker.actor_ref))
                || actors.contains_key(&coworker.actor_ref)
                || !handles.insert(coworker.handle.clone())
            {
                return invalid(format!(
                    "upstream identity or handle collision for {}",
                    coworker.handle
                ));
            }
            actors.insert(
                coworker.actor_ref.clone(),
                EnabledCoworker {
                    declaration: coworker,
                    enabled: false,
                },
            );
        }
        let session = format!("{runtime_ref}/providers/{}", Uuid::new_v4().simple());
        self.providers.insert(
            session.clone(),
            Provider {
                owner: owner.into(),
                actors,
                hidden,
            },
        );
        Ok(session)
    }

    pub fn enable(
        &mut self,
        owner: &str,
        session: &str,
        actor: &str,
        enabled: bool,
        known: &[(String, String)],
    ) -> CwrResult<()> {
        let provider = self
            .providers
            .get(session)
            .ok_or_else(|| std::io::Error::other("unknown upstream provider"))?;
        if provider.hidden {
            return invalid("gateway handlers cannot be enabled as coworkers");
        }
        if provider.owner != owner {
            return invalid("only the registering connection may change availability");
        }
        let entry = provider
            .actors
            .get(actor)
            .ok_or_else(|| std::io::Error::other("actor was not advertised by this provider"))?;
        if enabled {
            if known.iter().any(|(known_actor, handle)| {
                known_actor != actor && handle == &entry.declaration.handle
            }) {
                return invalid(format!(
                    "{} already exists; choose another handle",
                    entry.declaration.handle
                ));
            }
            for (other_session, other) in &self.providers {
                for (other_actor, candidate) in &other.actors {
                    if other_session == session && other_actor == actor {
                        continue;
                    }
                    if candidate.enabled
                        && candidate.declaration.handle == entry.declaration.handle
                        && other.owner == owner
                    {
                        return invalid(format!(
                            "{} already exists in this scope; choose another handle",
                            entry.declaration.handle
                        ));
                    }
                }
            }
        }
        let entry = self
            .providers
            .get_mut(session)
            .unwrap()
            .actors
            .get_mut(actor)
            .unwrap();
        entry.enabled = enabled;
        Ok(())
    }

    pub fn visible(&self, owner: Option<&str>) -> CwrResult<Vec<UpstreamRoute>> {
        let mut handles = BTreeSet::new();
        let mut routes = Vec::new();
        for (session, provider) in &self.providers {
            for entry in provider.actors.values() {
                if !provider.hidden && entry.enabled && owner == Some(provider.owner.as_str()) {
                    if !handles.insert(&entry.declaration.handle) {
                        return invalid("connection has conflicting upstream handles");
                    }
                    routes.push(UpstreamRoute {
                        provider_session_ref: session.clone(),
                        gateway_resource_ref: None,
                        coworker: entry.declaration.clone(),
                    });
                }
            }
        }
        Ok(routes)
    }

    pub fn owns_route(&self, owner: &str, route: &UpstreamRoute) -> bool {
        self.providers
            .get(&route.provider_session_ref)
            .is_some_and(|provider| {
                provider.owner == owner
                    && provider.hidden == route.gateway_resource_ref.is_some()
                    && provider
                        .actors
                        .get(&route.coworker.actor_ref)
                        .is_some_and(|entry| entry.declaration == route.coworker)
            })
    }

    pub fn validate_graph(&self, known: &[(String, String)]) -> CwrResult<()> {
        for provider in self.providers.values() {
            if provider.hidden {
                continue;
            }
            for actor in provider.actors.values() {
                if known.iter().any(|(reference, handle)| {
                    reference == &actor.declaration.actor_ref || handle == &actor.declaration.handle
                }) {
                    return invalid("reloaded graph collides with a registered upstream coworker");
                }
            }
        }
        Ok(())
    }

    pub fn disconnect(&mut self, owner: &str) -> Vec<String> {
        let removed = self
            .providers
            .iter()
            .filter(|(_, provider)| provider.owner == owner)
            .map(|(session, _)| session.clone())
            .collect::<Vec<_>>();
        self.providers.retain(|_, provider| provider.owner != owner);
        removed
    }
}

fn validate_schema(schema: &Value) -> CwrResult<()> {
    jsonschema::options()
        .with_format("cwr-ref", |value: &str| CwrRef::parse(value).is_ok())
        .should_validate_formats(true)
        .build(schema)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn coworker(instance: &str) -> UpstreamCoworker {
        UpstreamCoworker {
            actor_ref: format!("cwp://example.test/tuis/{instance}/actors/tui"),
            coworker_ref: "cwp://example.test/2026-09/coworkers/tui".into(),
            handle: "@tui".into(),
            name: "TUI".into(),
            actions: vec![UpstreamAction {
                action_name: "new--conversation".into(),
                title: "New conversation".into(),
                description: "Start composing a new conversation".into(),
                input_schema: json!({"type":"object","additionalProperties":false}),
                output_schema: json!({"type":"object"}),
            }],
        }
    }

    #[test]
    fn advertisements_are_invisible_and_enabled_coworkers_are_connection_local() {
        let mut registry = UpstreamRegistry::default();
        let actor = coworker("one");
        let session = registry
            .register(
                "cwp://example.test/runtime",
                "connection-one",
                vec![actor.clone()],
                &[],
            )
            .unwrap();
        assert!(registry.visible(Some("connection-one")).unwrap().is_empty());
        registry
            .enable("connection-one", &session, &actor.actor_ref, true, &[])
            .unwrap();
        assert_eq!(registry.visible(Some("connection-one")).unwrap().len(), 1);
        assert_eq!(registry.visible(None).unwrap().len(), 0);
        assert!(registry.visible(Some("connection-two")).unwrap().is_empty());
        assert!(registry.visible(None).unwrap().is_empty());
    }

    #[test]
    fn unique_actors_may_reuse_a_handle_on_different_connections() {
        let mut registry = UpstreamRegistry::default();
        let first = coworker("one");
        let second = coworker("two");
        let one = registry
            .register(
                "cwp://example.test/runtime",
                "one",
                vec![first.clone()],
                &[],
            )
            .unwrap();
        assert!(registry
            .register(
                "cwp://example.test/runtime",
                "two",
                vec![first.clone()],
                &[]
            )
            .is_err());
        let two = registry
            .register(
                "cwp://example.test/runtime",
                "two",
                vec![second.clone()],
                &[],
            )
            .unwrap();
        registry
            .enable("one", &one, &first.actor_ref, true, &[])
            .unwrap();
        registry
            .enable("two", &two, &second.actor_ref, true, &[])
            .unwrap();
        assert_eq!(registry.visible(Some("one")).unwrap()[0].coworker, first);
        assert_eq!(registry.visible(Some("two")).unwrap()[0].coworker, second);
        assert!(registry.visible(None).unwrap().is_empty());
    }

    #[test]
    fn connection_ownership_and_reconnection_do_not_redirect_selected_routes() {
        let mut registry = UpstreamRegistry::default();
        let actor = coworker("one");
        let session = registry
            .register(
                "cwp://example.test/runtime",
                "one",
                vec![actor.clone()],
                &[],
            )
            .unwrap();
        assert!(registry
            .enable("other", &session, &actor.actor_ref, true, &[])
            .is_err());
        registry
            .enable("one", &session, &actor.actor_ref, true, &[])
            .unwrap();
        let selected = registry.visible(Some("one")).unwrap().remove(0);
        assert!(!registry.owns_route("other", &selected));
        assert_eq!(registry.disconnect("one"), vec![session.clone()]);
        assert!(!registry.owns_route("one", &selected));
        let new_session = registry
            .register("cwp://example.test/runtime", "one", vec![actor], &[])
            .unwrap();
        assert_ne!(new_session, session);
        assert!(!registry.owns_route("one", &selected));
    }

    #[test]
    fn rejects_workspace_collisions_and_invalid_declarations_before_registration() {
        let mut registry = UpstreamRegistry::default();
        let actor = coworker("one");
        assert!(registry
            .register(
                "cwp://example.test/runtime",
                "one",
                vec![actor.clone()],
                &[("~@/actors/tui".into(), "@tui".into())]
            )
            .is_err());
        let mut invalid = actor.clone();
        invalid.actions[0].input_schema = json!({"type":"not-a-type"});
        assert!(registry
            .register(
                "cwp://example.test/runtime",
                "one",
                vec![actor.clone(), invalid],
                &[]
            )
            .is_err());
        assert!(registry.providers.is_empty());
        invalid = actor.clone();
        invalid.handle = "@CWR".into();
        assert!(invalid.validate().is_err());
        invalid = actor.clone();
        invalid.actions[0].action_name = "1run--task".into();
        assert!(invalid.validate().is_err());
        invalid.actions[0].action_name = "draft-item--brief".into();
        assert!(invalid.validate().is_err());
        invalid = actor;
        invalid.actions.push(invalid.actions[0].clone());
        assert!(invalid.validate().is_err());
    }
}

/// A host-selected action endpoint for one caller; route authority still applies.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ContextActionPermission {
    pub caller_actor_ref: String,
    pub target_actor_ref: String,
    pub action_names: Vec<String>,
}

/// Opaque host contexts intersect existing route authority.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConversationContextPolicy {
    /// Additional caller-specific action endpoints, intersected with existing route
    /// authority and actor allowlists. A `respond_to--message` permission lets
    /// its caller message that coworker without making the coworker a public
    /// message target in the host context.
    /// A nonmember source caller is confined to its exact outgoing action names,
    /// even when that target is an ordinary context member. Its retained parent
    /// chain must remain authorized at admission, execution, and result delivery.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub action_permissions: Vec<ContextActionPermission>,
    /// Close new coworker admissions while the native host refreshes the graph.
    #[serde(default)]
    pub suspended: bool,
    pub default_context_ref: String,
    pub contexts: BTreeMap<String, Vec<String>>,
}
