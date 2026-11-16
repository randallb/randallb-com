use super::super::MetricContext;
use super::*;
use std::io::{Read, Write};
pub(super) fn config() -> Config {
    Config {
        token: "test-token".into(),
        host: "https://us.i.posthog.com".into(),
        channel: "canary".into(),
        build: "abc123".into(),
    }
}
pub(super) fn record(checkpoint: &'static str) -> MetricRecord {
    MetricRecord {
        schema_version: 2,
        recorded_at_unix_ms: 1_700_000_000_123,
        process_id: 1,
        app_version: "test",
        context: MetricContext {
            runtime_ref: "cwp://runtime/test".into(),
            invocation_ref: "cwp://invocation/test".into(),
            conversation_ref: None,
            parent_invocation_ref: None,
            attempt: 1,
            model_run_ref: None,
            actor_ref: "private-actor".into(),
            model: None,
            reasoning_effort: None,
            invocation_continuation_supplied: false,
            private_history_supplied: false,
            context_bytes: 0,
        },
        producer: "runtime",
        checkpoint,
        measurements: json!({"exit_code":1}),
        dropped_records_total: 0,
    }
}
#[test]
fn durable_identity_destination_and_exception() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("outbox");
    let config = config();
    let mut db = store(&path).unwrap();
    enqueue(&mut db, &config, &[record("adapter_process_failed")]).unwrap();
    let before = batch(&mut db, &config).unwrap();
    assert_eq!(before[0].1["event"], "$exception");
    assert_eq!(before[0].1["timestamp"], "2023-11-14T22:13:20.123Z");
    assert!(before[0].1["properties"].get("actor_ref").is_none());
    assert_eq!(before[0].1["properties"]["release_channel"], "canary");
    drop(db);
    let mut db = store(&path).unwrap();
    assert_eq!(batch(&mut db, &config).unwrap(), before);
    let other = Config {
        token: "another-project".into(),
        ..config
    };
    assert!(batch(&mut db, &other).unwrap().is_empty());
}
pub(super) fn read_request_body(stream: &mut std::net::TcpStream) -> Vec<u8> {
    let mut bytes = Vec::new();
    loop {
        let mut chunk = [0; 4096];
        let n = stream.read(&mut chunk).unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&chunk[..n]);
        let Some(end) = bytes.windows(4).position(|s| s == b"\r\n\r\n") else {
            continue;
        };
        let headers = String::from_utf8_lossy(&bytes[..end]);
        let length: usize = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(|s| s.trim().parse().unwrap())
            })
            .unwrap();
        if bytes.len() >= end + 4 + length {
            return bytes[end + 4..].to_vec();
        }
    }
}
#[test]
fn failure_retains_identical_batch_and_success_acknowledges() {
    let temp = tempfile::tempdir().unwrap();
    let mut db = store(&temp.path().join("outbox")).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut config = config();
    config.host = format!("http://{}", listener.local_addr().unwrap());
    enqueue(&mut db, &config, &[record("attempt_ended")]).unwrap();
    let rows = batch(&mut db, &config).unwrap();
    let server = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for status in ["503 Unavailable", "200 OK", "422 Unprocessable Entity"] {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            requests.push(read_request_body(&mut stream));
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
        }
        assert_eq!(requests[0], requests[1]);
    });
    let client = reqwest::blocking::Client::new();
    assert!(!upload(&mut db, &config, &client, &rows).unwrap());
    assert_eq!(batch(&mut db, &config).unwrap(), rows);
    assert!(upload(&mut db, &config, &client, &rows).unwrap());
    assert!(batch(&mut db, &config).unwrap().is_empty());
    enqueue(&mut db, &config, &[record("attempt_ended")]).unwrap();
    let invalid = batch(&mut db, &config).unwrap();
    assert!(upload(&mut db, &config, &client, &invalid).unwrap());
    assert!(batch(&mut db, &config).unwrap().is_empty());
    assert_eq!(db.accounting().unwrap().rows, 1);
    server.join().unwrap();
}
#[test]
fn export_requires_prerelease_and_valid_destination() {
    for (enabled, channel, host, accepted) in [
        ("1", "canary", "https://us.i.posthog.com", true),
        ("1", "beta", "https://eu.i.posthog.com", true),
        ("0", "canary", "https://us.i.posthog.com", false),
        ("1", "stable", "https://us.i.posthog.com", false),
        ("1", "production", "https://us.i.posthog.com", false),
        ("1", "canary", "http://us.i.posthog.com", false),
    ] {
        let config = Config::from_values(|key| {
            Some(
                match key {
                    "CWR_POSTHOG_ENABLED" => enabled,
                    "WORKLOOP_RELEASE_CHANNEL" => channel,
                    "CWR_POSTHOG_HOST" => host,
                    "CWR_POSTHOG_TOKEN" => "test-token",
                    "WORKLOOP_BUILD_SHA" => "abc123",
                    _ => return None,
                }
                .into(),
            )
        });
        assert_eq!(config.is_some(), accepted);
    }
    let shared = |key: &str| match key {
        "WORKLOOP_RELEASE_CHANNEL" => Some("canary".into()),
        "POSTHOG_PROJECT_TOKEN" => Some("shared-token".into()),
        "POSTHOG_HOST" => Some("https://posthog.getworkloop.com/".into()),
        _ => None,
    };
    let config = Config::from_values(shared).unwrap();
    assert_eq!(config.host, "https://posthog.getworkloop.com");
    assert_eq!(config.token, "shared-token");
    assert_eq!(config.build, "unknown");
    for invalid in [
        "http://example.com",
        "https://user:pass@example.com",
        "https://example.com/?token=x",
        "https://example.com/#fragment",
    ] {
        assert!(Config::from_values(|key| if key == "POSTHOG_HOST" {
            Some(invalid.into())
        } else {
            shared(key)
        })
        .is_none());
    }
    assert!(Config::from_values(|key| if key == "CWR_POSTHOG_ENABLED" {
        Some("0".into())
    } else {
        shared(key)
    })
    .is_none());
    assert!(Config::from_values(|_| None).is_none());
}
#[test]
fn prerelease_uses_workloop_destination_without_environment_setup() {
    for channel in ["canary", "alpha", "beta", "delta"] {
        let config =
            Config::from_values(|key| (key == "WORKLOOP_RELEASE_CHANNEL").then(|| channel.into()))
                .unwrap();
        assert_eq!(config.host, "https://b.getworkloop.com");
        assert!(config.token.starts_with("phc_"));
    }
}
#[test]
fn payload_budget_evicts_oldest_and_batch_stays_under_request_cap() {
    let temp = tempfile::tempdir().unwrap();
    let mut db = store(&temp.path().join("outbox")).unwrap();
    let config = config();
    let mut sample = record("attempt_ended");
    sample.app_version = Box::leak("界".repeat(20_000).into_boxed_str());
    enqueue(&mut db, &config, &vec![sample; 160]).unwrap();
    assert!(db.accounting().unwrap().bytes <= 8 * 1024 * 1024);
    assert!(db.dropped().unwrap() > 0);
    let rows = batch(&mut db, &config).unwrap();
    let body =
        json!({"token":config.token,"batch": rows.iter().map(|(_, v)|v).collect::<Vec<_>>()})
            .to_string();
    assert!(body.len() < MAX_BODY);
}
#[cfg(unix)]
#[test]
fn rejects_symlinked_store() {
    let temp = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(temp.path(), temp.path().join("link")).unwrap();
    assert!(store(&temp.path().join("link")).is_err());
}

#[test]
fn export_omits_hostname_and_hashes_linked_references() {
    let mut sample = record("attempt_ended");
    sample.context.runtime_ref = "cwp://local.Private-Mac/runtimes/private".into();
    sample.context.invocation_ref = "cwp://local.Private-Mac/invocations/private".into();
    sample.context.model = Some("private-model".into());
    sample.measurements = json!({"source_message_ref":"cwp://local.Private-Mac/messages/private", "secret":"Private-Mac", "elapsed_ms":42});
    let safe = export_properties(&sample, "random-analytics-id");
    let text = safe.to_string();
    assert!(!text.contains("Private-Mac"));
    assert!(!text.contains("private-model"));
    assert!(!text.contains("cwp://"));
    assert_eq!(safe["measurements"]["elapsed_ms"], 42);
    assert_eq!(safe, export_properties(&sample, "random-analytics-id"));
    assert_ne!(
        safe["runtime_ref"],
        export_properties(&sample, "other-id")["runtime_ref"]
    );
}

#[test]
fn expiry_counts_dropped_records_atomically() {
    let temp = tempfile::tempdir().unwrap();
    let mut db = store(&temp.path().join("outbox")).unwrap();
    enqueue(&mut db, &config(), &[record("attempt_ended")]).unwrap();
    let now = unix_ms() + MAX_AGE_MS + 1;
    expire(&mut db, now).unwrap();
    assert!(batch(&mut db, &config()).unwrap().is_empty());
    assert_eq!(db.dropped().unwrap(), 1);
    expire(&mut db, now).unwrap();
    assert_eq!(db.dropped().unwrap(), 1);
}

#[test]
fn shutdown_drains_records_into_outbox() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("outbox");
    let mut config = config();
    config.host = "http://127.0.0.1:1".into();
    let exporter = Exporter::start(path.clone(), config.clone()).unwrap();
    exporter.record(&[record("attempt_ended")]);
    drop(exporter);
    let mut db = store(&path).unwrap();
    assert_eq!(batch(&mut db, &config).unwrap().len(), 1);
}

#[test]
fn preserves_declared_runtime_categories_without_arbitrary_strings() {
    use crate::v2026_09::invocation::ActionInvocationState;
    for state in [
        ActionInvocationState::Admitted,
        ActionInvocationState::AwaitingPerson,
        ActionInvocationState::Running,
        ActionInvocationState::Waiting,
        ActionInvocationState::Completed,
        ActionInvocationState::Blocked,
        ActionInvocationState::Failed,
        ActionInvocationState::Stopping,
        ActionInvocationState::Stopped,
    ] {
        let mut sample = record("slow_invocation");
        sample.measurements = json!({"state":state, "outcome":state});
        assert_eq!(
            export_properties(&sample, "id")["measurements"],
            sample.measurements
        );
    }
    for stage in [
        "route_selection",
        "action_only_message",
        "lifecycle_operations",
        "invocation_retention",
        "invocation_started_observer",
        "adapter_readiness",
    ] {
        let mut sample = record("runtime_setup");
        sample.measurements = json!({"stage":stage, "duration_ms":4});
        assert_eq!(
            export_properties(&sample, "id")["measurements"],
            sample.measurements
        );
    }
    let mut sample = record("attempt_ended");
    sample.measurements =
        json!({"outcome":"provider_unavailable", "stage":"private-path", "stdout_state":"eof"});
    assert_eq!(
        export_properties(&sample, "id")["measurements"],
        json!({"outcome":"provider_unavailable", "stdout_state":"eof"})
    );
}

#[test]
fn every_export_setting_changes_resident_fingerprint() {
    let baseline = fingerprint_from_values(|_| None);
    for field in [
        "WORKLOOP_RELEASE_CHANNEL",
        "CWR_POSTHOG_ENABLED",
        "CWR_POSTHOG_HOST",
        "CWR_POSTHOG_TOKEN",
        "POSTHOG_HOST",
        "POSTHOG_PROJECT_TOKEN",
        "WORKLOOP_BUILD_SHA",
    ] {
        let changed = fingerprint_from_values(|key| (key == field).then(|| "changed".into()));
        assert_ne!(baseline, changed, "{field}");
        assert!(!changed.contains("changed"));
    }
}

#[test]
fn bundled_destination_changes_resident_fingerprint() {
    let baseline = fingerprint_with_defaults(|_| None, BUNDLED_DEFAULTS);
    for field in ["host", "projectToken"] {
        let mut defaults: Value = serde_json::from_str(BUNDLED_DEFAULTS).unwrap();
        defaults[field] = json!("changed");
        let changed = fingerprint_with_defaults(|_| None, &defaults.to_string());
        assert_ne!(baseline, changed, "{field}");
        assert!(!changed.contains("changed"));
    }
}
