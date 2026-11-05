use super::*;
use std::{io::BufReader, net::TcpListener, thread};
fn assignment() -> CloudAssignment {
    CloudAssignment {
        assignment_id: "fixture".into(),
        invocation_ref: "cwp://fixture/invocation".into(),
        executor_ref: "cwp://fixture/executor".into(),
        generation: 1,
        expires_at_unix_ms: now() + 10000,
        token: "fictional-scoped-token".into(),
    }
}
fn serve(responses: Vec<Option<&'static str>>) -> (Url, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        responses
            .into_iter()
            .map(|response| {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = String::new();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    request.push_str(&line);
                    if line == "\r\n" {
                        break;
                    }
                }
                if let Some(response) = response {
                    stream.write_all(response.as_bytes()).unwrap();
                }
                request
            })
            .collect()
    });
    (
        Url::parse(&format!(
            "http://{address}/v1/assignments/fixture/events?after=492"
        ))
        .unwrap(),
        handle,
    )
}
#[test]
fn transient_event_reads_reuse_cursor_without_redispatch_or_duplicate_frames() {
    let body = r#"{"events":[{"sequence":493,"frame":{"invocation_ref":"cwp://fixture/invocation","type":"fixture"}}],"terminal":true}"#;
    let success = Box::leak(
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_boxed_str(),
    );
    let (url, server) = serve(vec![
        None,
        Some("HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
        Some(success),
    ]);
    let client = Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    let frames = read_events(&client, url, &assignment()).unwrap();
    assert_eq!(frames.events.len(), 1);
    assert_eq!(frames.events[0].sequence, 493);
    assert!(frames.terminal);
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 3);
    for request in requests {
        assert!(request.starts_with("GET /v1/assignments/fixture/events?after=492 HTTP/1.1\r\n"));
        assert!(request
            .to_lowercase()
            .contains("authorization: bearer fictional-scoped-token\r\n"));
    }
}
#[test]
fn refused_and_malformed_event_reads_are_not_retried() {
    for response in [
        "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        "HTTP/1.1 200 OK\r\nContent-Length: 1\r\nConnection: close\r\n\r\nx",
    ] {
        let (url, server) = serve(vec![Some(response)]);
        assert!(read_events(&Client::new(), url, &assignment()).is_err());
        assert_eq!(server.join().unwrap().len(), 1);
    }
}
#[test]
fn expired_assignment_cannot_start_an_event_read() {
    let mut selected = assignment();
    selected.expires_at_unix_ms = now().saturating_sub(1);
    let error = read_events(
        &Client::new(),
        Url::parse("http://127.0.0.1:1/").unwrap(),
        &selected,
    )
    .unwrap_err();
    assert!(error.to_string().contains("expired"));
}

#[test]
fn exhausted_event_transport_retains_safe_error_classification() {
    let (url, server) = serve(vec![None, None, None]);
    let error = read_events(&Client::new(), url, &assignment())
        .unwrap_err()
        .to_string();
    assert!(error.contains("timeout=false"));
    assert!(!error.contains("fictional-scoped-token"));
    assert!(!error.contains("/v1/assignments"));
    assert_eq!(server.join().unwrap().len(), 3);
}
