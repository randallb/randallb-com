use super::*;
impl NativeReadSnapshot {
    pub(super) fn select_inspector(
        &self,
        operation: &ReadSelectionOperation,
    ) -> CwpdbResult<Value> {
        Ok(match operation {
            ReadSelectionOperation::InspectorCaptureAvailable { invocation_ref } => {
                serde_json::to_value(self.inspector_capture_available(invocation_ref)?)?
            }
            ReadSelectionOperation::InspectorDetailsAllowed {
                invocation_ref,
                actor_ref,
            } => serde_json::to_value(self.inspector_details_allowed(invocation_ref, actor_ref)?)?,
            ReadSelectionOperation::InspectorInvocationRoot { invocation_ref } => {
                serde_json::to_value(self.inspector_invocation_root(invocation_ref)?)?
            }
            ReadSelectionOperation::InspectorScope {
                actor_ref,
                invocation_ref,
                collection,
                activity_ref,
            } => serde_json::to_value(self.inspector_scope(
                actor_ref,
                invocation_ref,
                *collection,
                activity_ref.as_deref(),
            ))?,
            ReadSelectionOperation::InspectorPage { request } => {
                serde_json::to_value(self.inspector_page(request)?)?
            }
            ReadSelectionOperation::InspectorActivitySequence {
                invocation_ref,
                conversation_ref,
                activity_ref,
            } => serde_json::to_value(self.inspector_activity_sequence(
                invocation_ref,
                conversation_ref,
                activity_ref,
            )?)?,
            ReadSelectionOperation::InspectorExitCode {
                invocation_ref,
                conversation_ref,
                actor_ref,
                activity_ref,
            } => serde_json::to_value(self.inspector_exit_code(
                invocation_ref,
                conversation_ref,
                actor_ref,
                activity_ref,
            )?)?,
            ReadSelectionOperation::InspectorText { request } => {
                serde_json::to_value(self.inspector_text(request)?)?
            }
            _ => return Err("invalid finite selection dispatch".into()),
        })
    }
}
