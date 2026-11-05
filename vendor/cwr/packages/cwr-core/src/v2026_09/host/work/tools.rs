use crate::v2026_09::conversation::{ConversationActivity, WorkActivity, WorkFileChange};

fn is_coworker_call(operation: &str) -> bool {
    matches!(
        operation,
        crate::action_contract::START_ACTION_HOST_OPERATION
            | crate::action_contract::INVOKE_ACTION_HOST_OPERATION
            | "cwr.send_message"
    )
}

fn represented_host_calls(execution: &RetainedConversation) -> BTreeSet<&str> {
    let calls: BTreeSet<_> = execution
        .activity
        .iter()
        .filter_map(|item| match &item.event {
            ConversationActivityEvent::HostCall {
                call_id, operation, ..
            } if is_coworker_call(operation) => Some(call_id.as_str()),
            _ => None,
        })
        .collect();
    execution
        .activity
        .iter()
        .filter_map(|item| match &item.event {
            ConversationActivityEvent::HostActionStarted { call_id, .. }
            | ConversationActivityEvent::HostResult {
                call_id,
                error: Some(_),
                ..
            } if calls.contains(call_id.as_str()) => Some(call_id.as_str()),
            _ => None,
        })
        .collect()
}

fn provider_tool_has_host_activity(
    retained: &ConversationActivity,
    represented_calls: &BTreeSet<&str>,
) -> bool {
    let ConversationActivityEvent::ProviderEvent { method, data, .. } = &retained.event else {
        return false;
    };
    if !matches!(method.as_str(), "item/started" | "item/completed") {
        return false;
    }
    let Some(item) = data.get("item") else {
        return false;
    };
    if !matches!(
        item.get("type").and_then(serde_json::Value::as_str),
        Some("dynamicToolCall" | "mcpToolCall")
    ) {
        return false;
    }
    // Codex uses the same item id for the provider tool and its host call.
    // Match identity, not namespace: CWP discovery and unrelated tools remain visible.
    item.get("callId")
        .or_else(|| item.get("call_id"))
        .or_else(|| item.get("id"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|id| represented_calls.contains(id))
}

pub(crate) fn project_tool_activity(
    retained: &ConversationActivity,
    progress: Option<&str>,
    tools: &mut BTreeMap<String, WorkActivity>,
) -> Option<WorkActivity> {
    let ConversationActivityEvent::ProviderEvent { method, data, .. } = &retained.event else {
        return None;
    };
    if !matches!(method.as_str(), "item/started" | "item/completed") {
        return None;
    }
    let item = data.get("item")?;
    let id = item.get("id")?.as_str()?;
    let item_type = item.get("type")?.as_str()?;
    let (kind, label) = tool_label(item_type, item, progress)?;
    let started = method == "item/started";
    let state = match item.get("status").and_then(serde_json::Value::as_str) {
        Some("failed" | "error" | "declined") => "failed",
        _ if item
            .get("exitCode")
            .and_then(serde_json::Value::as_i64)
            .is_some_and(|code| code != 0)
            || item.get("success").and_then(serde_json::Value::as_bool) == Some(false) =>
        {
            "failed"
        }
        _ if started => "running",
        _ => "completed",
    };
    let mut activity = tools.get(id).cloned().unwrap_or_else(|| WorkActivity {
        activity_ref: retained.activity_ref.clone(),
        kind: kind.into(),
        label,
        state: state.into(),
        started_at_unix_ms: if started {
            retained.occurred_at_unix_ms
        } else {
            0
        },
        child_invocation_ref: None,
        duration_ms: None,
        command: None,
        output: None,
        error: None,
        exit_code: None,
        files: Vec::new(),
    });
    activity.state = state.into();
    activity.duration_ms = item
        .get("durationMs")
        .and_then(serde_json::Value::as_u64)
        .or_else(|| {
            (!started && activity.started_at_unix_ms > 0).then(|| {
                retained
                    .occurred_at_unix_ms
                    .saturating_sub(activity.started_at_unix_ms)
            })
        });
    activity.error = tool_error(item);
    // Allowlisted display fields only: never forward arbitrary arguments, results,
    // provider IDs, or reasoning records. Command output is shown only in details.
    if item_type == "commandExecution" {
        if let Some(summary) = item
            .get("progress_summary")
            .and_then(serde_json::Value::as_str)
        {
            activity.label = summary.trim_end_matches('.').into();
        }
        activity.command = item
            .get("command")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        activity.output = item
            .get("aggregatedOutput")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        activity.exit_code = item.get("exitCode").and_then(serde_json::Value::as_i64);
    }
    if item_type == "fileChange" {
        activity.files = item
            .get("changes")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|change| {
                Some(WorkFileChange {
                    path: change.get("path")?.as_str()?.into(),
                    kind: change
                        .pointer("/kind/type")
                        .or_else(|| change.get("kind"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("update")
                        .into(),
                    diff: change
                        .get("diff")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned),
                })
            })
            .collect();
        activity.label = format!(
            "{} {} file{}",
            if started { "Editing" } else { "Changed" },
            activity.files.len(),
            if activity.files.len() == 1 { "" } else { "s" }
        );
        if state == "failed" {
            activity.label = "File edit failed".into();
        }
    }
    tools.insert(id.into(), activity.clone());
    Some(activity)
}

fn tool_error(item: &serde_json::Value) -> Option<String> {
    item.get("error")
        .and_then(|error| {
            error
                .as_str()
                .or_else(|| error.get("message").and_then(serde_json::Value::as_str))
        })
        .or_else(|| {
            matches!(
                item.get("status").and_then(serde_json::Value::as_str),
                Some("failed" | "error" | "declined")
            )
            .then(|| {
                item.get("contentItems")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                    .find_map(|content| content.get("text").and_then(serde_json::Value::as_str))
            })
            .flatten()
        })
        .map(str::to_owned)
}

fn tool_label(
    item_type: &str,
    item: &serde_json::Value,
    progress: Option<&str>,
) -> Option<(&'static str, String)> {
    let (kind, label) = match item_type {
        "commandExecution" => {
            let label = item
                .get("progress_summary")
                .and_then(serde_json::Value::as_str)
                .or_else(|| progress.filter(|label| !label.contains("failed")))
                .unwrap_or("Running a command.")
                .trim_end_matches('.');
            let kind = if label.starts_with("Searching") {
                "search"
            } else if label.starts_with("Inspecting") {
                "inspect"
            } else {
                "command"
            };
            (kind, label.to_owned())
        }
        "fileChange" => ("files", "Editing files".into()),
        "webSearch" => ("web_search", "Searching the web".into()),
        "mcpToolCall" | "dynamicToolCall" => ("tool", tool_activity_label(item)),
        _ => return None,
    };
    Some((kind, label))
}

fn tool_activity_label(item: &serde_json::Value) -> String {
    let tool = item.get("tool").and_then(serde_json::Value::as_str);
    let cwr_tool = tool.and_then(|tool| {
        tool.strip_prefix("cwr.").or_else(|| {
            (item.get("namespace").and_then(serde_json::Value::as_str) == Some("cwr"))
                .then_some(tool)
        })
    });
    match cwr_tool {
        Some("start_action") => "Starting a coworker task".into(),
        Some("invoke_action") => "Starting an action".into(),
        Some("inspect_invocation" | "await_invocation") => "Waiting for a coworker".into(),
        Some("stop_invocation") => "Stopping a coworker task".into(),
        Some("send_message") => "Sending a coworker message".into(),
        Some("return_result")
            if item
                .get("arguments")
                .and_then(|arguments| arguments.get("status"))
                .and_then(serde_json::Value::as_u64)
                .is_some_and(|status| (100..200).contains(&status)) =>
        {
            "Continuing the response".into()
        }
        Some("return_result") => "Finishing the response".into(),
        _ => match tool {
            Some(tool) => format!("Using {tool}"),
            None => "Using a tool".into(),
        },
    }
}
