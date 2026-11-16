use super::{error, trace, Input};
use crate::{
    adapter::{read_adapter_message, write_adapter_message},
    CwrResult,
};
use serde_json::{json, Value};
use std::{
    io::{self, BufReader, Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::Path,
    sync::mpsc::SyncSender,
    thread,
    time::Duration,
};

/// A stdio MCP transport launched by Claude. The private socket keeps the CWR
/// protocol on the parent adapter's stdout; it never appears in the MCP stream.
pub fn run_mcp(socket: &Path) -> CwrResult<()> {
    trace::emit("mcp_child", "connecting to adapter socket");
    let mut stream = UnixStream::connect(socket)?;
    trace::emit("mcp_child", "connected to adapter socket");
    let mut incoming = stream.try_clone()?;
    thread::spawn(move || {
        trace::emit("mcp_child_stdin", "forwarding started");
        let _ = io::copy(&mut io::stdin().lock(), &mut stream);
        trace::emit("mcp_child_stdin", "stdin EOF; shutting down socket write");
        let _ = stream.shutdown(std::net::Shutdown::Write);
    });
    let mut output = io::stdout().lock();
    let mut buffer = [0; 8 * 1024];
    let mut response_bytes = 0;
    loop {
        let count = incoming.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        if response_bytes == 0 {
            trace::emit("mcp_child", "forwarding adapter response");
        }
        output.write_all(&buffer[..count])?;
        output.flush()?;
        for byte in &buffer[..count] {
            response_bytes += 1;
            if *byte == b'\n' {
                trace::emit(
                    "mcp_child",
                    format!("adapter response forwarded bytes={response_bytes}"),
                );
                response_bytes = 0;
            }
        }
    }
    trace::emit("mcp_child", "adapter socket EOF; exiting");
    Ok(())
}

pub(super) fn listen(listener: UnixListener, events: SyncSender<Input>) {
    thread::spawn(move || {
        trace::emit("mcp_listener", "waiting for connection");
        let outcome = (|| -> CwrResult<()> {
            // A single configured MCP child owns this invocation's connection.
            let (stream, _) = listener.accept()?;
            trace::emit("mcp_listener", "connection accepted");
            // A provider that stops reading must not trap the event loop inside
            // a large tool-result write and prevent timeout/cancellation cleanup.
            stream.set_write_timeout(Some(Duration::from_secs(1)))?;
            let mut reader = BufReader::new(stream.try_clone()?);
            while let Some(request) = read_adapter_message::<Value>(&mut reader)? {
                trace::emit(
                    "mcp_listener",
                    format!(
                        "request method={}",
                        request["method"].as_str().unwrap_or("unknown")
                    ),
                );
                events.send(Input::Mcp(request, stream.try_clone()?))?;
            }
            trace::emit("mcp_listener", "client EOF observed");
            Ok(())
        })();
        if let Err(cause) = outcome {
            trace::emit("mcp_listener", format!("failed cause={cause}"));
            let _ = events.send(Input::Failure(cause.to_string()));
        } else {
            trace::emit("mcp_listener", "stopped");
        }
    });
}

pub(super) fn respond(stream: &mut UnixStream, id: &Value, result: Value) -> CwrResult<()> {
    write_adapter_message(stream, &json!({"jsonrpc":"2.0","id":id,"result":result}))
}

pub(super) fn reject(
    stream: &mut UnixStream,
    id: &Value,
    code: i32,
    message: &str,
) -> CwrResult<()> {
    write_adapter_message(
        stream,
        &json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}}),
    )
}

pub(super) fn tool_result(result: Result<Value, Value>) -> Value {
    match result {
        Ok(value) => {
            let mut result =
                json!({"content":[{"type":"text","text":value.to_string()}],"isError":false});
            if value.is_object() {
                result["structuredContent"] = value;
            }
            result
        }
        Err(value) => json!({"content":[{"type":"text","text":value.to_string()}],"isError":true}),
    }
}

pub(super) fn initialize(request: &Value) -> CwrResult<Value> {
    let version = request["params"]["protocolVersion"]
        .as_str()
        .ok_or_else(|| error("MCP initialize requires protocolVersion"))?;
    let version = match version {
        "2024-11-05" | "2025-03-26" | "2025-06-18" | "2025-11-25" => version,
        _ => "2025-11-25",
    };
    Ok(
        json!({"protocolVersion":version,"capabilities":{"tools":{}},
        "serverInfo":{"name":"cwr","version":env!("CARGO_PKG_VERSION")}}),
    )
}
