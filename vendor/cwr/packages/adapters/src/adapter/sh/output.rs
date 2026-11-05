#[derive(Clone)]
struct ShellOutputTail {
    bytes: std::collections::VecDeque<u8>,
    clipped: bool,
}

struct ShellOutputCapture {
    tail: std::sync::Mutex<ShellOutputTail>,
    finished: AtomicBool,
}

impl ShellOutputCapture {
    fn new() -> Self {
        Self {
            tail: std::sync::Mutex::new(ShellOutputTail::new()),
            finished: AtomicBool::new(false),
        }
    }

    fn push(&self, bytes: &[u8]) {
        self.tail.lock().expect("shell output capture lock").push(bytes);
    }

    fn snapshot(&self) -> String {
        self.tail
            .lock()
            .expect("shell output capture lock")
            .clone()
            .finish()
    }
}

impl ShellOutputTail {
    fn new() -> Self {
        Self {
            bytes: std::collections::VecDeque::with_capacity(MAX_SHELL_STREAM_BYTES),
            clipped: false,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        let excess = self.bytes.len().saturating_add(bytes.len())
            .saturating_sub(MAX_SHELL_STREAM_BYTES);
        self.clipped |= excess > 0;
        if bytes.len() >= MAX_SHELL_STREAM_BYTES {
            self.bytes.clear();
            self.bytes.extend(&bytes[bytes.len() - MAX_SHELL_STREAM_BYTES..]);
        } else {
            self.bytes.drain(..excess);
            self.bytes.extend(bytes);
        }
    }

    fn finish(mut self) -> String {
        let bytes = self.bytes.make_contiguous();
        // A clipped UTF-8 character may leave continuation bytes at the front.
        // Decode only after drainage so read boundaries cannot split characters.
        let start = if self.clipped {
            bytes.iter().position(|byte| byte & 0xc0 != 0x80).unwrap_or(bytes.len())
        } else {
            0
        };
        let text = String::from_utf8_lossy(&bytes[start..]);
        if !self.clipped && text.len() <= MAX_SHELL_STREAM_BYTES {
            return text.into_owned();
        }
        // Invalid input can expand during lossy decoding. Cap the returned UTF-8
        // string too, including its marker, rather than just the captured bytes.
        let limit = MAX_SHELL_STREAM_BYTES - SHELL_OUTPUT_TRUNCATED.len();
        let mut start = text.len().saturating_sub(limit);
        while !text.is_char_boundary(start) {
            start += 1;
        }
        let mut output = String::from_utf8_lossy(SHELL_OUTPUT_TRUNCATED).into_owned();
        output.push_str(&text[start..]);
        output
    }
}

#[cfg(test)]
fn read_capped_shell_output(mut reader: impl io::Read, stream_output: bool) -> CwrResult<String> {
    let capture = ShellOutputCapture::new();
    read_capped_shell_output_into(&mut reader, stream_output, &capture)?;
    Ok(capture.snapshot())
}

fn read_capped_shell_output_into(
    mut reader: impl io::Read,
    stream_output: bool,
    capture: &ShellOutputCapture,
) -> CwrResult<()> {
    let mut buffer = [0_u8; 8192];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        if stream_output {
            let mut diagnostics = io::stderr().lock();
            let _ = diagnostics.write_all(&buffer[..count]);
            let _ = diagnostics.flush();
        }
        capture.push(&buffer[..count]);
    }
    Ok(())
}

#[cfg(test)]
mod shell_output_tests {
    use super::*;

    include!("output_tests.rs");

    #[test]
    fn long_stderr_is_drained_without_misreading_chunk_boundaries_as_progress() {
        let (sender, receiver) = std::sync::mpsc::sync_channel(SHELL_PROGRESS_QUEUE_CAPACITY);
        let mut bytes = vec![b'x'; MAX_SHELL_STREAM_BYTES + 1];
        bytes.extend_from_slice(b"CWR_PROGRESS\t{\"summary\":\"ordinary stderr\"}\n");
        bytes.extend_from_slice(&vec![b'y'; 2 * 1024 * 1024]);
        bytes.extend_from_slice(b"\nCWR_PROGRESS\t{\"summary\":\"real progress\"}\n");
        bytes.extend_from_slice(b"final diagnostic\n");
        let output =
            read_capped_shell_output_with_progress(io::Cursor::new(bytes), false, Some(sender))
                .unwrap();
        assert_eq!(output.len(), MAX_SHELL_STREAM_BYTES);
        assert!(output.as_bytes().starts_with(SHELL_OUTPUT_TRUNCATED));
        assert!(output.ends_with("final diagnostic\n"));
        assert_eq!(
            receiver
                .into_iter()
                .map(|record| record.summary)
                .collect::<Vec<_>>(),
            ["real progress"]
        );
    }

    #[test]
    fn malformed_and_oversized_records_remain_capped_diagnostics() {
        let (sender, receiver) = std::sync::mpsc::sync_channel(SHELL_PROGRESS_QUEUE_CAPACITY);
        let mut bytes = b"CWR_PROGRESS\t{bad json}\n".to_vec();
        bytes.extend_from_slice(SHELL_PROGRESS_PREFIX);
        bytes.extend_from_slice(&vec![b'x'; 2 * MAX_SHELL_STREAM_BYTES]);
        bytes.extend_from_slice(b"\nCWR_PROGRESS\t{\"summary\":\"ok\"}\n");
        bytes.extend_from_slice(b"final diagnostic\n");
        let output =
            read_capped_shell_output_with_progress(io::Cursor::new(bytes), false, Some(sender))
                .unwrap();
        assert_eq!(output.len(), MAX_SHELL_STREAM_BYTES);
        assert!(output.as_bytes().starts_with(SHELL_OUTPUT_TRUNCATED));
        assert!(output.ends_with("final diagnostic\n"));
        assert_eq!(
            receiver
                .into_iter()
                .map(|record| record.summary)
                .collect::<Vec<_>>(),
            ["ok"]
        );
    }
}

#[cfg(test)]
fn read_capped_shell_output_with_progress(
    reader: impl io::Read,
    stream_output: bool,
    progress: Option<std::sync::mpsc::SyncSender<ShellProgress>>,
) -> CwrResult<String> {
    let capture = ShellOutputCapture::new();
    read_capped_shell_output_with_progress_into(reader, stream_output, progress, &capture)?;
    Ok(capture.snapshot())
}

fn read_capped_shell_output_with_progress_into(
    reader: impl io::Read,
    stream_output: bool,
    progress: Option<std::sync::mpsc::SyncSender<ShellProgress>>,
    capture: &ShellOutputCapture,
) -> CwrResult<()> {
    use std::io::Read;

    if progress.is_none() {
        return read_capped_shell_output_into(reader, stream_output, capture);
    }
    let mut reader = io::BufReader::new(reader);
    let mut line = Vec::with_capacity(MAX_SHELL_STREAM_BYTES + 1);
    let mut line_start = true;
    // Ordinary lines may be arbitrarily long. Consume them in bounded chunks and
    // only interpret a progress prefix at the beginning of an actual stderr line.
    while reader
        .by_ref()
        .take(MAX_SHELL_STREAM_BYTES as u64 + 1)
        .read_until(b'\n', &mut line)?
        > 0
    {
        let record = line_start
            .then(|| line.strip_prefix(SHELL_PROGRESS_PREFIX))
            .flatten()
            .filter(|_| line.len() <= MAX_SHELL_STREAM_BYTES)
            .and_then(|payload| {
                serde_json::from_slice::<ShellProgress>(payload.trim_ascii_end()).ok()
            })
            .filter(|record| {
                !record.summary.trim().is_empty() && !record.summary.contains(['\r', '\n'])
            });
        if let Some(record) = record {
            let _ = progress
                .as_ref()
                .expect("progress sender exists")
                .try_send(record);
        } else {
            if stream_output {
                let mut diagnostics = io::stderr().lock();
                let _ = diagnostics.write_all(&line);
                let _ = diagnostics.flush();
            }
            capture.push(&line);
        }
        line_start = line.ends_with(b"\n");
        line.clear();
    }
    Ok(())
}

fn write_shell_action_input(mut stdin: impl Write, input_json: &Value) -> CwrResult<()> {
    let mut input = serde_json::to_vec(input_json).map_err(|error| {
        protocol_error(format!("could not serialize shell action input: {error}"))
    })?;
    input.push(b'\n');
    match stdin.write_all(&input) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => protocol_failure(format!("could not write shell action input: {error}")),
    }
}

fn drain_shell_progress(
    progress: &std::sync::mpsc::Receiver<ShellProgress>,
    events: &mut ShellAdapterEvents<'_, impl Write>,
) -> CwrResult<()> {
    for _ in 0..SHELL_PROGRESS_QUEUE_CAPACITY {
        let Ok(record) = progress.try_recv() else {
            break;
        };
        events.progress(record)?;
    }
    Ok(())
}

fn command_result_json(result: CommandResult) -> Value {
    let mut value = json!({
        "command": result.command,
        "exit_code": result.exit_code,
        "stdout": result.stdout,
        "stderr": result.stderr,
    });
    if let Some(timeout_ms) = result.timed_out_after_ms {
        value["timed_out_after_ms"] = timeout_ms.into();
    }
    value
}

fn command_result_text(result: &CommandResult) -> String {
    let mut text = result.timed_out_after_ms.map_or_else(
        || format!("`{}` exited with code {}.", result.command, result.exit_code),
        |timeout_ms| format!("`{}` timed out after {timeout_ms} ms.", result.command),
    );
    if !result.stdout.is_empty() {
        text.push_str("\n\nstdout:\n```");
        text.push_str(&result.stdout);
        if !result.stdout.ends_with('\n') {
            text.push('\n');
        }
        text.push_str("```");
    }
    if !result.stderr.is_empty() {
        text.push_str("\n\nstderr:\n```");
        text.push_str(&result.stderr);
        if !result.stderr.ends_with('\n') {
            text.push('\n');
        }
        text.push_str("```");
    }
    text
}
