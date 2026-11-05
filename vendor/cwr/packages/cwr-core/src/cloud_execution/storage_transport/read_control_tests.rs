use super::*;
use cwpdb_store::conversation::{ReadCancellation, ReadControl};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::Instant;
#[test]
fn read_transport_cancellation_and_deadline_close_stalled_response() {
    for cancel in [true, false] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/declared", listener.local_addr().unwrap());
        let (ready, accepted) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse::<usize>().unwrap();
                }
            }
            reader.read_exact(&mut vec![0; length]).unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\nConnection: close\r\n\r\n{")
                .unwrap();
            stream.flush().unwrap();
            ready.send(()).unwrap();
            let mut byte = [0];
            assert_eq!(
                stream.read(&mut byte).unwrap(),
                0,
                "cancellation/deadline must close the pending request, not detach it"
            );
        });
        let cancellation = ReadCancellation::default();
        let stopping = cancellation.clone();
        let control = ReadControl {
            cancellation,
            deadline: Instant::now()
                + if cancel {
                    Duration::from_secs(5)
                } else {
                    Duration::from_millis(200)
                },
            vm_steps: None,
        };
        let worker = std::thread::spawn(move || {
            let mut transport = HttpDeclaredTransport::new(
                &endpoint,
                "fictional-storage-token-only-32-characters".into(),
            )
            .unwrap();
            transport.call_read(serde_json::json!({"operation":"conversation_timeline_page","request":{"fixture":true}}),&control).unwrap_err().to_string()
        });
        accepted.recv_timeout(Duration::from_secs(5)).unwrap();
        if cancel {
            stopping.cancel();
        }
        let error = worker.join().unwrap();
        assert!(
            error.contains(if cancel { "cancelled" } else { "deadline" }),
            "{error}"
        );
        server.join().unwrap();
    }
}
#[test]
fn read_transport_preserves_typed_conflict_without_exposing_other_backend_errors() {
    for (status, body, conflict) in [
        (
            409,
            r#"{"error":{"code":"source_authority_changed"}}"#,
            true,
        ),
        (500, "private backend details", false),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/declared", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse::<usize>().unwrap();
                }
            }
            reader.read_exact(&mut vec![0; length]).unwrap();
            write!(stream,"HTTP/1.1 {status} Refused\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        });
        let mut transport = HttpDeclaredTransport::new(
            &endpoint,
            "fictional-storage-token-only-32-characters".into(),
        )
        .unwrap();
        let error = transport
            .call_read(
                serde_json::json!({"operation":"conversation_timeline_page"}),
                &ReadControl {
                    cancellation: ReadCancellation::default(),
                    deadline: Instant::now() + Duration::from_secs(5),
                    vm_steps: None,
                },
            )
            .unwrap_err();
        server.join().unwrap();
        assert_eq!(
            error.is::<cwpdb_store::conversation::SourceAuthorityConflict>(),
            conflict
        );
        assert!(!error.to_string().contains("private backend details"));
    }
}
