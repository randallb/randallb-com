use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ResolvedCwrActor {
    pub actor_ref: String,
    pub actor_source: PathBuf,
    pub role_refs: Vec<String>,
    pub native_tools: Vec<ResolvedNativeTool>,
    pub default_actor_integration_ref: Option<String>,
    pub message_binding_preference: Vec<String>,
    pub default_conversation_target_ref: Option<String>,
    pub actor_name: Option<String>,
    pub authority: Vec<String>,
    pub coworker_ref: String,
    pub coworker_source: PathBuf,
    pub coworker_name: Option<String>,
    pub coworker_slug: String,
    pub category: Option<CoworkerCategory>,
    pub provider_model: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CoworkerCategory {
    Human,
    Ai,
    Software,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ResolvedNativeTool {
    pub coworker_ref: String,
    pub action_name: String,
    pub binding_ref: Option<String>,
}
