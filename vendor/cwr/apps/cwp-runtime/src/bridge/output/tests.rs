use super::*;
use std::{io::Read, time::Instant};

fn event(bytes: usize) -> HostEvent {
    HostEvent::CommandFailed {
        request_id: "output".into(),
        error: "x".repeat(bytes),
    }
}

#[test]
fn socket_stall_policy_owns_the_deadline_and_releases_waiters() {
    let (_reader, output) = UnixStream::pair().unwrap();
    let options = output.try_clone().unwrap();
    let writer = host_socket_writer(output).unwrap();
    assert_eq!(
        options.write_timeout().unwrap(),
        Some(HOST_SOCKET_STALL_TIMEOUT)
    );
    options
        .set_write_timeout(Some(Duration::from_millis(25)))
        .unwrap();
    let started = Instant::now();
    let error = write_event(&writer, &event(2 * 1024 * 1024)).unwrap_err();
    assert!(matches!(
        error.downcast_ref::<io::Error>().unwrap().kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    ));
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(write_event(&writer, &event(1)).is_err());
}

#[test]
fn acknowledgement_waits_for_the_whole_frame_and_queued_writes_keep_their_order() {
    let (reader, output) = UnixStream::pair().unwrap();
    let writer = host_socket_writer(output).unwrap();
    let first = event(2 * 1024 * 1024);
    let first_writer = Arc::clone(&writer);
    let first_event = first.clone();
    let (done, completion) = mpsc::channel();
    let writing = thread::spawn(move || {
        let result = write_event(&first_writer, &first_event);
        done.send(()).unwrap();
        result.unwrap();
    });
    assert!(completion.recv_timeout(Duration::from_millis(50)).is_err());
    let second = event(1);
    // Queue behind the first only once its write is actually in progress.
    let mut reader = std::io::BufReader::new(reader);
    let mut prefix = [0; 1];
    reader.read_exact(&mut prefix).unwrap();
    let second_writer = Arc::clone(&writer);
    let second_event = second.clone();
    let queued = thread::spawn(move || write_event(&second_writer, &second_event).unwrap());
    use std::io::BufRead;
    let mut line = String::from_utf8(prefix.to_vec()).unwrap();
    reader.read_line(&mut line).unwrap();
    assert_eq!(serde_json::from_str::<HostEvent>(&line).unwrap(), first);
    assert!(line.ends_with('\n'));
    line.clear();
    reader.read_line(&mut line).unwrap();
    assert_eq!(serde_json::from_str::<HostEvent>(&line).unwrap(), second);
    writing.join().unwrap();
    queued.join().unwrap();
}

#[test]
fn session_teardown_finishes_an_active_frame_before_socket_shutdown() {
    let (mut reader, output) = UnixStream::pair().unwrap();
    let closing_stream = output.try_clone().unwrap();
    let writer = host_socket_writer(output).unwrap();
    let first = event(2 * 1024 * 1024);
    let writing_writer = Arc::clone(&writer);
    let first_event = first.clone();
    let writing = thread::spawn(move || write_event(&writing_writer, &first_event));
    let mut bytes = vec![0; 1];
    reader.read_exact(&mut bytes).unwrap();
    let (finished, completion) = mpsc::channel();
    let closing_writer = Arc::clone(&writer);
    let closing = thread::spawn(move || {
        finish(&closing_writer).unwrap();
        closing_stream.shutdown(std::net::Shutdown::Both).unwrap();
        finished.send(()).unwrap();
    });
    assert!(completion.recv_timeout(Duration::from_millis(50)).is_err());
    reader.read_to_end(&mut bytes).unwrap();
    assert!(bytes.ends_with(b"\n"));
    assert_eq!(serde_json::from_slice::<HostEvent>(&bytes).unwrap(), first);
    writing.join().unwrap().unwrap();
    closing.join().unwrap();
    assert!(write_event(&writer, &event(1)).is_err());
}

#[test]
fn session_teardown_releases_a_stalled_output_writer() {
    let (mut reader, output) = UnixStream::pair().unwrap();
    let options = output.try_clone().unwrap();
    let writer = host_socket_writer(output).unwrap();
    options
        .set_write_timeout(Some(Duration::from_millis(25)))
        .unwrap();
    let writing_writer = Arc::clone(&writer);
    let writing = thread::spawn(move || write_event(&writing_writer, &event(2 * 1024 * 1024)));
    reader.read_exact(&mut [0; 1]).unwrap();
    let started = Instant::now();
    finish(&writer).unwrap_err();
    assert!(writing.join().unwrap().is_err());
    assert!(started.elapsed() < Duration::from_secs(3));
}
