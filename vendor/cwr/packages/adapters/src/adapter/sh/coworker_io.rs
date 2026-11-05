// Deadline-aware framing for the private Shell callback socket.
fn write_coworker_message(
    stream: &mut CoworkerStream,
    value: &Value,
    deadline: Instant,
) -> CwrResult<bool> {
    let mut message = serde_json::to_vec(value)?;
    message.push(b'\n');
    if message.len() > MAX_COWORKER_MESSAGE_BYTES {
        return protocol_failure(format!(
            "coworker request exceeds {MAX_COWORKER_MESSAGE_BYTES} bytes"
        ));
    }
    let mut written = 0;
    while written < message.len() {
        if shell_is_stopping() || Instant::now() >= deadline {
            return Ok(false);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(false);
        }
        stream.set_write_timeout(Some(remaining.min(Duration::from_millis(100))))?;
        match stream.write(&message[written..]) {
            Ok(0) => return Err(io::Error::from(io::ErrorKind::WriteZero).into()),
            Ok(count) => written += count,
            Err(error)
                if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(true)
}

fn read_coworker_message(
    stream: &mut CoworkerStream,
    received: &mut Vec<u8>,
    deadline: Instant,
) -> CwrResult<Option<Value>> {
    use std::io::Read;
    let mut buffer = [0_u8; 4096];
    loop {
        if let Some(newline) = received.iter().position(|byte| *byte == b'\n') {
            if newline.saturating_add(1) > MAX_COWORKER_MESSAGE_BYTES {
                return protocol_failure(format!(
                    "coworker request exceeds {MAX_COWORKER_MESSAGE_BYTES} bytes"
                ));
            }
            let frame = received.drain(..=newline).collect::<Vec<_>>();
            return serde_json::from_slice(&frame[..newline])
                .map(Some)
                .map_err(|error| protocol_error(format!("invalid coworker request: {error}")));
        }
        if received.len() >= MAX_COWORKER_MESSAGE_BYTES {
            return protocol_failure(format!(
                "coworker request exceeds {MAX_COWORKER_MESSAGE_BYTES} bytes"
            ));
        }
        if shell_is_stopping() || Instant::now() >= deadline {
            return Ok(None);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(None);
        }
        stream.set_read_timeout(Some(remaining.min(Duration::from_millis(100))))?;
        match stream.read(&mut buffer) {
            Ok(0) => return protocol_failure("coworker command closed before completing its request"),
            Ok(count) => {
                received.extend_from_slice(&buffer[..count]);
            }
            Err(error) if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
            Err(error) => return Err(error.into()),
        }
    }
}

