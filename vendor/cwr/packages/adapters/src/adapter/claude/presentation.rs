// Provider phases are activity metadata, not authored timeline messages.
use crate::adapter::{AdapterModelOutput, ExecutionActivity, ExecutionPhase, ExecutionPhaseState};
use cwr::open_responses::v2026_04_24::{AssistantPhase, StreamValidator};

#[derive(Default)]
struct Presentation {
    phase: Option<ExecutionPhase>,
    blocks: BTreeMap<u64, ExecutionPhase>,
    tools: BTreeMap<String, String>,
    completed_tools: BTreeSet<String>,
    deferred_tools: BTreeMap<String, Value>,
    represented_calls: Vec<(String, Value)>,
    text_items: BTreeSet<String>,
    output_sequence: u64,
    output_index: u64,
    output_validator: StreamValidator,
}

impl<O: Write> Session<'_, O> {
    fn execution_started(&mut self, phase: ExecutionPhase) -> CwrResult<()> {
        if phase != ExecutionPhase::CompactingContext {
            self.finish_compaction()?;
        }
        if self.presentation.phase == Some(phase) {
            return Ok(());
        }
        self.presentation.phase = Some(phase);
        self.publish_activity(AdapterActivityEvent::ExecutionActivity {
            activity: ExecutionActivity::started(phase),
        })
    }

    fn finish_compaction(&mut self) -> CwrResult<()> {
        if self.presentation.phase != Some(ExecutionPhase::CompactingContext) {
            return Ok(());
        }
        self.presentation.phase = None;
        self.publish_activity(AdapterActivityEvent::ExecutionActivity {
            activity: ExecutionActivity {
                phase: ExecutionPhase::CompactingContext,
                state: ExecutionPhaseState::Completed,
                completed_parts: None,
                total_parts: None,
            },
        })
    }

    fn represented_host_tool(&self, name: &str) -> bool {
        let prefix = format!("mcp__{}__", self.config.tool_namespace);
        let Some(local) = name.strip_prefix(&prefix) else {
            return false;
        };
        self.invocation.host_operations.iter().any(|operation| {
            (matches!(
                operation.name.as_str(),
                "cwr.start_action" | "cwr.invoke_action" | "cwr.send_message" | "cwr.create_conversation" | "cwr.list_conversation_contexts"
            ) || operation.input_schema.get("x-cwr-target").is_some())
                && crate::adapter::tools::registration(operation)
                    .is_ok_and(|tool| tool.name == local)
        })
    }

    fn execution_completed(&mut self, phase: ExecutionPhase) -> CwrResult<()> {
        // A block or parallel tool ending must not finish another live item or
        // clear a newer foreground phase. Individual tool events remain exact.
        let pending = self
            .presentation
            .blocks
            .values()
            .any(|active| *active == phase)
            || self
                .presentation
                .tools
                .values()
                .any(|name| tool_phase(name) == phase);
        if pending || self.presentation.phase != Some(phase) {
            return Ok(());
        }
        if let Some(remaining) = self
            .presentation
            .blocks
            .values()
            .next_back()
            .copied()
            .or_else(|| {
                self.presentation
                    .tools
                    .values()
                    .next()
                    .map(|name| tool_phase(name))
            })
        {
            return self.execution_started(remaining);
        }
        self.presentation.phase = None;
        self.publish_activity(AdapterActivityEvent::ExecutionActivity {
            activity: ExecutionActivity {
                phase,
                state: ExecutionPhaseState::Completed,
                completed_parts: None,
                total_parts: None,
            },
        })
    }

    fn stream_progress(&mut self, event: &Value) -> CwrResult<()> {
        if self.session_id.is_none() || self.session_id.as_deref() != event["session_id"].as_str() {
            return Ok(());
        }
        let stream = &event["event"];
        let Some(index) = stream["index"].as_u64() else {
            return Ok(());
        };
        match stream["type"].as_str() {
            Some("content_block_start") => {
                let phase = match stream["content_block"]["type"].as_str() {
                    Some("thinking" | "redacted_thinking") => ExecutionPhase::Thinking,
                    Some("text") => ExecutionPhase::Responding,
                    // Actual tool identities are reported by the completed assistant
                    // event, avoiding duplicate starts from partial and full events.
                    _ => return Ok(()),
                };
                self.presentation.blocks.insert(index, phase);
                self.execution_started(phase)
            }
            Some("content_block_stop") => {
                if let Some(phase) = self.presentation.blocks.remove(&index) {
                    self.execution_completed(phase)?;
                }
                Ok(())
            }
            _ => Ok(()), // Never publish thinking, signatures, or raw partial JSON.
        }
    }

    fn observe_tools(&mut self, event: &Value) -> CwrResult<()> {
        if self.session_id.is_none() || self.session_id.as_deref() != event["session_id"].as_str() {
            return Ok(());
        }
        let Some(blocks) = event["message"]["content"].as_array() else {
            return Ok(());
        };
        for block in blocks {
            match (event["type"].as_str(), block["type"].as_str()) {
                (Some("assistant"), Some("tool_use")) => {
                    let (Some(id), Some(name)) = (block["id"].as_str(), block["name"].as_str())
                    else {
                        continue;
                    };
                    if id.is_empty()
                        || name.is_empty()
                        || name == "StructuredOutput"
                        || self.presentation.tools.contains_key(id)
                        || self.presentation.completed_tools.contains(id)
                    {
                        continue;
                    }
                    self.presentation.tools.insert(id.into(), name.into());
                    self.execution_started(tool_phase(name))?;
                    if self.represented_host_tool(name) {
                        // MCP has no provider tool-use ID. Defer projection until
                        // its result, when exact name/input dispatch evidence exists.
                        self.presentation
                            .deferred_tools
                            .insert(id.into(), block["input"].clone());
                    } else {
                        self.tool_activity("item/started", id, name, "in_progress")?;
                    }
                }
                (Some("user"), Some("tool_result")) => {
                    let Some(id) = block["tool_use_id"].as_str() else {
                        continue;
                    };
                    let Some(name) = self.presentation.tools.remove(id) else {
                        continue;
                    };
                    self.presentation.completed_tools.insert(id.into());
                    let status = if block["is_error"].as_bool() == Some(true) {
                        "failed"
                    } else {
                        "completed"
                    };
                    let represented =
                        if let Some(input) = self.presentation.deferred_tools.remove(id) {
                            if let Some(index) =
                                self.presentation.represented_calls.iter().position(
                                    |(tool, arguments)| tool == &name && arguments == &input,
                                )
                            {
                                self.presentation.represented_calls.remove(index);
                                true
                            } else {
                                self.tool_activity("item/started", id, &name, "in_progress")?;
                                false
                            }
                        } else {
                            false
                        };
                    if !represented {
                        self.tool_activity("item/completed", id, &name, status)?;
                    }
                    self.execution_completed(tool_phase(&name))?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn tool_activity(&mut self, method: &str, id: &str, name: &str, status: &str) -> CwrResult<()> {
        // Names and IDs suffice for lifecycle; arguments and results may contain
        // credentials or private data and do not belong in generic diagnostics.
        let mut item = json!({"id":id,"type":if name == "Bash" {"commandExecution"} else {"dynamicToolCall"},
            "tool":name,"status":status});
        if name == "Bash" {
            item["progress_summary"] = json!("Running a command.");
        }
        self.publish_activity(AdapterActivityEvent::ProviderEvent {
            provider: "claude".into(),
            method: method.into(),
            data: json!({"item":item}),
        })
    }

    fn present_text(&mut self, id: &str, text: &str, phase: AssistantPhase) -> CwrResult<()> {
        let Some(model_run_ref) = self.invocation.model_run_ref.clone() else {
            return Ok(());
        };
        if text.trim().is_empty() || !self.presentation.text_items.insert(id.into()) {
            return Ok(());
        }
        let item = |status: &str, content: Value| {
            json!({"id":id,"type":"message","role":"assistant",
            "status":status,"phase":phase.as_str(),"content":content})
        };
        let part = json!({"type":"output_text","text":text,"annotations":[]});
        let index = self.presentation.output_index;
        // The CLI has no upfront commentary/final phase. Emit only after tool-use
        // or the terminal establishes the phase, rather than guessing from tokens.
        for mut event in [
            json!({"type":"response.output_item.added","output_index":index,"item":item("in_progress",json!([]))}),
            json!({"type":"response.content_part.added","output_index":index,"item_id":id,"content_index":0,"part":{"type":"output_text","text":"","annotations":[]}}),
            json!({"type":"response.output_text.done","output_index":index,"item_id":id,"content_index":0,"text":text}),
            json!({"type":"response.content_part.done","output_index":index,"item_id":id,"content_index":0,"part":part}),
            json!({"type":"response.output_item.done","output_index":index,"item":item("completed",json!([part]))}),
        ] {
            event["sequence_number"] = json!(self.presentation.output_sequence);
            self.presentation.output_validator.accept(&event)?;
            self.presentation.output_sequence += 1;
            write_adapter_message(
                self.output,
                &AdapterEvent::ModelOutput {
                    invocation_ref: self.invocation.invocation_ref.clone(),
                    output: AdapterModelOutput {
                        model_run_ref: model_run_ref.clone(),
                        provider: "claude".into(),
                        thread_id: self.session_id.clone(),
                        response_or_turn_id: if phase == AssistantPhase::Commentary {
                            id.into()
                        } else {
                            self.invocation.invocation_ref.clone()
                        },
                        phase,
                        event,
                    },
                },
            )?;
        }
        self.presentation.output_index += 1;
        Ok(())
    }
}

fn tool_phase(name: &str) -> ExecutionPhase {
    if name == "Bash" {
        ExecutionPhase::RunningCommand
    } else {
        ExecutionPhase::UsingTool
    }
}
