#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DynamicTool {
    pub namespace: String,
    pub name: String,
    pub operation: String,
    pub description: String,
    pub input_schema: Value,
}
#[derive(Clone, Copy)]
pub(crate) enum ProviderAccess {
    Normal,
    Isolated,
    CwrOnly,
}
pub(crate) struct BoundedActionConfig {
    pub client_name: String,
    pub client_title: String,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub service_tier_file: Option<PathBuf>,
    pub workspace: PathBuf,
    pub developer_instructions: String,
    // Used only to recognize version 1 private-history compatibility digests.
    pub legacy_context_window_tokens: usize,
    pub observation: Value,
    pub return_schema: Value,
    pub host_tools: Vec<DynamicTool>,
    pub model_run_ref: Option<String>,
    pub continuation: Option<Value>,
    pub provider_store: Option<&'static str>,
    pub ephemeral: bool,
    pub native_text_result: bool,
    pub provider_access: ProviderAccess,
    pub sandbox: &'static str,
}
pub(crate) struct BoundedActionResult {
    pub returned: Value,
    pub continuation: Value,
}
pub(crate) struct ToolCallResult {
    success: bool,
    content: Value,
}
impl ToolCallResult {
    pub fn success(content: Value) -> Self {
        Self {
            success: true,
            content,
        }
    }
    pub fn failure(content: Value) -> Self {
        Self {
            success: false,
            content,
        }
    }
}
pub(crate) trait BoundedActionHandler {
    fn on_timing(&mut self, _timing: crate::adapter::AdapterTiming) {}
    fn is_stopped(&self) -> bool {
        false
    }
    fn claim_terminal(&mut self) -> bool {
        true
    }
    fn on_progress(&mut self, status: u16, summary: &str) -> CwrResult<()>;
    fn on_public_progress(&mut self, status: u16, summary: &str) -> CwrResult<()> {
        self.on_progress(status, summary)
    }
    fn on_activity(&mut self, activity: AdapterActivityEvent) -> CwrResult<()>;
    fn on_model_output(&mut self, output: AdapterModelOutput) -> CwrResult<()>;
    fn on_host_call(
        &mut self,
        call_id: &str,
        operation: &str,
        input: Value,
    ) -> CwrResult<ToolCallResult>;
}
