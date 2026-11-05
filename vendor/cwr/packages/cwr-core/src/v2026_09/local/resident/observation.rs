/// A live, non-subscribing owner response. This is not a drain or shutdown proof.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LocalResidentObservation {
    pub version: u32,
    pub runtime_ref: String,
    pub owner_pid: i32,
    pub owner_uid: u32,
    /// The existing drain-status counter, not proof that admission is fenced.
    pub remaining: usize,
    pub observed_at_unix_ms: u64,
}

/// Inspect only an existing owner. Never starts a resident, changes admission,
/// subscribes to conversation data, or converts a missing socket into idle work.
/// The caller supplies the runtime reference from its trusted saved selection.
pub fn observe_existing_local_resident(
    runtime_directory: &Path,
    expected_runtime_ref: &str,
) -> CwrResult<LocalResidentObservation> {
    use super::service::{RuntimeServiceCommand, RuntimeServiceEvent, RUNTIME_SERVICE_VERSION};
    use std::io::{BufReader, Write as _};
    if !expected_runtime_ref.starts_with("cwp://")
        || expected_runtime_ref.len() > 2048
        || expected_runtime_ref.chars().any(char::is_whitespace)
    {
        return invalid("invalid expected resident runtime reference");
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut stream =
        resident_observation_connect(&runtime_directory.join("runtime.sock"), deadline)?;
    let pid = local_resident_peer_pid(&stream)?;
    let host = resident_observation_connect(&runtime_directory.join("host.sock"), deadline)?;
    if pid <= 1 || local_resident_peer_pid(&host)? != pid {
        return invalid("resident observation sockets belong to different owners");
    }
    drop(host);

    let mut reader = BufReader::new(stream.try_clone()?);
    match resident_observation_frame(&mut reader, deadline)? {
        RuntimeServiceEvent::Ready { runtime_ref } if runtime_ref == expected_runtime_ref => {}
        _ => return invalid("resident observation runtime identity did not match"),
    }
    let request_id = format!("observe-{}", Uuid::new_v4());
    stream.set_write_timeout(Some(resident_observation_remaining(deadline)?))?;
    serde_json::to_writer(
        &mut stream,
        &RuntimeServiceCommand::Drain {
            version: RUNTIME_SERVICE_VERSION,
            request_id: request_id.clone(),
            operation: "supports-resident-observation-status".into(),
        },
    )?;
    stream.set_write_timeout(Some(resident_observation_remaining(deadline)?))?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    let remaining = match resident_observation_frame(&mut reader, deadline)? {
        RuntimeServiceEvent::DrainStatus {
            request_id: response,
            remaining,
        } if response == request_id => remaining,
        _ => return invalid("resident observation was not acknowledged"),
    };
    // SAFETY: geteuid takes no pointers; resident_observation_connect already verified
    // that both sockets belong to this effective user.
    let owner_uid = unsafe { libc::geteuid() };
    Ok(LocalResidentObservation {
        version: 1,
        runtime_ref: expected_runtime_ref.into(),
        owner_pid: pid,
        owner_uid,
        remaining,
        observed_at_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_millis()
            .try_into()?,
    })
}

fn resident_observation_remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|value| !value.is_zero())
        .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "resident observation timed out"))
}

fn resident_observation_connect(
    path: &Path,
    deadline: Instant,
) -> io::Result<std::os::unix::net::UnixStream> {
    let socket = socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None)?;
    socket.connect_timeout(
        &socket2::SockAddr::unix(path)?,
        resident_observation_remaining(deadline)?,
    )?;
    let stream = std::os::unix::net::UnixStream::from(socket);
    verify_local_resident_peer(&stream)?;
    Ok(stream)
}

fn resident_observation_frame(
    reader: &mut std::io::BufReader<std::os::unix::net::UnixStream>,
    deadline: Instant,
) -> CwrResult<super::service::RuntimeServiceEvent> {
    use std::io::BufRead as _;
    let mut line = Vec::new();
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| invalid_error("resident observation timed out"))?;
        reader.get_mut().set_read_timeout(Some(remaining))?;
        let bytes = reader.fill_buf()?;
        if bytes.is_empty() {
            return invalid("resident closed before observation completed");
        }
        let count = bytes
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(bytes.len(), |index| index + 1);
        if line.len() + count > 16 * 1024 {
            return invalid("resident observation frame is too large");
        }
        let complete = bytes[count - 1] == b'\n';
        line.extend_from_slice(&bytes[..count]);
        reader.consume(count);
        if complete {
            break;
        }
    }
    serde_json::from_slice(&line).map_err(|_| invalid_error("invalid resident observation frame"))
}

#[cfg(test)]
mod observation_tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        os::unix::net::UnixListener,
    };
    const REF: &str = "cwp://local.fixture/runtimes/00000000-0000-4000-8000-000000000001";

    fn fixture(ready: Vec<u8>, expected: &str) -> CwrResult<LocalResidentObservation> {
        let root = tempfile::tempdir().unwrap();
        let runtime = UnixListener::bind(root.path().join("runtime.sock")).unwrap();
        let host = UnixListener::bind(root.path().join("host.sock")).unwrap();
        let host_thread = thread::spawn(move || {
            let _ = host.accept();
        });
        let runtime_thread = thread::spawn(move || {
            let (mut stream, _) = runtime.accept().unwrap();
            let _ = stream.write_all(&ready);
            let mut line = String::new();
            if BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap_or(0)
                == 0
            {
                return;
            }
            let command: super::super::service::RuntimeServiceCommand =
                serde_json::from_str(&line).unwrap();
            if let super::super::service::RuntimeServiceCommand::Drain {
                request_id,
                operation,
                ..
            } = command
            {
                assert_eq!(operation, "supports-resident-observation-status");
                let response =
                    serde_json::to_vec(&super::super::service::RuntimeServiceEvent::DrainStatus {
                        request_id,
                        remaining: 7,
                    })
                    .unwrap();
                let _ = stream.write_all(&response);
                let _ = stream.write_all(b"\n");
            } else {
                panic!("observation must not send a mutating command");
            }
        });
        let result = observe_existing_local_resident(root.path(), expected);
        runtime_thread.join().unwrap();
        host_thread.join().unwrap();
        result
    }
    #[test]
    fn observes_verified_owner_and_explicit_counter_without_starting_work() {
        let ready = format!("{{\"kind\":\"ready\",\"runtime_ref\":\"{REF}\"}}\n").into_bytes();
        let result = fixture(ready, REF).unwrap();
        assert_eq!(result.runtime_ref, REF);
        assert_eq!(result.owner_pid, std::process::id() as i32);
        assert_eq!(result.remaining, 7);
        assert_eq!(result.version, 1);
    }
    #[test]
    fn stalled_owner_response_respects_the_remaining_deadline() {
        let (stream, _silent_owner) = std::os::unix::net::UnixStream::pair().unwrap();
        let started = Instant::now();
        assert!(resident_observation_frame(
            &mut BufReader::new(stream),
            started + Duration::from_millis(30)
        )
        .is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(resident_observation_remaining(started).is_err());
    }

    #[test]
    fn missing_owner_is_unknown_and_allocates_nothing() {
        let root = tempfile::tempdir().unwrap();
        assert!(observe_existing_local_resident(root.path(), REF).is_err());
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
    #[test]
    fn mismatched_or_oversized_owner_frames_never_become_idle() {
        assert!(fixture(
            b"{\"kind\":\"ready\",\"runtime_ref\":\"cwp://local.other/runtime\"}\n".to_vec(),
            REF
        )
        .is_err());
        assert!(fixture(vec![b'x'; 17000], REF).is_err());
    }
}
