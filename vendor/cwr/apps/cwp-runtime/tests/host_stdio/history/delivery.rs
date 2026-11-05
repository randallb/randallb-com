use super::*;
use cwr::v2026_09::conversation::RetainedConversation;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

// A second real host process. Its reader runs in the consuming test thread;
// events are never buffered in an unbounded test-side channel.
struct Peer {
    _process: support::RuntimeProcess,
    input: ChildStdin,
    output: BufReader<std::process::ChildStdout>,
}

impl Peer {
    fn start(host: &HistoryHost) -> Self {
        let mut process = support::RuntimeProcess(
            Command::new(env!("CARGO_BIN_EXE_cwp-runtime"))
                .arg(host.directory.path().join("workspace/CWR.toml"))
                .args([
                    "--host-stdio",
                    "--ingress",
                    "cwp://example.test/integrations/delivery-peer",
                ])
                .env("WORKLOOP_CONFIGURATION", r#"{}"#)
                .env("XDG_STATE_HOME", host.directory.path().join("state-home"))
                .env(
                    "XDG_RUNTIME_DIR",
                    support::runtime_home(host.directory.path()),
                )
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(fs::File::create(host.directory.path().join("peer.log")).unwrap())
                .spawn()
                .unwrap(),
        );
        let input = process.0.stdin.take().unwrap();
        let output = BufReader::new(process.0.stdout.take().unwrap());
        let mut peer = Self {
            _process: process,
            input,
            output,
        };
        write(
            &mut peer.input,
            &HostCommand::SelectActor {
                version: HOST_COMMAND_VERSION,
                request_id: "select-peer".into(),
                actor_ref: crate::test_refs::actor_ref("root"),
            },
        );
        peer
    }

    fn receive(&mut self) -> HostEvent {
        let mut line = String::new();
        assert!(
            self.output.read_line(&mut line).unwrap() > 0,
            "peer disconnected"
        );
        assert!(line.ends_with('\n'), "partial host frame");
        let event: HostEvent = serde_json::from_str(&line).unwrap();
        event.validate().unwrap();
        event
    }

    fn snapshot(&mut self) -> Vec<RetainedConversation> {
        loop {
            if let HostEvent::ConversationSnapshot { conversations, .. } = self.receive() {
                return conversations;
            }
        }
    }
}

fn seed(host: &mut HistoryHost, bytes: usize) -> String {
    let events = host.command(HostCommand::CreateConversation {
        version: HOST_COMMAND_VERSION,
        request_id: "seed-delivery".into(),
        source_ref: SOURCE_REF.into(),
        author_actor_ref: crate::test_refs::actor_ref("root"),
        text: format!("@root {}", "h".repeat(bytes)),
    });
    receipt(&events).conversation_ref.clone()
}

fn has_latest(conversation: &RetainedConversation) -> bool {
    conversation
        .messages
        .iter()
        .any(|message| message.text.starts_with("visible update 299:"))
}

// This is a transport-isolation oracle, not a 30-second throughput benchmark.
// Only a strictly advancing retained update count renews the inactivity budget.
struct ProgressDeadline {
    highest: usize,
    deadline: Instant,
    inactivity: Duration,
}

impl ProgressDeadline {
    fn new(now: Instant, inactivity: Duration) -> Self {
        Self {
            highest: 0,
            deadline: now + inactivity,
            inactivity,
        }
    }

    fn remaining(&mut self, count: usize, now: Instant) -> Duration {
        if count > self.highest {
            self.highest = count;
            self.deadline = now + self.inactivity;
        }
        self.deadline.saturating_duration_since(now)
    }
}

fn wait_for_latest_progress(
    updates: &mpsc::Receiver<bool>,
    progress: &AtomicUsize,
    inactivity: Duration,
) -> Result<(), String> {
    let mut watchdog = ProgressDeadline::new(Instant::now(), inactivity);
    loop {
        let count = progress.load(Ordering::Relaxed);
        let remaining = watchdog.remaining(count, Instant::now());
        if remaining.is_zero() {
            return Err(format!(
                "no advancing peer progress for {inactivity:?}; delivered={count}/300"
            ));
        }
        match updates.recv_timeout(remaining.min(Duration::from_millis(100))) {
            Ok(false) if progress.load(Ordering::Relaxed) == 300 => return Ok(()),
            Ok(false) => return Err("latest notification before all 300 updates".into()),
            Ok(true) => return Err("terminal result before latest progress".into()),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("peer disconnected before latest progress".into())
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

#[test]
fn progress_inactivity_budget_requires_strictly_advancing_retained_count() {
    let now = Instant::now();
    let idle = Duration::from_secs(30);
    let mut watchdog = ProgressDeadline::new(now, idle);
    assert_eq!(watchdog.remaining(1, now + idle / 2), idle);
    // Duplicate and stale upserts cannot keep a stalled peer alive.
    assert_eq!(watchdog.remaining(1, now + idle), idle / 2);
    assert_eq!(watchdog.remaining(0, now + idle + idle / 2), Duration::ZERO);
    // A slow but advancing burst can exceed 30 seconds in total.
    let mut watchdog = ProgressDeadline::new(now, idle);
    for count in 1..=300 {
        assert_eq!(
            watchdog.remaining(count, now + idle / 2 * count as u32),
            idle
        );
    }
    let advanced_at = now + idle / 2 * 300;
    assert_eq!(watchdog.remaining(300, advanced_at + idle), Duration::ZERO);
}

#[test]
fn progress_watchdog_rejects_a_stalled_peer_and_early_completion() {
    let (sender, receiver) = mpsc::sync_channel(2);
    let progress = AtomicUsize::new(1);
    // Fault injection: a connected peer stops advancing. It must fail even if
    // the progress count is nonzero; retaining the sender prevents disconnect.
    assert!(
        wait_for_latest_progress(&receiver, &progress, Duration::from_millis(20))
            .unwrap_err()
            .contains("no advancing peer progress")
    );
    sender.send(true).unwrap();
    assert!(
        wait_for_latest_progress(&receiver, &progress, Duration::from_secs(30))
            .unwrap_err()
            .contains("terminal result")
    );
    drop(sender);
    assert!(
        wait_for_latest_progress(&receiver, &progress, Duration::from_secs(30))
            .unwrap_err()
            .contains("disconnected")
    );
}

#[test]
fn paused_host_gets_latest_history_and_terminal_results_while_a_peer_stays_healthy() {
    for stop in [false, true] {
        let case_started = Instant::now();
        let mut host = HistoryHost::start_with_adapter(include_str!("delivery_adapter.py"));
        let conversation_ref = seed(&mut host, 384 * 1024);
        let started = host.command(HostCommand::ContinueConversation {
            version: HOST_COMMAND_VERSION,
            request_id: "start-delivery".into(),
            source_ref: SOURCE_REF.into(),
            author_actor_ref: crate::test_refs::actor_ref("root"),
            conversation_ref: conversation_ref.clone(),
            text: "@sh Start updates".into(),
            recovery_only: false,
        });
        let invocation_ref = receipt(&started).invocation_ref.clone().unwrap();
        host.wait_for("ready");
        host.barrier(&conversation_ref, 1);
        let mut peer = Peer::start(&host);
        assert_eq!(peer.snapshot().len(), 1);
        let (healthy, updates) = mpsc::sync_channel(2);
        let delivered_progress = Arc::new(AtomicUsize::new(0));
        let peer_progress = delivered_progress.clone();
        let peer_reader = thread::spawn(move || {
            let mut seen_latest = false;
            loop {
                match peer.receive() {
                    HostEvent::ConversationUpsert { conversation, .. } => {
                        peer_progress.store(
                            conversation
                                .messages
                                .iter()
                                .filter(|message| message.text.starts_with("visible update "))
                                .count(),
                            Ordering::Relaxed,
                        );
                        if has_latest(&conversation) && !seen_latest {
                            seen_latest = true;
                            healthy.send(false).unwrap();
                        }
                    }
                    HostEvent::ConversationExecutionFinished { response, .. } => {
                        assert!(seen_latest);
                        assert_eq!(response.status.is_stopped(), stop);
                        assert_eq!(response.status.is_completed(), !stop);
                        healthy.send(true).unwrap();
                        return peer;
                    }
                    _ => {}
                }
            }
        });
        // Build bulk history while both hosts consume it. Only the final ten
        // growing upserts belong to the genuinely paused interval: arbitrarily
        // slow fixture construction must not cross the 30-second socket policy.
        host.release("warmup");
        loop {
            match host.receive() {
                HostEvent::ConversationUpsert { conversation, .. }
                    if conversation
                        .messages
                        .iter()
                        .any(|message| message.text.starts_with("visible update 289:")) =>
                {
                    break
                }
                event => assert!(
                    !matches!(event, HostEvent::CommandFailed { .. }),
                    "{event:?}"
                ),
            }
        }
        host.wait_for("warmup-done");
        host.barrier(&conversation_ref, 2);
        // No first-host reads occur from this ordered barrier through the peer's
        // terminal result. The 384 KiB seed makes each update exceed pipe space.
        let burst_started = Instant::now();
        host.release("burst");
        wait_for_latest_progress(&updates, &delivered_progress, Duration::from_secs(30))
            .unwrap_or_else(|error| {
                panic!(
                    "healthy peer progress deadline: {error}; stop={stop}, adapter_barrier_acknowledged={}",
                    host.directory.path().join("burst-done").exists(),
                )
            });
        eprintln!(
            "healthy peer received final 10 of 300 updates in {:?}; stop={stop}",
            burst_started.elapsed()
        );
        host.wait_for("burst-done");
        // The host pipe has not been drained during the final growing upserts.
        // Stay paused past the old five-second acknowledgement timeout.
        thread::sleep(Duration::from_secs(6));
        if stop {
            // The paused client's input must still reach the runtime. Observe
            // Stop through the healthy peer before resuming the paused reader.
            write(
                &mut host.input,
                &HostCommand::StopConversation {
                    version: HOST_COMMAND_VERSION,
                    request_id: "stop-paused".into(),
                    source_ref: SOURCE_REF.into(),
                    conversation_ref: conversation_ref.clone(),
                    requesting_actor_ref: crate::test_refs::actor_ref("root"),
                    invocation_ref: Some(invocation_ref.clone()),
                },
            );
        } else {
            host.release("finish");
        }
        assert!(updates
            .recv_timeout(Duration::from_secs(10))
            .expect("healthy peer receives terminal result while first host remains paused"));
        let _peer = peer_reader.join().unwrap();
        let mut received_upserts = 0;
        let mut latest = None;
        let mut got_terminal = false;
        let mut got_stop_receipt = !stop;
        while !got_terminal || !got_stop_receipt {
            match host.receive() {
                HostEvent::ConversationUpsert { conversation, .. } => {
                    received_upserts += 1;
                    latest = Some(conversation);
                }
                HostEvent::ConversationExecutionFinished { response, .. } => {
                    assert_eq!(response.invocation_ref, invocation_ref);
                    assert_eq!(response.status.is_stopped(), stop);
                    assert!(latest.as_ref().is_some_and(has_latest));
                    got_terminal = true;
                }
                HostEvent::CommandSucceeded {
                    request_id,
                    receipt,
                    ..
                } if request_id == "stop-paused" => {
                    assert_eq!(
                        receipt.stopped_invocation_refs,
                        vec![invocation_ref.clone()]
                    );
                    got_stop_receipt = true;
                }
                event => assert!(
                    !matches!(event, HostEvent::CommandFailed { .. }),
                    "{event:?}"
                ),
            }
        }
        assert!(
            received_upserts < 10,
            "obsolete upserts retained: {received_upserts}"
        );
        assert_eq!(
            latest
                .unwrap()
                .messages
                .iter()
                .filter(|message| message.text.starts_with("visible update "))
                .count(),
            300
        );
        host.barrier(&conversation_ref, 3);
        assert!(host.process.0.try_wait().unwrap().is_none());
        eprintln!("paused host: stop={stop}, 300 updates retained, {received_upserts} upserts delivered after pause >6s; peer completed before resume; total={:?}", case_started.elapsed());
    }
}

#[test]
fn reconnecting_host_receives_a_complete_snapshot_larger_than_the_queue_budget() {
    let mut host = HistoryHost::start();
    let conversation_ref = seed(&mut host, 700 * 1024);
    // Receipts precede history publication. Drain through a small ordered
    // barrier so the next full-history receipt cannot overlap that upsert.
    host.barrier(&conversation_ref, 1);
    for index in 1..48 {
        host.command(HostCommand::ContinueConversation {
            version: HOST_COMMAND_VERSION,
            request_id: format!("seed-{index}"),
            source_ref: SOURCE_REF.into(),
            author_actor_ref: crate::test_refs::actor_ref("root"),
            conversation_ref: conversation_ref.clone(),
            text: format!("@root {index}: {}", "h".repeat(700 * 1024)),
            recovery_only: false,
        });
        host.barrier(&conversation_ref, index + 1);
    }
    let mut peer = Peer::start(&host);
    // Pause this reconnecting host while its oversized snapshot fills the pipe.
    thread::sleep(Duration::from_secs(6));
    let conversations = peer.snapshot();
    let bytes = serde_json::to_vec(&conversations).unwrap().len();
    assert!(bytes > 32 * 1024 * 1024);
    assert_eq!(conversations[0].conversation_ref, conversation_ref);
    assert_eq!(conversations[0].messages.len(), 48);
    assert!(conversations[0]
        .messages
        .last()
        .unwrap()
        .text
        .starts_with("47: "));
    eprintln!("reconnecting real host received {bytes}-byte snapshot with all 48 messages");
}
