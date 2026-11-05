/// Metadata-only execution signals. Provider reasoning, item IDs, and payloads
/// never belong in this presentation contract.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionPhase {
    Queued,
    Working,
    Thinking,
    RestoringContext,
    CompactingContext,
    UsingTool,
    RunningCommand,
    HandingOff,
    Waiting,
    Resuming,
    Responding,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionPhaseState {
    Started,
    Updated,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExecutionActivity {
    pub phase: ExecutionPhase,
    pub state: ExecutionPhaseState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_parts: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_parts: Option<u64>,
}

impl ExecutionActivity {
    pub fn started(phase: ExecutionPhase) -> Self {
        Self {
            phase,
            state: ExecutionPhaseState::Started,
            completed_parts: None,
            total_parts: None,
        }
    }
}
