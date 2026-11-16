#[derive(Debug, Serialize)]
pub struct CheckedGraph {
    #[serde(rename = "type")]
    type_ref: String,
    #[serde(rename = "ref")]
    reference: String,
    resources: Vec<CheckedResource>,
    actors: Vec<CheckedActor>,
    actor_integrations: Vec<CheckedActorIntegration>,
    routes: Vec<CheckedActionRoute>,
    gateways: Vec<CheckedGateway>,
}

#[derive(Debug, Serialize)]
struct CheckedResource {
    path: String,
    #[serde(rename = "type")]
    type_ref: String,
    #[serde(rename = "ref")]
    reference: String,
    visibility: ResolvedCwrVisibility,
}

#[derive(Debug, Serialize)]
struct CheckedActor {
    actor_ref: String,
    coworker_ref: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    default_conversation_target_ref: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    authority: Vec<String>,
}

#[derive(Debug, Serialize)]
struct CheckedActorIntegration {
    actor_ref: String,
    coworker_ref: String,
    actor_integration_ref: String,
    integration_ref: String,
    adapter_ref: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    available_authority: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    missing_required_authority: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum Visibility {
    Import,
    Export,
}

impl From<Visibility> for ResolvedCwrVisibility {
    fn from(visibility: Visibility) -> Self {
        match visibility {
            Visibility::Import => Self::Import,
            Visibility::Export => Self::Export,
        }
    }
}

#[derive(Debug, Serialize)]
struct CheckedActionRoute {
    actor_ref: String,
    coworker_ref: String,
    action_name: String,
    action_title: String,
    action_description: String,
    actor_integration_ref: String,
    integration_ref: String,
    adapter_ref: String,
}

