// The Shell callback exposes only host-supplied gateway contracts.
fn handle_shell_gateway<W: Write>(
    stream: &mut CoworkerStream,
    events: &mut ShellAdapterEvents<'_, W>,
    deadline: Instant,
    mut describe: Value,
    mut received: Vec<u8>,
) -> CwrResult<bool> {
    if describe["type"] == "describe_request" {
        let Some(operation) = events.gateway_operation.as_ref() else {
            return write_coworker_message(
                stream,
                &json!({"type":"error", "message":"gateway requests are unavailable"}),
                deadline,
            );
        };
        if !write_coworker_message(
            stream,
            &json!({"type":"gateway", "input_schema":operation.input_schema, "output_schema":operation.output_schema}),
            deadline,
        )? {
            return Ok(false);
        }
        let Some(request) = read_coworker_message(stream, &mut received, deadline)? else {
            return Ok(false);
        };
        if request["type"] != "send_request" {
            return protocol_failure("expected send_request after gateway discovery");
        }
        describe = request;
    }
    if describe["type"] == "send_request" {
        let Some(operation) = events.gateway_operation.clone() else {
            return write_coworker_message(
                stream,
                &json!({"type":"error", "message":"gateway requests are unavailable"}),
                deadline,
            );
        };
        let input = describe.get("input").cloned().unwrap_or(Value::Null);
        if !jsonschema::is_valid(&operation.input_schema, &input) {
            return write_coworker_message(
                stream,
                &json!({"type":"error", "message":"gateway input does not match the permitted contract"}),
                deadline,
            );
        }
        let call_id = "shell-gateway-request-1";
        if !events.write_host_event_until(
            &AdapterEvent::Call {
                invocation_ref: events.invocation_ref.into(),
                call_id: call_id.into(),
                operation: operation.name,
                input,
            },
            deadline,
        )? {
            return Ok(false);
        }
        let Some(reply) = events.receive_until(deadline)? else {
            return Ok(false);
        };
        return match reply {
            AdapterHostMessage::CallResult {
                invocation_ref,
                call_id: returned,
                result,
                error,
            } if invocation_ref == events.invocation_ref && returned == call_id => {
                let reply = match (result, error) {
                    (Some(result), None)
                        if jsonschema::is_valid(&operation.output_schema, &result) =>
                    {
                        json!({"type":"completed", "result":result})
                    }
                    (None, Some(error)) => {
                        json!({"type":"error", "message":error.message.unwrap_or(error.code)})
                    }
                    _ => return protocol_failure("invalid gateway response"),
                };
                write_coworker_message(stream, &reply, deadline)
            }
            _ => protocol_failure("CWR returned a gateway result for another call"),
        };
    }
    protocol_failure("unsupported Shell gateway request")
}
