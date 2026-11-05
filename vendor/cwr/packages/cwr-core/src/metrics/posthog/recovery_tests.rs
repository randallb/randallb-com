//! Real exporter/HTTP fixture evidence, never a hosted analytics destination.
use super::tests::{config, read_request_body, record};
use super::*;
use std::{io::Write, sync::mpsc};

#[test]
fn accepted_request_with_lost_response_retries_identical_uuid_and_body_after_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("outbox");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut config = config();
    config.host = format!("http://{}", listener.local_addr().unwrap());
    let mut db = store(&path).unwrap();
    enqueue(&mut db, &config, &[record("attempt_ended")]).unwrap();
    let rows = batch(&mut db, &config).unwrap();
    let uuid = rows[0].1["uuid"].clone();
    let (release, wait) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut first, _) = listener.accept().unwrap();
        first
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let accepted = read_request_body(&mut first);
        // Remote accepted the body but the caller times out before any reply.
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
        drop(first);
        let (mut retry, _) = listener.accept().unwrap();
        retry
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let repeated = read_request_body(&mut retry);
        write!(
            retry,
            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        assert_eq!(accepted, repeated);
        let body: Value = serde_json::from_slice(&accepted).unwrap();
        assert_eq!(body["batch"][0]["uuid"], uuid);
        println!(
            "accepted/lost-response/reopen retry retained identical UUID and {} body bytes",
            accepted.len()
        );
    });
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(100))
        .build()
        .unwrap();
    assert!(!upload(&mut db, &config, &client, &rows).unwrap());
    release.send(()).unwrap();
    drop(db);
    let mut db = store(&path).unwrap();
    assert_eq!(batch(&mut db, &config).unwrap(), rows);
    assert!(upload(&mut db, &config, &reqwest::blocking::Client::new(), &rows).unwrap());
    assert!(batch(&mut db, &config).unwrap().is_empty());
    server.join().unwrap();
}

#[test]
fn worker_exports_retained_legacy_queue_after_atomic_startup_conversion() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("outbox");
    std::fs::create_dir(&path).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut config = config();
    config.host = format!("http://{}", listener.local_addr().unwrap());
    let event=json!({"uuid":"00000000-0000-4000-8000-000000000099","event":"cwr_metric","timestamp":"2026-10-23T00:00:00Z","properties":{"synthetic":true}}).to_string();
    cwpdb_store::test_support::legacy_posthog_outbox(
        &path.join("queue.sqlite3"),
        &destination(&config),
        unix_ms(),
        &event,
    )
    .unwrap();
    let (accepted, done) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let body = read_request_body(&mut stream);
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        accepted.send(body).unwrap();
    });
    let exporter = Exporter::start(path.clone(), config.clone()).unwrap();
    let body: Value =
        serde_json::from_slice(&done.recv_timeout(Duration::from_secs(10)).unwrap()).unwrap();
    assert_eq!(
        body["batch"][0]["uuid"],
        "00000000-0000-4000-8000-000000000099"
    );
    drop(exporter);
    let mut reopened = store(&path).unwrap();
    assert!(batch(&mut reopened, &config).unwrap().is_empty());
    assert_eq!(reopened.installation_id().unwrap(), "retained-installation");
    assert_eq!(reopened.dropped().unwrap(), 7);
    server.join().unwrap();
    println!(
        "actual exporter worker migrated, delivered and atomically acknowledged retained event 99"
    );
}
