use super::*;
use crate::NativeStore;
use serde_json::json;
struct Allow;
impl Authorizer for Allow {
    fn authorize(&self, _: &Context, _: &str, _: bool) -> CwpdbResult<()> {
        Ok(())
    }
}
struct Deny;
impl Authorizer for Deny {
    fn authorize(&self, _: &Context, _: &str, _: bool) -> CwpdbResult<()> {
        Err("access denied".into())
    }
}
fn context() -> Context {
    Context {
        workspace_ref: "cwp://test/workspaces/one".into(),
        caller_ref: "cwp://test/actors/coordinator".into(),
    }
}
fn request(request_id: &str, expected_revision: Option<String>, operation: Operation) -> Mutation {
    Mutation {
        context: context(),
        invocation_ref: "cwp://test/invocations/one".into(),
        request_id: request_id.into(),
        expected_revision,
        operation,
    }
}
fn admit() -> Mutation {
    request(
        "admit",
        None,
        Operation::Admit {
            conversation_ref: "cwp://test/conversations/one".into(),
            parent_invocation_ref: None,
            admitted: json!({"action":"respond"}),
        },
    )
}
fn committed(outcome: Outcome) -> Receipt {
    match outcome {
        Outcome::Committed(receipt) => receipt,
        _ => panic!("unexpected expired receipt"),
    }
}
fn assign(store: &mut NativeStore, revision: String) -> Receipt {
    committed(
        store
            .mutate(
                &request(
                    "assign",
                    Some(revision),
                    Operation::Assign {
                        assignment: Assignment {
                            executor_ref: "cwp://test/executors/one".into(),
                            assignment_ref: "cwp://test/assignments/one".into(),
                            generation: 1,
                            route_fingerprint: "route-one".into(),
                            expires_at_unix_ms: 1000,
                        },
                    },
                ),
                &Allow,
                10,
            )
            .unwrap(),
    )
}
fn append(revision: String, sequence: u64, kind: EventKind) -> Mutation {
    request(
        &format!("event-{sequence}"),
        Some(revision),
        Operation::Append {
            assignment_ref: "cwp://test/assignments/one".into(),
            executor_ref: "cwp://test/executors/one".into(),
            generation: 1,
            event: Event {
                sequence,
                kind,
                payload: json!({"text":"useful result"}),
            },
        },
    )
}
#[test]
fn admission_assignment_events_reopen_and_replay() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("store.db");
    let mut store = NativeStore::open(&path).unwrap();
    let first = committed(store.mutate(&admit(), &Allow, 10).unwrap());
    assert_eq!(
        store.mutate(&admit(), &Allow, 10).unwrap(),
        Outcome::Committed(first.clone())
    );
    let assigned = assign(&mut store, first.revision);
    let event = append(assigned.revision, 0, EventKind::Progress);
    let progress = committed(store.mutate(&event, &Allow, 10).unwrap());
    assert!(store.mutate(&event, &Deny, 10).is_err());
    let mut conflict = event.clone();
    if let Operation::Append { event, .. } = &mut conflict.operation {
        event.payload = json!({"text":"different"});
    }
    assert!(store
        .mutate(&conflict, &Allow, 10)
        .unwrap_err()
        .to_string()
        .contains("request conflict"));
    drop(store);
    let mut store = NativeStore::open(&path).unwrap();
    assert_eq!(
        store
            .receipt(&context(), &event.invocation_ref, &event.request_id, &Allow)
            .unwrap(),
        ReceiptLookup::Retained {
            request: Box::new(event.clone()),
            receipt: progress.clone()
        }
    );
    assert!(store
        .receipt(&context(), &event.invocation_ref, &event.request_id, &Deny)
        .is_err());

    assert_eq!(
        store.mutate(&event, &Allow, 1500).unwrap(),
        Outcome::Committed(progress.clone())
    );
    let finished = committed(
        store
            .mutate(
                &append(progress.revision, 1, EventKind::Completed),
                &Allow,
                20,
            )
            .unwrap(),
    );
    assert_eq!(finished.terminal, Some(EventKind::Completed));
    assert!(store
        .mutate(
            &append(finished.revision, 2, EventKind::Progress),
            &Allow,
            20
        )
        .unwrap_err()
        .to_string()
        .contains("terminal"));
    let page = store
        .events(&context(), &event.invocation_ref, None, 1, &Allow)
        .unwrap();
    assert_eq!(page.items[0].sequence, 0);
    assert!(!page.complete);
    assert_eq!(page.continuation, Some(0));
    let page = store
        .events(
            &context(),
            &event.invocation_ref,
            page.continuation,
            1,
            &Allow,
        )
        .unwrap();
    assert_eq!(page.items[0].sequence, 1);
    assert!(page.complete);
    assert!(page.continuation.is_none());
}
#[test]
fn wrong_assignment_revision_expiry_and_workspace_are_rejected_without_writing() {
    let mut store = NativeStore::open_in_memory().unwrap();
    let first = committed(store.mutate(&admit(), &Allow, 10).unwrap());
    let assigned = assign(&mut store, first.revision.clone());
    let mut wrong = append(assigned.revision.clone(), 0, EventKind::Progress);
    if let Operation::Append { executor_ref, .. } = &mut wrong.operation {
        *executor_ref = "cwp://test/executors/other".into();
    }
    assert!(store.mutate(&wrong, &Allow, 10).is_err());
    let stale = append(first.revision, 0, EventKind::Progress);
    assert!(store
        .mutate(&stale, &Allow, 10)
        .unwrap_err()
        .to_string()
        .contains("revision conflict"));
    assert!(store
        .mutate(
            &append(assigned.revision.clone(), 0, EventKind::Progress),
            &Allow,
            1000
        )
        .is_err());
    let mut foreign = context();
    foreign.workspace_ref = "cwp://test/workspaces/other".into();
    assert!(store
        .invocation(&foreign, &admit().invocation_ref, &Allow)
        .unwrap()
        .is_none());
    let current = store
        .invocation(&context(), &admit().invocation_ref, &Allow)
        .unwrap()
        .unwrap();
    assert_eq!(current.revision, assigned.revision);
    assert_eq!(current.next_sequence, 0);
    let stop = request(
        "interrupt",
        Some(current.revision),
        Operation::Stop {
            kind: EventKind::Interrupted,
            reason: json!({"uncertain_effects":["calendar-write-42"]}),
        },
    );
    let stopped = committed(store.mutate(&stop, &Allow, 1500).unwrap());
    let reassignment = request(
        "reassign",
        Some(stopped.revision),
        Operation::Assign {
            assignment: Assignment {
                executor_ref: "cwp://test/executors/two".into(),
                assignment_ref: "cwp://test/assignments/two".into(),
                generation: 2,
                route_fingerprint: "route-two".into(),
                expires_at_unix_ms: 2000,
            },
        },
    );
    assert!(store.mutate(&reassignment, &Allow, 1500).is_err());
}
#[test]
fn payload_erasure_expires_receipt_and_never_replays_mutation() {
    let mut store = NativeStore::open_in_memory().unwrap();
    let first = committed(store.mutate(&admit(), &Allow, 10).unwrap());
    let assigned = assign(&mut store, first.revision);
    let event = append(assigned.revision, 0, EventKind::Progress);
    store.mutate(&event, &Allow, 10).unwrap();
    // Simulate the storage owner's expiry of the receipt's disposable payload.
    store
        .graph_transaction(|tx| {
            let root = crate::derived_ref(
                "invocation_execution_journal_v1",
                &[&context().workspace_ref, &event.invocation_ref],
            );
            let reference = crate::derived_ref("invocation_execution_event_v1", &[&root, "0"]);
            let id = crate::find(tx, &reference)?.unwrap();
            tx.execute(
                "DELETE FROM edges WHERE target_id=?1 AND kind='sequence_mutation_receipt_v1'",
                [id],
            )?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        store.mutate(&event, &Allow, 10).unwrap(),
        Outcome::CommittedResultExpired
    );
    let current = store
        .invocation(&context(), &event.invocation_ref, &Allow)
        .unwrap()
        .unwrap();
    assert_eq!(current.next_sequence, 1);
}

#[test]
fn stop_during_append_rolls_back_event_cursor_and_receipt() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct StopsAfterAuthorization(AtomicBool);
    impl Authorizer for StopsAfterAuthorization {
        fn authorize(&self, _: &Context, _: &str, _: bool) -> CwpdbResult<()> {
            if self.0.swap(true, Ordering::SeqCst) {
                Err("normal CWR stop fence".into())
            } else {
                Ok(())
            }
        }
    }
    let mut store = NativeStore::open_in_memory().unwrap();
    let admitted = committed(store.mutate(&admit(), &Allow, 10).unwrap());
    let assigned = assign(&mut store, admitted.revision);
    let publication = append(assigned.revision.clone(), 0, EventKind::Progress);
    assert!(store
        .mutate(
            &publication,
            &StopsAfterAuthorization(AtomicBool::new(false)),
            10
        )
        .unwrap_err()
        .to_string()
        .contains("stop fence"));
    let retained = store
        .invocation(&context(), &publication.invocation_ref, &Allow)
        .unwrap()
        .unwrap();
    assert_eq!(retained.revision, assigned.revision);
    assert_eq!(retained.next_sequence, 0);
    assert_eq!(retained.terminal, None);
    assert!(store
        .events(&context(), &publication.invocation_ref, None, 10, &Allow)
        .unwrap()
        .items
        .is_empty());
    assert!(matches!(
        store
            .receipt(
                &context(),
                &publication.invocation_ref,
                &publication.request_id,
                &Allow
            )
            .unwrap(),
        ReceiptLookup::NotCommitted
    ));
    let accepted = store.mutate(&publication, &Allow, 10).unwrap();
    assert_eq!(store.mutate(&publication, &Allow, 10).unwrap(), accepted);
}

mod remote_authority;
