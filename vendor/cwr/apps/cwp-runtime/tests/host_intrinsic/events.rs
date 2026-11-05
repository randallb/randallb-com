use super::*;
use std::{cell::RefCell, collections::VecDeque};

pub(super) struct HostEvents {
    receiver: Receiver<String>,
    pending: RefCell<VecDeque<HostEvent>>,
}

impl HostEvents {
    pub(super) fn new(receiver: Receiver<String>) -> Self {
        Self {
            receiver,
            pending: RefCell::new(VecDeque::new()),
        }
    }

    fn read(&self) -> HostEvent {
        let line = self
            .receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("bounded runtime response");
        let event: HostEvent = serde_json::from_str(&line).expect("typed host event");
        event.validate().expect("valid host event");
        event
    }

    pub(super) fn receive(&self) -> HostEvent {
        self.pending
            .borrow_mut()
            .pop_front()
            .unwrap_or_else(|| self.read())
    }

    pub(super) fn receipt(&self, expected: &str) -> HostEvent {
        let position = self
            .pending
            .borrow()
            .iter()
            .position(|event| is_receipt(event, expected));
        if let Some(position) = position {
            return self.pending.borrow_mut().remove(position).unwrap();
        }
        loop {
            let event = self.read();
            if is_receipt(&event, expected) {
                return event;
            }
            // Work, validation, and publication can precede a command receipt.
            self.pending.borrow_mut().push_back(event);
        }
    }
}

fn is_receipt(event: &HostEvent, expected: &str) -> bool {
    match event {
        HostEvent::UpstreamRegistered { request_id, .. }
        | HostEvent::WorkspaceInspected { request_id, .. }
        | HostEvent::ActorSelected { request_id, .. }
        | HostEvent::IntrinsicResponse { request_id, .. }
        | HostEvent::CommandSucceeded { request_id, .. }
        | HostEvent::CommandFailed { request_id, .. } => request_id == expected,
        _ => false,
    }
}

#[test]
fn receipt_wait_preserves_earlier_upstream_events_in_order() {
    use cwr::v2026_09::upstream::{UpstreamCoworker, UpstreamEvent, UpstreamRoute, UpstreamWork};
    let (sender, receiver) = mpsc::channel();
    let events = HostEvents::new(receiver);
    let before = [
        HostEvent::Upstream {
            event: UpstreamEvent::WorkRequested {
                work: Box::new(UpstreamWork {
                    invocation_ref: "cwp://example.test/invocations/work".into(),
                    root_invocation_ref: "cwp://example.test/invocations/work".into(),
                    conversation_ref: "cwp://example.test/conversations/one".into(),
                    requester_actor_ref: "cwp://example.test/actors/person".into(),
                    route: UpstreamRoute {
                        provider_session_ref: "cwp://example.test/providers/one".into(),
                        gateway_resource_ref: None,
                        coworker: UpstreamCoworker {
                            actor_ref: "cwp://example.test/actors/helper".into(),
                            coworker_ref: "cwp://example.test/coworkers/helper".into(),
                            handle: "@helper".into(),
                            name: "Helper".into(),
                            actions: vec![],
                        },
                    },
                    action_name: "new--conversation".into(),
                    input: serde_json::json!({}),
                    addressed_message: None,
                }),
            },
        },
        HostEvent::Upstream {
            event: UpstreamEvent::StopRequested {
                invocation_ref: "cwp://example.test/invocations/work".into(),
            },
        },
        HostEvent::Upstream {
            event: UpstreamEvent::ValidateResult {
                invocation_ref: "cwp://example.test/invocations/work".into(),
                validation_ref: "cwp://example.test/validations/result".into(),
            },
        },
    ];
    for event in &before {
        sender.send(serde_json::to_string(event).unwrap()).unwrap();
    }
    sender
        .send(
            serde_json::to_string(&HostEvent::CommandFailed {
                request_id: "admission".into(),
                error: "fixture receipt".into(),
            })
            .unwrap(),
        )
        .unwrap();
    drop(sender);
    assert!(matches!(
        events.receipt("admission"),
        HostEvent::CommandFailed { .. }
    ));
    for expected in before {
        assert_eq!(
            serde_json::to_value(events.receive()).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
    }
}

#[test]
fn receipt_wait_can_consume_an_already_buffered_receipt() {
    let (sender, receiver) = mpsc::channel();
    let events = HostEvents::new(receiver);
    for request_id in ["second", "first"] {
        sender
            .send(
                serde_json::to_string(&HostEvent::CommandFailed {
                    request_id: request_id.into(),
                    error: "fixture receipt".into(),
                })
                .unwrap(),
            )
            .unwrap();
    }
    drop(sender);
    assert!(is_receipt(&events.receipt("first"), "first"));
    assert!(is_receipt(&events.receipt("second"), "second"));
}
