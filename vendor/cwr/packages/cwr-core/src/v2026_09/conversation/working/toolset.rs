//! Lifecycle capabilities missing from a retained provider session require one
//! fresh private history; completed public work is never replayed.
use super::*;

pub(super) fn changed(previous: &AdapterInvocation, invocation: &AdapterInvocation) -> bool {
    // Lifecycle tools are installed after continuity is claimed. Upgrade settled
    // Codex sessions once, without attempting to resume their older tool schema.
    let file_tool_changed = invocation
        .retained_route
        .as_ref()
        .is_some_and(|route| route.adapter_ref() == crate::adapter::CODEX_ADAPTER_REF)
        && previous
            .host_operations
            .iter()
            .any(|tool| tool.name == "cwr.send_message")
        && !previous
            .host_operations
            .iter()
            .any(|tool| tool.name == "cwr.read_file");
    let creation_tool_changed = previous
        .host_operations
        .iter()
        .any(|tool| tool.name == "cwr.send_message")
        && !previous
            .host_operations
            .iter()
            .any(|tool| tool.name == "cwr.create_conversation");
    let contexts_tool_changed = previous
        .host_operations
        .iter()
        .any(|tool| tool.name == "cwr.send_message")
        && !previous
            .host_operations
            .iter()
            .any(|tool| tool.name == "cwr.list_conversation_contexts");
    file_tool_changed || creation_tool_changed || contexts_tool_changed
}
