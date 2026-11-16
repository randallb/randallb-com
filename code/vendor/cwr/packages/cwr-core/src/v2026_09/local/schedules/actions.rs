impl LocalRuntime {
    fn execute_schedule_control(
        &self,
        record: &ActionInvocationRecord,
        invocation: &AdapterInvocation,
    ) -> CwrResult<AdapterTerminal> {
        let owner = {
            let state = self.lock()?;
            let conversation = record
                .conversation_ref
                .as_deref()
                .and_then(|r| state.admission.conversation_header(r))
                .ok_or_else(|| {
                    invalid_error("schedule controls require an installed-context conversation")
                })?;
            if !conversation
                .participant_actor_refs
                .contains(&record.requester_actor_ref)
            {
                return invalid("schedule controls require conversation participation");
            }
            conversation
                .owner_context_ref
                .clone()
                .ok_or_else(|| invalid_error("schedule controls require an installed context"))?
        };
        let service = self.schedules_module_as(&owner, &record.requester_actor_ref)?;
        let reference = invocation.input.get("schedule_ref").and_then(Value::as_str);
        let now = current_unix_ms();
        let result = match invocation.action_name.as_str() {
            "list--schedules" => Value::Array(
                service.list()?.into_iter().map(public_schedule_result).collect(),
            ),
            "get--schedule" => {
                public_schedule_result(service.get(reference.ok_or("schedule_ref is required")?)?)
            }
            "pause--schedule" => public_schedule_result(service.set_paused(
                reference.ok_or("schedule_ref is required")?,
                true,
                now,
            )?),
            "resume--schedule" => public_schedule_result(service.set_paused(
                reference.ok_or("schedule_ref is required")?,
                false,
                now,
            )?),
            "list--schedule_runs" => Value::Array(
                service
                    .recent_runs(reference.ok_or("schedule_ref is required")?, 20)?
                    .into_iter()
                    .map(public_schedule_run_result)
                    .collect(),
            ),
            _ => return invalid("unsupported schedule control"),
        };
        let summary = if invocation.action_name == "list--schedule_runs" {
            let runs = result.as_array().unwrap();
            if runs.is_empty() {
                "No recent runs.".into()
            } else {
                runs.iter()
                    .map(|run| {
                        format!(
                            "- {} — {}{}",
                            run["due_unix_ms"],
                            run["outcome"].as_str().unwrap_or("unknown"),
                            run["conversation_ref"]
                                .as_str()
                                .map(|r| format!(" — [Open run]({r})"))
                                .unwrap_or_default()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        } else {
            format!(
                "{}\n\n```json\n{}\n```",
                invocation.action_name,
                serde_json::to_string_pretty(&result)?
            )
        };
        Ok(AdapterTerminal {
            invocation_ref: record.invocation_ref.clone(),
            sequence: 1,
            status: 200,
            summary,
            result: Some(result),
            error: None,
            continuation: None,
        })
    }
}

// Action results are retained in ordinary conversation history. Project the
// same public fields as the collection read contract before writing either the
// result or its summary; saved input and exact target stay in schedule storage.
fn public_schedule_result(schedule: InstalledSchedule) -> Value {
    let ScheduleDeclaration {
        declaration_ref,
        revision,
        anchor_unix_ms,
        interval_ms,
        ..
    } = schedule.definition;
    json!({
        "id": schedule.schedule_ref.clone(),
        "schedule_ref": schedule.schedule_ref,
        "owner_context_ref": schedule.owner_context_ref,
        "author_actor_ref": schedule.author_actor_ref,
        "definition": {
            "declaration_ref": declaration_ref,
            "revision": revision,
            "anchor_unix_ms": anchor_unix_ms,
            "interval_ms": interval_ms,
        },
        "availability": schedule.availability,
        "paused": schedule.paused,
        "next_due_unix_ms": schedule.next_due_unix_ms,
        "active_occurrence_ref": schedule.active_occurrence_ref,
        "last_admission_error": schedule.last_admission_error,
    })
}

fn public_schedule_run_result(run: ScheduleOccurrence) -> Value {
    json!({
        "id": run.occurrence_ref.clone(),
        "occurrence_ref": run.occurrence_ref,
        "schedule_ref": run.schedule_ref,
        "due_unix_ms": run.due_unix_ms,
        "outcome": run.outcome,
        "invocation_ref": run.invocation_ref,
        "conversation_ref": run.conversation_ref,
        "archived": run.archived,
    })
}
