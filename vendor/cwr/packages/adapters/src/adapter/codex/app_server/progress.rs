#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandProgressKind {
    SourceControl,
    Search,
    Test,
    Build,
    PrepareWorkspace,
    InspectWorkspace,
    Command,
}

impl CommandProgressKind {
    fn started_summary(self) -> &'static str {
        match self {
            Self::SourceControl => "Checking source-control changes.",
            Self::Search => "Searching the workspace.",
            Self::Test => "Running tests.",
            Self::Build => "Building the project.",
            Self::PrepareWorkspace => "Preparing an isolated workspace.",
            Self::InspectWorkspace => "Inspecting the workspace.",
            Self::Command => "Running a command.",
        }
    }

    fn failed_summary(self) -> &'static str {
        match self {
            Self::SourceControl => "Source-control check failed; continuing.",
            Self::Search => "Workspace search failed; continuing.",
            Self::Test => "Test command failed; continuing.",
            Self::Build => "Build command failed; continuing.",
            Self::PrepareWorkspace => "Workspace preparation failed; continuing.",
            Self::InspectWorkspace => "Workspace inspection failed; continuing.",
            Self::Command => "Command failed; continuing.",
        }
    }
}

// Command status belongs to the command activity, not a second numeric response.
fn command_progress_summary(message: &Value) -> Option<&'static str> {
    let method = message.get("method")?.as_str()?;
    let item = message.pointer("/params/item")?;
    if item.get("type").and_then(Value::as_str) != Some("commandExecution") {
        return None;
    }
    let kind = classify_command(item);
    match method {
        "item/started" => Some(kind.started_summary()),
        "item/completed" if command_failed(item) => Some(kind.failed_summary()),
        _ => None,
    }
}

fn classify_command(item: &Value) -> CommandProgressKind {
    let command = item
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if [
        "git status",
        "git diff",
        "git log",
        "sl status",
        "sl diff",
        "sl log",
    ]
    .iter()
    .any(|needle| command.contains(needle))
    {
        return CommandProgressKind::SourceControl;
    }
    if [
        "cargo test",
        "deno test",
        "npm test",
        "npm run test",
        "pnpm test",
        "yarn test",
        "pytest",
        "go test",
    ]
    .iter()
    .any(|needle| command.contains(needle))
    {
        return CommandProgressKind::Test;
    }
    if [
        "cargo build",
        "deno task build",
        "npm run build",
        "pnpm build",
        "yarn build",
        "go build",
    ]
    .iter()
    .any(|needle| command.contains(needle))
    {
        return CommandProgressKind::Build;
    }
    if ["mktemp", "rsync", "cp -r", "cp -a"]
        .iter()
        .any(|needle| command.contains(needle))
    {
        return CommandProgressKind::PrepareWorkspace;
    }
    if ["rg ", "grep ", "find "]
        .iter()
        .any(|needle| command.contains(needle))
    {
        return CommandProgressKind::Search;
    }
    let mut action_types = item
        .get("commandActions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|action| action.get("type").and_then(Value::as_str));
    if action_types
        .clone()
        .any(|action_type| action_type == "search")
    {
        return CommandProgressKind::Search;
    }
    if action_types.any(|action_type| matches!(action_type, "read" | "listFiles"))
        || ["pwd", "ls ", "sed ", "cat "]
            .iter()
            .any(|needle| command.contains(needle))
    {
        return CommandProgressKind::InspectWorkspace;
    }
    CommandProgressKind::Command
}

fn command_failed(item: &Value) -> bool {
    item.get("exitCode")
        .and_then(Value::as_i64)
        .is_some_and(|exit_code| exit_code != 0)
        || matches!(
            item.get("status").and_then(Value::as_str),
            Some("failed" | "error")
        )
}

fn provider_activity(
    message: &Value,
    scope: Option<&CodexOpenResponsesMapper>,
) -> Option<AdapterActivityEvent> {
    let method = message.get("method")?.as_str()?;
    let data = match method {
        "item/tool/call" => json!({
            "call_id": message.pointer("/params/callId"),
            "namespace": message.pointer("/params/namespace"),
            "tool": message.pointer("/params/tool"),
            "arguments": message.pointer("/params/arguments"),
        }),
        "item/started" | "item/completed" => {
            let item = message.pointer("/params/item")?;
            if item.get("type").and_then(Value::as_str) == Some("reasoning") {
                return None;
            }
            let mut item = sanitize_provider_value(item);
            if let Some(summary) = command_progress_summary(message) {
                item["progress_summary"] = json!(summary);
            }
            json!({"item": item})
        }
        "turn/started" | "turn/completed" => json!({
            "thread_id": message.pointer("/params/threadId"),
            "turn": sanitize_provider_value(message.pointer("/params/turn").unwrap_or(&Value::Null)),
            "scope": if turn_notification_matches_scope(message, scope) {
                "active"
            } else {
                "nested_or_unscoped"
            },
        }),
        _ => return None,
    };
    Some(AdapterActivityEvent::ProviderEvent {
        provider: "codex".into(),
        method: method.into(),
        data,
    })
}

fn execution_activity(message: &Value, scope: Option<&CodexOpenResponsesMapper>) -> Option<AdapterActivityEvent> {
    let scope = scope?;
    let thread = message.pointer("/params/threadId").and_then(Value::as_str);
    let turn = message.pointer("/params/turnId").and_then(Value::as_str);
    if thread.is_some_and(|id| id != scope.thread_id()) || turn.is_some_and(|id| id != scope.turn_id()) { return None; }
    use crate::adapter::{ExecutionActivity, ExecutionPhase, ExecutionPhaseState};
    let method = message.get("method")?.as_str()?;
    let state = match method {
        "item/started" => ExecutionPhaseState::Started,
        "item/completed" => ExecutionPhaseState::Completed,
        _ => return None,
    };
    let phase = match message.pointer("/params/item/type")?.as_str()? {
        "reasoning" => ExecutionPhase::Thinking,
        "contextCompaction" => ExecutionPhase::CompactingContext,
        "commandExecution" => ExecutionPhase::RunningCommand,
        "mcpToolCall" | "dynamicToolCall" | "webSearch" | "fileChange" => ExecutionPhase::UsingTool,
        "agentMessage" => ExecutionPhase::Responding,
        _ => return None,
    };
    Some(AdapterActivityEvent::ExecutionActivity { activity: ExecutionActivity {
        phase, state, completed_parts: None, total_parts: None,
    }})
}
fn provider_call_id(message: &Value) -> CwrResult<&str> {
    message
        .pointer("/params/callId")
        .and_then(Value::as_str)
        .filter(|call_id| !call_id.trim().is_empty())
        .ok_or_else(|| invalid_error("Codex dynamic tool call has no callId"))
}
fn sanitize_provider_value(value: &Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .filter(|(key, _)| {
                    let key = key.to_ascii_lowercase();
                    !key.contains("encrypted") && key != "reasoning"
                })
                .map(|(key, value)| (key.clone(), sanitize_provider_value(value)))
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(sanitize_provider_value).collect()),
        Value::String(value) if value.len() > 32 * 1024 => {
            let mut end = 32 * 1024;
            while !value.is_char_boundary(end) {
                end -= 1;
            }
            Value::String(format!("{}\n… [truncated by CWR]", &value[..end]))
        }
        _ => value.clone(),
    }
}
