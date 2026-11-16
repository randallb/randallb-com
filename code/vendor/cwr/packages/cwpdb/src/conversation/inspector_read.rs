//! Finite public tool-inspector reads. SQL and expression selection stay private.
use super::*;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InspectorCollection {
    Activities,
    Calls,
    Waits,
    Files,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InspectorKey {
    pub scope: String,
    pub position: i64,
    pub reference: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectorReadRequest {
    pub invocation_ref: String,
    pub conversation_ref: String,
    pub actor_ref: String,
    pub collection: InspectorCollection,
    pub activity_ref: Option<String>,
    pub details: bool,
    pub after: Option<InspectorKey>,
    pub before: Option<InspectorKey>,
    pub size: usize,
    pub backwards: bool,
    pub remaining_bytes: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectorEntryFacts {
    pub key: InspectorKey,
    pub payload: Value,
    pub serialized_bytes: usize,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InspectorTextField {
    Command,
    Output,
    Error,
    Diff,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectorTextRequest {
    pub invocation_ref: String,
    pub conversation_ref: String,
    pub actor_ref: String,
    pub activity_ref: String,
    pub file: Option<InspectorKey>,
    pub field: InspectorTextField,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InspectorTextFacts {
    pub bytes: Vec<u8>,
    pub full_bytes: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvocationStopFacts {
    pub input: Option<String>,
    pub result: Option<String>,
    pub resolved: Option<String>,
    pub state: String,
}
