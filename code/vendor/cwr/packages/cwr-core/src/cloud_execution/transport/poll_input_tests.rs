use super::*;
#[test]
fn idle_poll_wait_wakes_for_input_and_remains_bounded_by_assignment_expiry() {
    let assignment = CloudAssignment {
        assignment_id: "fixture".into(),
        invocation_ref: "cwp://fixture/invocation".into(),
        executor_ref: "cwp://fixture/executor".into(),
        generation: 1,
        expires_at_unix_ms: now() + 1000,
        token: "scoped".into(),
    };
    let (sender, receiver) = mpsc::channel();
    let producer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(10));
        sender.send(Ok(None)).unwrap();
        std::thread::sleep(Duration::from_millis(20));
    });
    let start = std::time::Instant::now();
    assert!(matches!(
        wait_for_poll_input(&receiver, &assignment, Duration::from_secs(1)).unwrap(),
        Some(Ok(None))
    ));
    assert!(start.elapsed() < Duration::from_millis(250));
    producer.join().unwrap();
    let (_sender, receiver) = mpsc::channel();
    let mut expired = assignment;
    expired.expires_at_unix_ms = now() + 30;
    let start = std::time::Instant::now();
    let error = wait_for_poll_input(&receiver, &expired, Duration::from_secs(1)).unwrap_err();
    assert!(error.to_string().contains("expired"));
    assert!(start.elapsed() < Duration::from_millis(250));
}
