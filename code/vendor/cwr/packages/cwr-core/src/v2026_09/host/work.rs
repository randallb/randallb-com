/// Adds public updates from direct execution conversations to their owning chat.
/// The saved execution remains unchanged; provider metadata never crosses this boundary.
pub fn project_conversation_with_work(
    conversation: &RetainedConversation,
    conversations: &[RetainedConversation],
) -> RetainedConversation {
    project_conversation_with_work_relations(conversation, conversations.iter(), &BTreeMap::new())
}

include!("work/authoritative.rs");

#[derive(Debug, Clone, Default)]
pub(crate) struct WorkInvocationRelation {
    pub(crate) parent_invocation_ref: Option<String>,
    pub(crate) wait: Option<crate::v2026_09::conversation::WorkInvocationWait>,
}

pub(crate) fn project_conversation_with_work_relations<'a>(
    conversation: &RetainedConversation,
    conversations: impl Iterator<Item = &'a RetainedConversation>,
    relations: &BTreeMap<String, WorkInvocationRelation>,
) -> RetainedConversation {
    let mut projected = project_conversation(conversation);
    projected.activity.extend(project_execution_work(
        conversation,
        conversations,
        relations,
    ));
    sort_work_activities(&mut projected.activity);
    projected
}

fn project_execution_work<'a>(
    conversation: &RetainedConversation,
    conversations: impl Iterator<Item = &'a RetainedConversation>,
    relations: &BTreeMap<String, WorkInvocationRelation>,
) -> Vec<ConversationActivity> {
    let mut projected = Vec::new();
    let executions = conversations
        .filter(|candidate| {
            candidate.parent_conversation_ref.as_deref() == Some(&conversation.conversation_ref)
        })
        .collect::<Vec<_>>();
    let parent_invocation_refs = execution_parent_invocation_refs(&executions);
    for execution in executions {
        let [actor_ref] = execution.participant_actor_refs.as_slice() else {
            continue;
        };
        let Some((invocation_ref, source_message_ref)) =
            execution
                .activity
                .iter()
                .find_map(|item| match &item.event {
                    ConversationActivityEvent::TurnStarted {
                        invocation_ref,
                        source_message_ref,
                        ..
                    } => Some((invocation_ref, source_message_ref)),
                    _ => None,
                })
        else {
            continue;
        };
        // Nested execution details belong to their own conversation.
        if !conversation
            .messages
            .iter()
            .any(|message| &message.message_ref == source_message_ref)
        {
            continue;
        }
        let mut tools = BTreeMap::new();
        let mut action_calls = BTreeMap::new();
        let mut coworker_calls = BTreeSet::new();
        let represented_calls = represented_host_calls(execution);
        let latest_keyed_progress = latest_keyed_progress(execution);
        let mut last_progress = None;
        for item in &execution.activity {
            if matches!(&item.event, ConversationActivityEvent::ExecutionActivity { activity }
                if activity.phase == crate::adapter::ExecutionPhase::Resuming) {
                tools.clear();
                action_calls.clear();
            }
            let phase = match &item.event {
                ConversationActivityEvent::HostCall {
                    call_id, operation, ..
                } if is_coworker_call(operation) => {
                    coworker_calls.insert(call_id.clone());
                    Some(crate::adapter::ExecutionPhase::HandingOff)
                }
                ConversationActivityEvent::HostResult { call_id, .. }
                    if coworker_calls.remove(call_id) =>
                {
                    Some(crate::adapter::ExecutionPhase::Resuming)
                }
                _ => None,
            };
            if let Some(phase) = phase {
                let mut signal = item.clone();
                signal.activity_ref = format!("{}/phase", item.activity_ref);
                signal.event = ConversationActivityEvent::WorkPhase {
                    invocation_ref: invocation_ref.clone(),
                    actor_ref: actor_ref.clone(),
                    activity: crate::adapter::ExecutionActivity::started(phase),
                };
                projected.push(signal);
            }
            if let ConversationActivityEvent::ExecutionActivity { activity } = &item.event {
                let mut phase = item.clone();
                phase.event = ConversationActivityEvent::WorkPhase {
                    invocation_ref: invocation_ref.clone(),
                    actor_ref: actor_ref.clone(),
                    activity: activity.clone(),
                };
                projected.push(phase);
                continue;
            }

            if let ConversationActivityEvent::Progress { summary, .. } = &item.event {
                last_progress = Some(summary.as_str());
            }
            if let ConversationActivityEvent::Progress {
                progress: Some(progress),
                ..
            } = &item.event
            {
                if latest_keyed_progress.get(progress.key.as_str()).copied()
                    != Some(item.activity_ref.as_str())
                {
                    continue;
                }
            }
            let activity = match &item.event {
                ConversationActivityEvent::HostCall {
                    call_id,
                    operation,
                    input,
                } if is_coworker_call(operation) => {
                    let label = input
                        .get("activation")
                        .or_else(|| input.get("to"))
                        .and_then(serde_json::Value::as_str)
                        .map_or_else(
                            || "Calling a coworker".into(),
                            |activation| format!("Calling {activation}"),
                        );
                    action_calls.insert(
                        call_id.clone(),
                        WorkActivity {
                            activity_ref: item.activity_ref.clone(),
                            kind: "coworker".into(),
                            label,
                            state: "running".into(),
                            started_at_unix_ms: item.occurred_at_unix_ms,
                            child_invocation_ref: None,
                            duration_ms: None,
                            command: None,
                            output: None,
                            error: None,
                            exit_code: None,
                            files: Vec::new(),
                        },
                    );
                    None
                }
                ConversationActivityEvent::HostActionStarted {
                    call_id,
                    nested_invocation_ref,
                    ..
                } => action_calls.remove(call_id).map(|mut activity| {
                    // Keep the relationship for nested work presentation. The
                    // child supplies lifecycle and progress; host results must
                    // not create another parent activity for this call.
                    activity.child_invocation_ref = Some(nested_invocation_ref.clone());
                    activity
                }),
                ConversationActivityEvent::HostResult { call_id, error, .. } => error
                    .as_ref()
                    .and_then(|_| action_calls.remove(call_id))
                    .map(|mut activity| {
                        activity.state = "failed".into();
                        activity.duration_ms = Some(
                            item.occurred_at_unix_ms
                                .saturating_sub(activity.started_at_unix_ms),
                        );
                        activity.error = error.clone();
                        activity
                    }),
                _ if provider_tool_has_host_activity(item, &represented_calls) => None,
                _ => project_tool_activity(item, last_progress, &mut tools),
            };
            let (status, text) = match &item.event {
                ConversationActivityEvent::TurnStarted { .. } => (102, None),
                ConversationActivityEvent::CommentaryProgress { text, .. } => {
                    (102, Some(text.clone()))
                }
                ConversationActivityEvent::Progress {
                    status, summary, ..
                } => (status.as_u16(), Some(summary.clone())),
                ConversationActivityEvent::TurnCompleted {
                    status, summary, ..
                } => (status.as_u16(), Some(summary.clone())),
                ConversationActivityEvent::TurnFailed { status, error } => {
                    (status.as_u16(), Some(error.clone()))
                }
                ConversationActivityEvent::TurnStopped { status, reason } => {
                    (status.as_u16(), reason.clone())
                }
                _ if activity.is_some() => (102, None),
                _ => continue,
            };
            let mut update = item.clone();
            let relation = relations.get(invocation_ref);
            update.event = ConversationActivityEvent::WorkUpdate {
                invocation_ref: invocation_ref.clone(),
                source_message_ref: source_message_ref.clone(),
                parent_invocation_ref: relation
                    .and_then(|relation| relation.parent_invocation_ref.clone())
                    .or_else(|| parent_invocation_refs.get(invocation_ref).cloned()),
                wait: relation
                    .and_then(|relation| relation.wait.clone())
                    .map(Box::new),
                actor_ref: actor_ref.clone(),
                status: crate::v2026_09::invocation::ActionResponseStatus::new(status)
                    .expect("valid work status"),
                text,
                text_kind: work_update_text_kind(&item.event),
                activity: activity.map(Box::new),
            };
            projected.push(update);
        }
    }
    sort_work_activities(&mut projected);
    projected
}

fn execution_parent_invocation_refs(
    executions: &[&RetainedConversation],
) -> BTreeMap<String, String> {
    let mut parent_invocation_refs = BTreeMap::new();
    for execution in executions {
        let Some(parent_invocation_ref) = execution.activity.iter().find_map(|item| {
            if let ConversationActivityEvent::TurnStarted { invocation_ref, .. } = &item.event {
                Some(invocation_ref)
            } else {
                None
            }
        }) else {
            continue;
        };
        for item in &execution.activity {
            if let ConversationActivityEvent::HostActionStarted {
                nested_invocation_ref,
                ..
            } = &item.event
            {
                parent_invocation_refs
                    .entry(nested_invocation_ref.clone())
                    .or_insert_with(|| parent_invocation_ref.clone());
            }
        }
    }
    parent_invocation_refs
}

include!("work/progress.rs");

include!("work/tools.rs");

#[cfg(test)]
include!("work/pre_admission_tests.rs");

#[cfg(test)]
mod work_tests {
    use super::*;
    use serde_json::json;

    fn provider_activity(
        sequence: u64,
        method: &str,
        item: serde_json::Value,
    ) -> ConversationActivity {
        serde_json::from_value(json!({
            "activity_ref": format!("cwp://example.test/activities/{sequence}"),
            "sequence": sequence, "occurred_at_unix_ms": sequence * 1000,
            "event": {"kind": "provider_event", "provider": "codex", "method": method,
                "data": {"item": item, "threadId": "private-thread"}}
        }))
        .unwrap()
    }

    #[test]
    fn command_activity_keeps_identity_and_result_without_provider_metadata() {
        let mut tools = BTreeMap::new();
        let start = provider_activity(
            1,
            "item/started",
            json!({
                "id": "private-command", "type": "commandExecution", "status": "inProgress",
                "command": "deno test", "arbitrary": "private-data"
            }),
        );
        let first = project_tool_activity(&start, Some("Running tests."), &mut tools).unwrap();
        let completed = provider_activity(
            4,
            "item/completed",
            json!({
                "id": "private-command", "type": "commandExecution", "status": "completed",
                "command": "deno test", "exitCode": 1, "aggregatedOutput": "One test failed.",
                "arbitrary": "private-data"
            }),
        );
        let last = project_tool_activity(&completed, None, &mut tools).unwrap();
        assert_eq!(last.activity_ref, first.activity_ref);
        assert_eq!(last.label, "Running tests");
        assert_eq!(last.state, "failed");
        assert_eq!(last.duration_ms, Some(3000));
        assert_eq!(last.output.as_deref(), Some("One test failed."));
        assert!(!serde_json::to_string(&last).unwrap().contains("private"));
    }

    #[test]
    fn failed_command_summary_updates_the_same_public_activity() {
        for retain_start in [true, false] {
            let mut tools = BTreeMap::new();
            let started = provider_activity(
                1,
                "item/started",
                json!({
                    "id": "command", "type": "commandExecution",
                    "progress_summary": "Running tests."
                }),
            );
            if retain_start {
                project_tool_activity(&started, None, &mut tools).unwrap();
            }
            let failed = provider_activity(
                2,
                "item/completed",
                json!({
                    "id": "command", "type": "commandExecution", "exitCode": 1,
                    "progress_summary": "Test command failed; continuing."
                }),
            );
            let activity = project_tool_activity(&failed, None, &mut tools).unwrap();
            assert_eq!(activity.label, "Test command failed; continuing");
            assert_eq!(activity.state, "failed");
            assert_eq!(activity.exit_code, Some(1));
            assert_eq!(
                activity.activity_ref,
                if retain_start {
                    &started.activity_ref
                } else {
                    &failed.activity_ref
                }
                .to_owned()
            );
            assert_eq!(tools.len(), 1);
        }
    }

    #[test]
    fn failed_tool_activity_keeps_the_error_without_arbitrary_provider_data() {
        let mut tools = BTreeMap::new();
        let failed = provider_activity(
            1,
            "item/completed",
            json!({
                "id": "private-tool", "type": "mcpToolCall", "status": "failed",
                "tool": "read_issue",
                "error": {"message": "Issue service returned 503.", "trace": "private"},
                "arguments": {"token": "private"}
            }),
        );

        let activity = project_tool_activity(&failed, None, &mut tools).unwrap();

        assert_eq!(activity.state, "failed");
        assert_eq!(activity.label, "Using read_issue");
        assert_eq!(
            activity.error.as_deref(),
            Some("Issue service returned 503.")
        );
        let wire = serde_json::to_string(&activity).unwrap();
        assert!(!wire.contains("trace"));
        assert!(!wire.contains("token"));
        assert!(!wire.contains("private"));
    }

    #[test]
    fn known_cwr_tools_have_human_readable_activity_labels() {
        for (item, expected) in [
            (
                json!({"namespace": "cwr", "tool": "start_action"}),
                "Starting a coworker task",
            ),
            (
                json!({"namespace": "cwr", "tool": "invoke_action"}),
                "Starting an action",
            ),
            (
                json!({"namespace": "cwr", "tool": "inspect_invocation"}),
                "Waiting for a coworker",
            ),
            (
                json!({"namespace": "cwr", "tool": "stop_invocation"}),
                "Stopping a coworker task",
            ),
            (
                json!({"namespace": "cwr", "tool": "send_message"}),
                "Sending a coworker message",
            ),
            (
                json!({"namespace": "cwr", "tool": "return_result", "arguments": {"status": 102}}),
                "Continuing the response",
            ),
            (
                json!({"namespace": "cwr", "tool": "return_result", "arguments": {"status": 200}}),
                "Finishing the response",
            ),
        ] {
            let (_, label) =
                tool_label("mcpToolCall", &item, None).expect("known tool call is projected");
            assert_eq!(label, expected);
        }
    }

    #[test]
    fn unrelated_tools_keep_their_provider_label() {
        let (_, label) = tool_label(
            "mcpToolCall",
            &json!({"namespace": "github", "tool": "return_result"}),
            None,
        )
        .expect("tool call is projected");
        assert_eq!(label, "Using return_result");
    }

    #[test]
    fn failed_dynamic_tool_activity_keeps_text_content_as_the_error() {
        let mut tools = BTreeMap::new();
        let failed = provider_activity(
            1,
            "item/completed",
            json!({
                "id": "private-tool", "type": "dynamicToolCall", "status": "failed",
                "tool": "review_issue",
                "contentItems": [{"type": "inputText", "text": "Review service rejected the request."}],
                "arguments": {"token": "private"}
            }),
        );

        let activity = project_tool_activity(&failed, None, &mut tools).unwrap();

        assert_eq!(activity.state, "failed");
        assert_eq!(
            activity.error.as_deref(),
            Some("Review service rejected the request.")
        );
        let wire = serde_json::to_string(&activity).unwrap();
        assert!(!wire.contains("token"));
        assert!(!wire.contains("private"));
    }

    #[test]
    fn file_activity_preserves_changes_but_ignores_unknown_and_reasoning_items() {
        let mut tools = BTreeMap::new();
        let change = provider_activity(
            1,
            "item/completed",
            json!({
                "id": "private-edit", "type": "fileChange", "status": "completed",
                "changes": [{"path": "actors/cos.toml", "kind": {"type": "add"},
                    "diff": "+name = 'Chief of Staff'", "secret": "private"}]
            }),
        );
        let result = project_tool_activity(&change, None, &mut tools).unwrap();
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].kind, "add");
        assert_eq!(
            result.files[0].diff.as_deref(),
            Some("+name = 'Chief of Staff'")
        );
        assert_eq!(result.label, "Changed 1 file");
        assert_eq!(result.started_at_unix_ms, 0);
        assert_eq!(result.duration_ms, None);
        assert!(!serde_json::to_string(&result).unwrap().contains("private"));
        for kind in ["reasoning", "unknown"] {
            let ignored = provider_activity(
                2,
                "item/completed",
                json!({
                    "id": "private", "type": kind, "text": "private"
                }),
            );
            assert!(project_tool_activity(&ignored, None, &mut tools).is_none());
        }
    }

    #[test]
    fn saved_work_updates_survive_projection_without_provider_details() {
        let root: RetainedConversation = serde_json::from_value(json!({
            "conversation_ref": "cwp://example.test/conversations/root",
            "participant_actor_refs": ["~@/actors/person", "~@/actors/helper"],
            "messages": [{
                "message_ref": "cwp://example.test/messages/request", "author_actor_ref": "~@/actors/person",
                "addressed_actor_refs": ["~@/actors/helper"], "text": "Help me"
            }]
        })).unwrap();
        let execution: RetainedConversation = serde_json::from_value(json!({
            "conversation_ref": "cwp://example.test/conversations/execution",
            "parent_conversation_ref": root.conversation_ref,
            "participant_actor_refs": ["~@/actors/helper"], "messages": [],
            "activity": [
                {"activity_ref":"cwp://example.test/activities/start", "sequence":1, "event":{
                    "kind":"turn_started", "invocation_ref":"cwp://example.test/invocations/one",
                    "source_message_ref":"cwp://example.test/messages/request", "invocation":{"secret":"private"}
                }},
                {"activity_ref":"cwp://example.test/activities/update", "sequence":2, "event":{
                    "kind":"commentary_progress", "model_run_ref":"cwp://example.test/model_runs/private",
                    "provider_integration_ref":"~@/integrations/private", "provider":"private",
                    "thread_id":"private", "response_or_turn_id":"private", "item_id":"private",
                    "text":"I found the actor configuration."
                }},
                {"activity_ref":"cwp://example.test/activities/command", "sequence":3, "event":{
                    "kind":"provider_event", "provider":"private", "method":"item/completed",
                    "data":{"item":{"id":"private", "type":"commandExecution", "command":"cwr check",
                        "exitCode":1, "aggregatedOutput":"Validation command failed."}}
                }},
                {"activity_ref":"cwp://example.test/activities/done", "sequence":4, "event":{
                    "kind":"turn_completed", "status":200, "summary":"Done", "result":{"secret":"private"}
                }}
            ]
        })).unwrap();
        // Round-trip the saved execution, as on reconnect/restart.
        let saved = serde_json::to_string(&execution).unwrap();
        let restored = serde_json::from_str(&saved).unwrap();
        let projected = project_conversation_with_work(&root, &[root.clone(), restored]);
        assert_eq!(projected.activity.len(), 4);
        assert!(matches!(&projected.activity[1].event,
            ConversationActivityEvent::WorkUpdate { text: Some(text), actor_ref, .. }
            if text == "I found the actor configuration." && actor_ref == "~@/actors/helper"));
        assert!(matches!(&projected.activity[2].event,
            ConversationActivityEvent::WorkUpdate { status, activity: Some(activity), .. }
            if status.as_u16() == 102 && activity.state == "failed"));
        let event = HostEvent::ConversationUpsert {
            source_ref: "cwp://example.test/sources/local".into(),
            conversation: projected.clone(),
        };
        event.validate().unwrap();
        let wire = serde_json::to_string(&event).unwrap();
        assert!(!wire.contains("private"));
        assert!(wire.contains("\"status\":200"));
        assert_eq!(root.activity.len(), 0);
        assert!(matches!(
            execution.activity[1].event,
            ConversationActivityEvent::CommentaryProgress { .. }
        ));
        // The same execution must never be shown in a different chat.
        let mut other = root.clone();
        other.conversation_ref = "cwp://example.test/conversations/other".into();
        assert!(project_conversation_with_work(&other, &[execution])
            .activity
            .is_empty());
    }

    include!("work/action_tests.rs");
    include!("work/message_tests.rs");
    include!("work/provider_calls_tests.rs");
}
