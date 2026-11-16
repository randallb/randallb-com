fn read_protocol_line(reader: &mut impl BufRead) -> io::Result<Option<String>> {
    loop {
        let mut bytes = Vec::new();
        loop {
            let available = reader.fill_buf()?;
            if available.is_empty() {
                if bytes.is_empty() {
                    return Ok(None);
                }
                break;
            }
            let count = available
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(available.len(), |index| index + 1);
            if bytes.len().saturating_add(count) > MAX_PROTOCOL_LINE_BYTES {
                return Err(protocol_line_too_long());
            }
            let complete = available[count - 1] == b'\n';
            bytes.extend_from_slice(&available[..count]);
            reader.consume(count);
            if complete {
                break;
            }
        }
        let line = String::from_utf8(bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if !line.trim().is_empty() {
            return Ok(Some(line));
        }
    }
}

pub fn read_adapter_message<T: DeserializeOwned>(
    reader: &mut impl BufRead,
) -> CwrResult<Option<T>> {
    let Some(line) = read_protocol_line(reader)? else {
        return Ok(None);
    };
    let mut message: Value = serde_json::from_str(&line)
        .map_err(|error| protocol_error(format!("adapter message is invalid: {error}")))?;
    if message.get("type").and_then(Value::as_str) == Some("invoke")
        && message.pointer("/invocation/thread") == Some(&context_stream_descriptor())
    {
        let invocation_ref = message
            .pointer("/invocation/invocation_ref")
            .and_then(Value::as_str)
            .ok_or_else(|| protocol_error("context stream requires an invocation ref"))?
            .to_owned();
        // Deserialize directly from the chunk reader. There is no encoded copy
        // of the complete history beside the adapter's source context Value.
        let context = serde_json::from_reader(ContextStreamReader::new(reader, invocation_ref))
            .map_err(|error| protocol_error(format!("adapter context is invalid: {error}")))?;
        message["invocation"]["thread"] = context;
    }
    serde_json::from_value(message)
        .map(Some)
        .map_err(|error| protocol_error(format!("adapter message is invalid: {error}")))
}

struct ProtocolFrameBuffer(Vec<u8>);

impl Write for ProtocolFrameBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        // Reserve one byte for the newline. Reject before allocating or writing
        // any part of an oversized control frame to the other process.
        if self.0.len().saturating_add(bytes.len()) >= MAX_PROTOCOL_LINE_BYTES {
            return Err(protocol_line_too_long());
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn encode_adapter_message(message: &impl Serialize) -> CwrResult<Vec<u8>> {
    let mut buffer = ProtocolFrameBuffer(Vec::new());
    serde_json::to_writer(&mut buffer, message)?;
    buffer.0.push(b'\n');
    Ok(buffer.0)
}

pub fn write_adapter_message(writer: &mut impl Write, message: &impl Serialize) -> CwrResult<()> {
    let encoded = encode_adapter_message(message)?;
    writer.write_all(&encoded)?;
    writer.flush()?;
    Ok(())
}
