use super::{config::Config, error, failure, mcp, trace};
use crate::{
    action_contract::{START_ACTION_HOST_OPERATION, STOP_INVOCATION_HOST_OPERATION},
    adapter::{
        write_adapter_message, AdapterActivityEvent, AdapterError, AdapterEvent,
        AdapterHostMessage, AdapterInvocation,
    },
    CwrResult,
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    os::unix::net::UnixStream,
};

struct PendingCall {
    id: Value,
    stream: UnixStream,
}

pub(super) struct Session<'a, O> {
    invocation: &'a AdapterInvocation,
    config: &'a Config,
    output: &'a mut O,
    sequence: u64,
    activity_sequence: u64,
    activities: BTreeSet<u64>,
    calls: BTreeMap<String, PendingCall>,
    session_id: Option<String>,
    returned: Option<Value>,
    last_progress: Option<String>,
    presentation: Presentation,
    pending_commentary: Option<(String, String)>,
    pub provider_failed: bool,
    mcp_initialized: bool,
    pub provider_closed: bool,
}

impl<'a, O: Write> Session<'a, O> {
    pub fn new(invocation: &'a AdapterInvocation, config: &'a Config, output: &'a mut O) -> Self {
        Self {
            invocation,
            config,
            output,
            sequence: 1,
            activity_sequence: 1,
            activities: BTreeSet::new(),
            calls: BTreeMap::new(),
            session_id: None,
            returned: None,
            last_progress: None,
            presentation: Presentation::default(),
            pending_commentary: None,
            provider_failed: false,
            mcp_initialized: false,
            provider_closed: false,
        }
    }

    pub fn progress(&mut self, summary: &str) -> CwrResult<()> {
        if self.last_progress.as_deref() == Some(summary) {
            return Ok(());
        }
        write_adapter_message(
            self.output,
            &AdapterEvent::Progress {
                progress: None,
                published: false,
                invocation_ref: self.invocation.invocation_ref.clone(),
                sequence: self.sequence,
                status: 100,
                summary: summary.into(),
            },
        )?;
        self.last_progress = Some(summary.into());
        self.sequence += 1;
        Ok(())
    }

    pub fn host(&mut self, message: AdapterHostMessage) -> CwrResult<()> {
        match message {
            AdapterHostMessage::ActivityResult {
                invocation_ref,
                sequence,
                error: failure,
            } if invocation_ref == self.invocation.invocation_ref
                && self.activities.remove(&sequence) =>
            {
                if failure.is_some() {
                    return Err(error("CWR rejected Claude activity"));
                }
            }
            AdapterHostMessage::CallResult {
                invocation_ref,
                call_id,
                result,
                error: failure,
            } if invocation_ref == self.invocation.invocation_ref => {
                let mut pending = self
                    .calls
                    .remove(&call_id)
                    .ok_or_else(|| error("Unknown Claude host call result"))?;
                let result = match (result, failure) {
                    (Some(value), None) => Ok(value),
                    (None, Some(failure)) => Err(serde_json::to_value(failure)?),
                    _ => {
                        return Err(error(
                            "Claude host call requires exactly one result or error",
                        ))
                    }
                };
                mcp::respond(&mut pending.stream, &pending.id, mcp::tool_result(result))?;
            }
            _ => return Err(error("Unexpected or mismatched Claude host message")),
        }
        Ok(())
    }

    pub fn mcp(&mut self, request: Value, mut stream: UnixStream) -> CwrResult<()> {
        let Some(id) = request.get("id") else {
            return Ok(());
        };
        if request["jsonrpc"] != "2.0" || !(id.is_number() || id.is_string()) {
            return mcp::reject(&mut stream, id, -32600, "Invalid request");
        }
        match request["method"].as_str() {
            Some("initialize") if !self.mcp_initialized => {
                let result = mcp::initialize(&request)?;
                self.mcp_initialized = true;
                mcp::respond(&mut stream, id, result)
            }
            Some("ping") => mcp::respond(&mut stream, id, json!({})),
            Some("tools/list") if self.mcp_initialized => {
                mcp::respond(&mut stream, id, json!({"tools":self.config.tools}))
            }
            Some("tools/call") if self.mcp_initialized => self.call(&request, stream),
            _ => mcp::reject(&mut stream, id, -32601, "Method not found"),
        }
    }

    fn call(&mut self, request: &Value, mut stream: UnixStream) -> CwrResult<()> {
        let id = &request["id"];
        let Some(tool) = self
            .config
            .tools
            .iter()
            .find(|tool| tool["name"] == request["params"]["name"])
        else {
            return mcp::reject(&mut stream, id, -32602, "Unknown CWR tool");
        };
        let input = request["params"]
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if !jsonschema::is_valid(&tool["inputSchema"], &input) {
            return mcp::respond(
                &mut stream,
                id,
                mcp::tool_result(Err(
                    json!({"code":"invalid_input","message":"Input does not match the declared CWR tool schema"}),
                )),
            );
        }
        let operation = self
            .invocation
            .host_operations
            .iter()
            .find(|operation| {
                crate::adapter::tools::registration(operation)
                    .is_ok_and(|name| Some(name.name.as_str()) == tool["name"].as_str())
            })
            .expect("tool came from a declared host operation");
        if self.calls.len() >= 64 || self.calls.values().any(|pending| pending.id == *id) {
            return mcp::reject(
                &mut stream,
                id,
                -32600,
                "Too many pending calls or duplicate request id",
            );
        }
        if self.returned.is_some() {
            return Err(error(
                "Claude requested a host call after its terminal result",
            ));
        }
        let (operation, input) = match crate::adapter::tools::dispatch(operation, input) {
            Ok(decoded) => decoded,
            Err(error) => {
                return mcp::respond(
                    &mut stream,
                    id,
                    mcp::tool_result(Err(
                        json!({"code":"tool_contract_rejected","message":error.to_string()}),
                    )),
                )
            }
        };
        if let Some(summary) = host_call_progress(&operation, &input) {
            self.progress(&summary)?;
        }
        let call_id = uuid::Uuid::new_v4().to_string();
        let represented = matches!(
            operation.as_str(),
            "cwr.start_action"
                | "cwr.invoke_action"
                | "cwr.send_message"
                | "cwr.create_conversation"
                | "cwr.list_conversation_contexts"
        );
        write_adapter_message(
            self.output,
            &AdapterEvent::Call {
                invocation_ref: self.invocation.invocation_ref.clone(),
                call_id: call_id.clone(),
                operation,
                input,
            },
        )?;
        if represented {
            self.presentation.represented_calls.push((
                format!(
                    "mcp__{}__{}",
                    self.config.tool_namespace,
                    request["params"]["name"].as_str().expect("validated tool")
                ),
                request["params"]
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({})),
            ));
        }
        self.calls.insert(
            call_id,
            PendingCall {
                id: id.clone(),
                stream,
            },
        );
        Ok(())
    }

    pub fn provider(&mut self, event: Value) -> CwrResult<()> {
        match (event["type"].as_str(), event["subtype"].as_str()) {
            (Some("system"), Some("init")) => self.initialize_provider(&event),
            (Some("system"), Some("api_retry")) => {
                if matches!(
                    event["error"].as_str(),
                    Some("authentication_failed" | "permission_error")
                ) || matches!(event["status"].as_u64(), Some(401 | 403))
                {
                    return Err(error(
                        "Claude authentication failed; check the host's Anthropic credentials",
                    ));
                }
                self.progress("Claude is retrying a provider request.")
            }
            (Some("system"), Some("status"))
                if self.session_id.is_some()
                    && self.session_id.as_deref() == event["session_id"].as_str() =>
            {
                if event["status"] != "compacting" {
                    self.finish_compaction()?;
                }
                match event["status"].as_str() {
                    Some("requesting") => {
                        self.execution_started(crate::adapter::ExecutionPhase::Waiting)
                    }
                    Some("compacting") => {
                        self.execution_started(crate::adapter::ExecutionPhase::CompactingContext)
                    }
                    _ => Ok(()),
                }
            }
            (Some("stream_event"), _) => self.stream_progress(&event),
            (Some("result"), _) => self.receive_result(event),
            (Some("assistant"), _) => {
                self.observe_commentary(&event)?;
                self.observe_tools(&event)
            }
            (Some("user"), _) => self.observe_tools(&event),
            // Assistant tool_use is telemetry; only MCP tools/call executes work.
            // Thinking and intermediate text never become final user responses.
            _ => Ok(()),
        }
    }

    fn initialize_provider(&mut self, event: &Value) -> CwrResult<()> {
        if self.session_id.is_some() {
            return Err(error("Claude initialized more than once"));
        }
        let session_id = event["session_id"]
            .as_str()
            .ok_or_else(|| error("Claude session id is missing"))?;
        uuid::Uuid::parse_str(session_id).map_err(|_| error("Claude session id is invalid"))?;
        if self
            .config
            .resume
            .as_deref()
            .is_some_and(|prior| prior == session_id)
        {
            return Err(error("Claude did not fork the previous settled session"));
        }
        let connected = event["mcp_servers"].as_array().is_some_and(|servers| {
            servers.iter().any(|server| {
                server["name"] == self.config.tool_namespace && server["status"] == "connected"
            })
        });
        if !connected {
            return Err(error(
                "Claude failed to connect the required CWR MCP server",
            ));
        }
        let mut expected: BTreeSet<_> = self
            .config
            .tools
            .iter()
            .map(|tool| {
                format!(
                    "mcp__{}__{}",
                    self.config.tool_namespace,
                    tool["name"].as_str().expect("validated tool")
                )
            })
            .collect();
        expected.extend(self.config.native_tools.iter().cloned());
        let tools = event["tools"]
            .as_array()
            .ok_or_else(|| error("Claude did not report its tools"))?;
        for name in &expected {
            if !tools.iter().any(|tool| tool == name) {
                return Err(error(format!(
                    "Claude did not load required tool {name}; reported tools: {}",
                    tools
                        .iter()
                        .map(|tool| tool.as_str().unwrap_or("<invalid>"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
        }
        if tools.iter().any(|tool| {
            tool.as_str().is_none_or(|name| {
                !(expected.contains(name) || name == "StructuredOutput" && !self.config.native_text)
            })
        }) {
            let unexpected = tools.iter().find_map(|tool| {
                tool.as_str().filter(|name| {
                    !(expected.contains(*name)
                        || *name == "StructuredOutput" && !self.config.native_text)
                })
            });
            return Err(error(format!(
                "Claude exposed unexpected tool {}",
                unexpected.unwrap_or("<invalid tool name>")
            )));
        }
        self.session_id = Some(session_id.into());
        let sequence = self.activity_sequence;
        write_adapter_message(
            self.output,
            &AdapterEvent::Activity {
                invocation_ref: self.invocation.invocation_ref.clone(),
                sequence,
                event: AdapterActivityEvent::ProviderThread {
                    provider: "claude".into(),
                    thread_id: session_id.into(),
                },
            },
        )?;
        self.activities.insert(sequence);
        self.activity_sequence += 1;
        Ok(())
    }

    fn publish_activity(&mut self, event: AdapterActivityEvent) -> CwrResult<()> {
        let sequence = self.activity_sequence;
        write_adapter_message(
            self.output,
            &AdapterEvent::Activity {
                invocation_ref: self.invocation.invocation_ref.clone(),
                sequence,
                event,
            },
        )?;
        self.activities.insert(sequence);
        self.activity_sequence += 1;
        Ok(())
    }

    pub fn ready(&self) -> CwrResult<bool> {
        if self.provider_closed && self.returned.is_none() {
            return Err(error("Claude exited without a terminal result"));
        }
        Ok(self.provider_closed
            && self.returned.is_some()
            && self.activities.is_empty()
            && self.calls.is_empty())
    }

    pub fn trace_state(&self, event: &str) {
        trace::emit(
            "session",
            format!(
                "{event} provider_closed={} provider_failed={} returned={} activities={} calls={} mcp_initialized={}",
                self.provider_closed,
                self.provider_failed,
                self.returned.is_some(),
                self.activities.len(),
                self.calls.len(),
                self.mcp_initialized
            ),
        );
    }

    pub fn complete(&mut self) -> CwrResult<()> {
        self.trace_state("writing terminal result");
        let returned = self
            .returned
            .take()
            .ok_or_else(|| error("Claude terminal result unavailable"))?;
        let error = returned
            .get("error")
            .cloned()
            .map(serde_json::from_value::<AdapterError>)
            .transpose()?;
        write_adapter_message(
            self.output,
            &AdapterEvent::Result {
                invocation_ref: self.invocation.invocation_ref.clone(),
                sequence: self.sequence,
                status: returned["status"].as_u64().expect("validated status") as u16,
                summary: returned["summary"]
                    .as_str()
                    .expect("validated summary")
                    .into(),
                result: returned.get("result").cloned(),
                error,
                continuation: if self.provider_failed {
                    None
                } else {
                    self.session_id.clone().map(Value::String)
                },
            },
        )
    }
}

fn host_call_progress(operation: &str, input: &Value) -> Option<String> {
    match operation {
        START_ACTION_HOST_OPERATION => input["activation"]
            .as_str()
            .map(|activation| format!("Starting {activation}.")),
        STOP_INVOCATION_HOST_OPERATION => Some("Stopping coworker work.".into()),
        _ => None,
    }
}

include!("commentary.rs");

include!("presentation.rs");
include!("result.rs");
