//! Default-on prerelease export. Network I/O has its own thread and disposable store.
use super::{unix_ms, MetricRecord};
mod properties;
use crate::{
    entities::posthog::{PendingEvent, PosthogOutbox},
    CwrResult,
};
use properties::export_properties;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant},
};

const BUNDLED_DEFAULTS: &str = include_str!("../../../configuration/workloop_analytics.json");

const MAX_EVENTS: usize = 250;
const MAX_BODY: usize = 1_000_000;
#[cfg(test)]
const MAX_AGE_MS: u64 = 14 * 24 * 60 * 60 * 1000;
const FLUSH_INTERVAL: Duration = Duration::from_secs(300);

/// Fingerprint of export configuration for resident-process environment reconciliation.
/// Tokens and destination overrides are hashed rather than retained in marker files.
pub fn environment_fingerprint() -> String {
    fingerprint_from_values(|key| std::env::var(key).ok())
}
fn fingerprint_from_values(value: impl Fn(&str) -> Option<String>) -> String {
    fingerprint_with_defaults(value, BUNDLED_DEFAULTS)
}
fn fingerprint_with_defaults(value: impl Fn(&str) -> Option<String>, defaults: &str) -> String {
    use sha2::{Digest, Sha256};
    let values: Vec<_> = [
        "WORKLOOP_RELEASE_CHANNEL",
        "CWR_POSTHOG_ENABLED",
        "CWR_POSTHOG_HOST",
        "CWR_POSTHOG_TOKEN",
        "POSTHOG_HOST",
        "POSTHOG_PROJECT_TOKEN",
        "WORKLOOP_BUILD_SHA",
    ]
    .iter()
    .map(|key| (*key, value(key)))
    .collect();
    hex::encode(Sha256::digest(
        serde_json::to_vec(&(values, defaults)).expect("string configuration"),
    ))
}

#[derive(Clone)]
struct Config {
    token: String,
    host: String,
    channel: String,
    build: String,
}
impl Config {
    fn from_environment() -> Option<Self> {
        Self::from_values(|key| std::env::var(key).ok())
    }
    fn from_values(value: impl Fn(&str) -> Option<String>) -> Option<Self> {
        if value("CWR_POSTHOG_ENABLED").as_deref() == Some("0") {
            return None;
        }
        let channel = value("WORKLOOP_RELEASE_CHANNEL")?;
        if !matches!(channel.as_str(), "canary" | "alpha" | "beta" | "delta") {
            return None;
        }
        // The ingestion token is public and write-only, shared with the bundled JS SDK.
        let defaults: Value = serde_json::from_str(BUNDLED_DEFAULTS).ok()?;
        let token = value("CWR_POSTHOG_TOKEN")
            .or_else(|| value("POSTHOG_PROJECT_TOKEN"))
            .unwrap_or_else(|| defaults["projectToken"].as_str().unwrap().into());
        let build = value("WORKLOOP_BUILD_SHA").unwrap_or_else(|| "unknown".into());
        let host = value("CWR_POSTHOG_HOST")
            .or_else(|| value("POSTHOG_HOST"))
            .unwrap_or_else(|| defaults["host"].as_str().unwrap().into());
        let host = if host.contains("://") {
            host
        } else {
            format!("https://{host}")
        };
        let url = reqwest::Url::parse(&host).ok()?;
        let host = host.trim_end_matches('/').to_owned();
        // Use the configured ingestion host (including Workloop proxies). Redirects stay disabled.
        if token.is_empty()
            || token.len() > 512
            || build.is_empty()
            || build.len() > 128
            || url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return None;
        }
        Some(Self {
            token,
            host,
            channel,
            build,
        })
    }
}

pub(super) struct Exporter {
    sender: Option<mpsc::SyncSender<Vec<MetricRecord>>>,
    finished: mpsc::Receiver<()>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Exporter {
    pub fn from_environment(database: &Path) -> Option<Self> {
        Self::start(
            database.parent()?.join("posthog-outbox"),
            Config::from_environment()?,
        )
    }
    fn start(path: PathBuf, config: Config) -> Option<Self> {
        let (sender, receiver) = mpsc::sync_channel::<Vec<MetricRecord>>(8);
        let (done, finished) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("cwr-posthog".into())
            .spawn(move || {
                if let Err(error) = run(&path, &config, receiver) {
                    // Deliberately omit server responses, request data and credentials.
                    let _ = error;
                    eprintln!("cwr: PostHog exporter unavailable (storage)");
                }
                let _ = done.send(());
            })
            .ok()?;
        Some(Self {
            sender: Some(sender),
            finished,
            worker: Some(worker),
        })
    }
    pub fn record(&self, records: &[MetricRecord]) {
        // Export loss must never block local collection or invocation execution.
        for chunk in records.chunks(MAX_EVENTS) {
            if self
                .sender
                .as_ref()
                .expect("exporter is open")
                .try_send(chunk.to_vec())
                .is_err()
            {
                eprintln!(
                    "cwr: PostHog export queue unavailable; {} samples dropped",
                    chunk.len()
                );
            }
        }
    }
}

impl Drop for Exporter {
    fn drop(&mut self) {
        drop(self.sender.take());
        // Keep the outer one-second metrics shutdown budget. A stalled upload may
        // outlive this wait, but ordinary shutdown drains the outbox before completion.
        if self
            .finished
            .recv_timeout(Duration::from_millis(750))
            .is_ok()
        {
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        } else {
            eprintln!("cwr: PostHog shutdown timed out; queued records may be lost");
        }
    }
}

fn store(path: &Path) -> CwrResult<PosthogOutbox> {
    PosthogOutbox::open(path)
}
fn enqueue(db: &mut PosthogOutbox, config: &Config, records: &[MetricRecord]) -> CwrResult<()> {
    let identity = db.installation_id()?;
    let mut events = Vec::with_capacity(records.len());
    for record in records {
        let timestamp = db.timestamp(record.recorded_at_unix_ms)?;
        let mut properties = export_properties(record, &identity);
        properties["$process_person_profile"] = json!(false);
        properties["$geoip_disable"] = json!(true);
        properties["release_channel"] = json!(config.channel);
        properties["platform"] = json!("desktop");
        properties["build_sha"] = json!(config.build);
        let event_name = if record.checkpoint == "adapter_process_failed" {
            properties["$exception_list"] = json!([{"type":"CwrAdapterProcessFailure","value":"Adapter exited before returning a protocol result","mechanism":{"handled":true,"synthetic":true}}]);
            "$exception"
        } else {
            "cwr_metric"
        };
        events.push(json!({"uuid":uuid::Uuid::new_v4().to_string(),"event":event_name,"timestamp":timestamp,"properties":properties}));
    }
    db.enqueue(&destination(config), unix_ms(), &events)
}
fn expire(db: &mut PosthogOutbox, now: u64) -> CwrResult<()> {
    db.expire(now)?;
    Ok(())
}
fn destination(config: &Config) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(format!("{}\n{}", config.host, config.token)))
}
fn batch(db: &mut PosthogOutbox, config: &Config) -> CwrResult<Vec<PendingEvent>> {
    let overhead = serde_json::to_vec(&json!({"token":config.token,"batch":[]}))?.len();
    db.pending(&destination(config), overhead)
}
fn upload(
    db: &mut PosthogOutbox,
    config: &Config,
    client: &reqwest::blocking::Client,
    rows: &[PendingEvent],
) -> CwrResult<bool> {
    if rows.is_empty() {
        return Ok(true);
    }
    let body =
        json!({"token":config.token,"batch":rows.iter().map(|(_,event)|event).collect::<Vec<_>>()});
    if serde_json::to_vec(&body)?.len() > MAX_BODY {
        return Err("PostHog body limit exceeded".into());
    }
    let status = client
        .post(format!("{}/batch/", config.host))
        .header(reqwest::header::USER_AGENT, "Workloop-CWR/metrics")
        .json(&body)
        .send()
        .map(|r| r.status());
    match status {
        Ok(status) if status.is_success() => {
            db.acknowledge(
                &rows
                    .iter()
                    .map(|(selected, _)| selected.clone())
                    .collect::<Vec<_>>(),
            )?;
            Ok(true)
        }
        Ok(status) if matches!(status.as_u16(), 400 | 413 | 422) => {
            if rows.len() == 1 {
                db.quarantine(&[rows[0].0.clone()])?;
                Ok(true)
            } else {
                let middle = rows.len() / 2;
                Ok(upload(db, config, client, &rows[..middle])?
                    && upload(db, config, client, &rows[middle..])?)
            }
        }
        _ => Ok(false),
    }
}
fn run(path: &Path, config: &Config, receiver: mpsc::Receiver<Vec<MetricRecord>>) -> CwrResult<()> {
    let mut db = store(path)?;
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("static HTTP client configuration");
    let mut next = if batch(&mut db, config)?.is_empty() {
        Instant::now() + FLUSH_INTERVAL
    } else {
        Instant::now()
    }; // Retry persisted events on restart.
    let mut retry = 1u64;
    loop {
        let message = receiver.recv_timeout(Duration::from_secs(1));
        let disconnected = matches!(message, Err(mpsc::RecvTimeoutError::Disconnected));
        if let Ok(records) = message {
            enqueue(&mut db, config, &records)?;
        }
        expire(&mut db, unix_ms())?;
        let rows = batch(&mut db, config)?;
        if (Instant::now() >= next || (retry == 1 && rows.len() == MAX_EVENTS) || disconnected)
            && !rows.is_empty()
        {
            if upload(&mut db, config, &client, &rows)? {
                retry = 1;
                next = Instant::now() + FLUSH_INTERVAL;
            } else {
                let jitter = u64::from(uuid::Uuid::new_v4().as_bytes()[0]) % retry.max(1);
                next = Instant::now() + Duration::from_secs(retry + jitter);
                retry = (retry * 2).min(300);
            }
        }
        if disconnected {
            break;
        }
    }
    Ok(())
}

/// Attempt all retained batches using the configured prerelease destination.
/// This blocking diagnostic belongs on a command/worker, never the turn path.
pub fn flush_pending(state_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    flush_pending_inner(state_path).map_err(|error| -> Box<dyn std::error::Error> { error })
}
fn flush_pending_inner(state_path: &Path) -> CwrResult<()> {
    let config = Config::from_environment().ok_or("PostHog prerelease export is not configured")?;
    let path: PathBuf = state_path
        .parent()
        .ok_or("state path has no parent")?
        .join("posthog-outbox");
    let mut db = store(&path)?;
    expire(&mut db, unix_ms())?;
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    loop {
        let rows = batch(&mut db, &config)?;
        if rows.is_empty() {
            break;
        }
        if !upload(&mut db, &config, &client, &rows)? {
            return Err("PostHog upload failed; events retained".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod recovery_tests;
#[cfg(test)]
mod tests;
