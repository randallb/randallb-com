//! Capture resident stderr in its own process, so a short-lived client cannot
//! own or interrupt logging. No diagnostic failure is written back into the pipe.
use std::{
    io::{Read, Write},
    os::fd::{AsRawFd, FromRawFd},
    os::unix::net::UnixStream,
    path::Path,
};

pub struct ResidentDiagnostics {
    original: std::fs::File,
    finished: std::sync::mpsc::Receiver<()>,
}
impl Drop for ResidentDiagnostics {
    fn drop(&mut self) {
        // SAFETY: the guard owns the original stderr descriptor until after dup2.
        unsafe {
            libc::dup2(self.original.as_raw_fd(), libc::STDERR_FILENO);
        }
        let _ = self
            .finished
            .recv_timeout(std::time::Duration::from_secs(1));
    }
}
pub fn capture_resident_stderr(state_directory: &Path) -> std::io::Result<ResidentDiagnostics> {
    let (mut reader, writer) = UnixStream::pair()?;
    // SAFETY: dup creates a new owned descriptor; stderr is process-global.
    let original = unsafe { libc::dup(libc::STDERR_FILENO) };
    if original < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: ownership of the newly duplicated fd is transferred once.
    let original = unsafe { std::fs::File::from_raw_fd(original) };
    // SAFETY: fcntl only changes descriptor flags on our owned duplicate.
    if unsafe { libc::fcntl(original.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut fallback = original.try_clone()?;
    let directory = state_directory.join("diagnostics");
    let (finished_send, finished) = std::sync::mpsc::channel();
    // At most 256 scrubbed 16 KiB lines await persistence. A full queue drops
    // evidence rather than blocking adapters on storage contention.
    let (send, receive) = std::sync::mpsc::sync_channel(256);
    let dropped = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let writer_dropped = dropped.clone();
    let run = super::process_run_id();
    std::thread::Builder::new().name("cwr-resident-diagnostic-writer".into()).spawn(move || {
        for record in receive {
            let lost = writer_dropped.swap(0, std::sync::atomic::Ordering::Relaxed);
            if lost > 0 {
                let loss = serde_json::json!({"schema_version": 1, "recorded_at_unix_ms": now_ms(),
                    "source": "resident_stderr", "run_id": run, "pid": std::process::id(),
                    "dropped_lines": lost, "message": "stderr diagnostic queue full; lines omitted"});
                let _ = super::store::write_record(&directory, &loss);
            }
            if super::store::write_record(&directory, &record).is_err() {
                let _ = writeln!(fallback, "cwr: resident diagnostics could not be saved");
            }
        }
        let lost = writer_dropped.swap(0, std::sync::atomic::Ordering::Relaxed);
        if lost > 0 {
            let loss = serde_json::json!({"schema_version": 1, "recorded_at_unix_ms": now_ms(),
                "source": "resident_stderr", "run_id": run, "pid": std::process::id(),
                "dropped_lines": lost, "message": "stderr diagnostic queue full; lines omitted"});
            let _ = super::store::write_record(&directory, &loss);
        }
        let _ = finished_send.send(());
    })?;
    std::thread::Builder::new()
        .name("cwr-resident-diagnostic-drain".into())
        .spawn(move || {
            drain(&mut reader, send, dropped);
        })?;
    // SAFETY: writer is live throughout dup2; fd 2 then owns its own reference.
    if unsafe { libc::dup2(writer.as_raw_fd(), libc::STDERR_FILENO) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(ResidentDiagnostics { original, finished })
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn drain(
    reader: &mut impl Read,
    send: std::sync::mpsc::SyncSender<serde_json::Value>,
    dropped: std::sync::Arc<std::sync::atomic::AtomicU64>,
) {
    let mut bytes = [0u8; 4096];
    let mut pending = Vec::new();
    let mut oversized = false;
    let mut sequence = 0u64;
    let mut emit = |line: &[u8], oversized: bool| {
        sequence += 1;
        let message = if oversized {
            "[oversized stderr line omitted]".to_owned()
        } else {
            super::text::scrub(&String::from_utf8_lossy(line), 16 * 1024)
        };
        let record = serde_json::json!({"schema_version": 1, "recorded_at_unix_ms": now_ms(),
            "source": "resident_stderr", "run_id": super::process_run_id(),
            "pid": std::process::id(), "sequence": sequence, "message": message});
        if send.try_send(record).is_err() {
            dropped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    };
    loop {
        match reader.read(&mut bytes) {
            Ok(0) => {
                if !pending.is_empty() || oversized {
                    emit(&pending, oversized);
                }
                break;
            }
            Ok(n) => {
                for byte in &bytes[..n] {
                    if *byte == b'\n' {
                        emit(&pending, oversized);
                        pending.clear();
                        oversized = false;
                    } else if !oversized {
                        if pending.len() >= 16 * 1024 {
                            pending.clear();
                            oversized = true;
                        } else {
                            pending.push(*byte);
                        }
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stalled_storage_does_not_stop_pipe_draining() {
        let (send, receive) = std::sync::mpsc::sync_channel(2);
        let dropped = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let (mut reader, mut writer) = UnixStream::pair().unwrap();
        let losses = dropped.clone();
        let (done, finished) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            drain(&mut reader, send, losses);
            done.send(()).unwrap();
        });
        // Larger than the pipe capacity, with no persistence consumer at all.
        let producer = std::thread::spawn(move || {
            for _ in 0..10000 {
                writer
                    .write_all(b"Authorization: Bearer secret-value\n")
                    .unwrap();
            }
        });
        finished
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        producer.join().unwrap();
        assert_eq!(dropped.load(std::sync::atomic::Ordering::Relaxed), 9998);
        let saved: Vec<_> = receive.try_iter().collect();
        assert_eq!(saved.len(), 2);
        assert!(saved
            .iter()
            .all(|record| !record.to_string().contains("secret-value")));
    }

    #[test]
    fn resident_stderr_persists_scrubbed_lines_in_its_own_process() {
        const CHILD: &str = "CWR_DIAGNOSTIC_TEST_CHILD";
        if let Some(root) = std::env::var_os(CHILD) {
            let _guard = capture_resident_stderr(Path::new(&root)).unwrap();
            let mut stderr = std::io::stderr();
            stderr.write_all(b"Author").unwrap();
            stderr
                .write_all(b"ization: Bearer private-value\nhealthy status\n")
                .unwrap();
            stderr.write_all(&vec![b'x'; 20_000]).unwrap();
            stderr.write_all(b"secret-tail\nlast line").unwrap();
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "diagnostics::resident::tests::resident_stderr_persists_scrubbed_lines_in_its_own_process", "--nocapture"])
            .env(CHILD, root.path()).status().unwrap();
        assert!(status.success());
        let mut db = crate::cwpdb::NativeStore::open_read_only(
            root.path().join("diagnostics/wlpdb.sqlite3"),
        )
        .unwrap();
        let page = db.diagnostic_records(None, 100).unwrap();
        assert!(!page.truncated);
        let records: Vec<_> = page
            .records
            .into_iter()
            .map(|record| record.value.to_string())
            .collect();
        assert_eq!(records.len(), 4);
        let text = records.join("\n");
        assert!(!text.contains("private-value"));
        assert!(!text.contains("secret-tail"));
        assert!(text.contains("healthy status"));
        assert!(text.contains("oversized stderr line omitted"));
        assert!(text.contains("last line"));
        assert!(!root.path().join("resident-runtime.log").exists());
    }
}
