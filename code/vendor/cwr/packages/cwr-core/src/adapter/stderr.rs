const MAX_DIAGNOSTIC_STDERR_BYTES: usize = 128 * 1024;

#[derive(Debug, Default)]
struct CapturedStderr {
    tail: String,
    total_bytes: u64,
    truncated: bool,
    complete: bool,
    error: Option<String>,
}

struct StderrReader {
    cancellation: UnixStream,
    thread: thread::JoinHandle<CapturedStderr>,
}

impl StderrReader {
    fn finish(self) -> CapturedStderr {
        let deadline = Instant::now() + Duration::from_millis(100);
        while !self.thread.is_finished() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        // Wake poll even when a descendant still owns the pipe. The reader
        // returns its accumulated tail and closes its own descriptor.
        drop(self.cancellation);
        let mut captured = self.thread.join().unwrap_or_else(|_| CapturedStderr {
            error: Some("stderr reader panicked".into()),
            ..Default::default()
        });
        if !captured.complete && captured.error.is_none() {
            captured.error = Some("stderr stream remained open after collection timeout".into());
        }
        captured
    }
}

fn spawn_capped_stderr_reader(
    mut stderr: impl Read + AsRawFd + Send + 'static,
) -> io::Result<StderrReader> {
    let (cancellation, reader_cancellation) = UnixStream::pair()?;
    let thread = thread::spawn(move || {
        let mut captured = CapturedStderr::default();
        let mut tail = Vec::new();
        let mut buffer = [0_u8; 8192];
        loop {
            match wait_for_protocol_input(stderr.as_raw_fd(), reader_cancellation.as_raw_fd()) {
                Ok(true) => {}
                Ok(false) => break,
                Err(error) => {
                    captured.error = Some(error.to_string());
                    break;
                }
            }
            match stderr.read(&mut buffer) {
                Ok(0) => {
                    captured.complete = true;
                    break;
                }
                Ok(count) => {
                    captured.total_bytes = captured.total_bytes.saturating_add(count as u64);
                    tail.extend_from_slice(&buffer[..count]);
                    if tail.len() > MAX_DIAGNOSTIC_STDERR_BYTES {
                        tail.drain(..tail.len() - MAX_DIAGNOSTIC_STDERR_BYTES);
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    captured.error = Some(error.to_string());
                    break;
                }
            }
        }
        captured.truncated = captured.total_bytes > tail.len() as u64;
        captured.tail = String::from_utf8_lossy(&tail).into_owned();
        captured
    });
    Ok(StderrReader { cancellation, thread })
}
