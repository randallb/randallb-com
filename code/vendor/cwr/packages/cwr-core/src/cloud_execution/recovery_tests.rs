fn assigned_fixture() -> (
    CloudCoordinator<NativeStore, CurrentAuthority>,
    String,
    String,
) {
    let mut coordinator = CloudCoordinator::new(
        NativeStore::open_in_memory().unwrap(),
        CurrentAuthority,
        vec![9; 32],
    )
    .unwrap();
    let (record, invocation) = admission("cwp://fixture.test/invocations/start");
    let admitted = receipt(
        coordinator
            .admit(context(), &record, &invocation, "admit".into(), 10)
            .unwrap(),
    );
    coordinator
        .assign(
            context(),
            record.invocation_ref.clone(),
            admitted.revision,
            Assignment {
                executor_ref: "cwp://fixture.test/executors/a".into(),
                assignment_ref: "cwp://fixture.test/assignments/a".into(),
                generation: 1,
                route_fingerprint: "route".into(),
                expires_at_unix_ms: 1000,
            },
            "assign".into(),
            10,
        )
        .unwrap();
    let token = coordinator
        .assignment_token(&context(), &record.invocation_ref)
        .unwrap();
    (coordinator, record.invocation_ref, token)
}
#[test]
fn uncertain_executor_start_cannot_be_replayed_on_a_different_boot() {
    let (mut coordinator, reference, token) = assigned_fixture();
    let started = coordinator
        .begin_execution(context(), reference.clone(), &token, "boot-a", 20)
        .unwrap();
    assert_eq!(
        coordinator
            .begin_execution(context(), reference.clone(), &token, "boot-a", 21)
            .unwrap(),
        started
    );
    assert!(coordinator
        .begin_execution(context(), reference, &token, "boot-b", 22)
        .unwrap_err()
        .to_string()
        .contains("reconciliation"));
}
#[test]
fn callback_receipt_lookup_recovers_cas_revision_after_acknowledgement_loss() {
    let (mut coordinator, reference, token) = assigned_fixture();
    coordinator
        .begin_execution(context(), reference.clone(), &token, "boot-a", 20)
        .unwrap();
    let frame = json!({"type":"progress","invocation_ref":reference,"sequence":1,"status":102,"summary":"Reading fictional fixture."});
    let first = coordinator
        .publish_recorded(
            context(),
            reference.clone(),
            &token,
            CloudCallback {
                request_id: "frame-one".into(),
                sequence: 1,
                frame: frame.clone(),
            },
            21,
        )
        .unwrap();
    let progressed = coordinator.record(&context(), &reference).unwrap();
    assert_eq!(progressed.next_sequence, 2);
    assert_eq!(
        coordinator
            .publish_recorded(
                context(),
                reference.clone(),
                &token,
                CloudCallback {
                    request_id: "frame-one".into(),
                    sequence: 1,
                    frame: frame.clone()
                },
                22
            )
            .unwrap(),
        first
    );
    let mut conflict = frame;
    conflict["summary"] = json!("A conflicting event.");
    assert!(coordinator
        .publish_recorded(
            context(),
            reference,
            &token,
            CloudCallback {
                request_id: "frame-one".into(),
                sequence: 1,
                frame: conflict
            },
            23
        )
        .is_err());
}
#[test]
fn invalid_terminal_schema_does_not_close_the_assignment() {
    let (mut coordinator, reference, token) = assigned_fixture();
    coordinator
        .begin_execution(context(), reference.clone(), &token, "boot-a", 20)
        .unwrap();
    let frame = json!({"type":"result","invocation_ref":reference,"sequence":1,"status":200,"summary":"Wrong result shape.","result":"must be an object"});
    assert!(coordinator
        .publish_recorded(
            context(),
            reference.clone(),
            &token,
            CloudCallback {
                request_id: "frame-one".into(),
                sequence: 1,
                frame
            },
            21
        )
        .is_err());
    assert_eq!(
        coordinator.record(&context(), &reference).unwrap().terminal,
        None
    );
}

#[test]
fn strict_publication_authority_refuses_stopped_receipt_replay_but_allows_cleanup() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    struct ScopedAuthority {
        strict: bool,
        stopped: Arc<AtomicBool>,
    }
    impl Authorizer for ScopedAuthority {
        fn authorize(&self, _: &Context, _: &str, _: bool) -> CwrResult<()> {
            if self.strict && self.stopped.load(Ordering::SeqCst) {
                Err("normal CWR stop fence".into())
            } else {
                Ok(())
            }
        }
    }
    let (previous, reference, token) = assigned_fixture();
    let stopped = Arc::new(AtomicBool::new(false));
    let mut coordinator = CloudCoordinator::new(
        previous.storage,
        ScopedAuthority {
            strict: false,
            stopped: stopped.clone(),
        },
        vec![9; 32],
    )
    .unwrap()
    .with_publication_authority(ScopedAuthority {
        strict: true,
        stopped: stopped.clone(),
    });
    coordinator
        .begin_execution(context(), reference.clone(), &token, "boot-a", 20)
        .unwrap();
    let callback = CloudCallback {
        request_id: "strict-frame".into(),
        sequence: 1,
        frame: json!({"type":"progress","invocation_ref":reference,"sequence":1,"status":102,"summary":"Accepted before stop"}),
    };
    coordinator
        .publish_recorded(context(), reference.clone(), &token, callback.clone(), 21)
        .unwrap();
    stopped.store(true, Ordering::SeqCst);
    assert!(coordinator
        .publish_recorded(context(), reference.clone(), &token, callback, 22)
        .unwrap_err()
        .to_string()
        .contains("stop fence"));
    assert!(coordinator
        .begin_execution(context(), reference.clone(), &token, "boot-a", 22)
        .is_err());
    let retained = coordinator.record(&context(), &reference).unwrap();
    assert_eq!(retained.next_sequence, 2);
    coordinator
        .stop(
            context(),
            reference.clone(),
            retained.revision,
            "strict-cleanup".into(),
            super::super::CloudInterruption {
                cancelled: true,
                uncertain_effects: json!([]),
            },
            23,
        )
        .unwrap();
    assert_eq!(
        coordinator.record(&context(), &reference).unwrap().terminal,
        Some(EventKind::Cancelled)
    );
}

#[test]
fn expired_start_proof_refuses_new_boot_and_dispatch_recovery_without_restarting() {
    let (mut coordinator, reference, token) = assigned_fixture();
    coordinator
        .begin_execution(context(), reference.clone(), &token, "boot-a", 20)
        .unwrap();
    let before = coordinator.record(&context(), &reference).unwrap();
    cwpdb_store::test_support::executor_start_receipt::expire_start_proof(
        &mut coordinator.storage,
        &context(),
        &reference,
    )
    .unwrap();
    let request = internal_request_id(
        "executor-start",
        &before.assignment.as_ref().unwrap().assignment_ref,
    );
    let expired = coordinator
        .lookup_receipt(&context(), &reference, &request)
        .unwrap();
    assert!(matches!(
        expired,
        cwpdb_store::invocation::ReceiptLookup::CommittedResultExpired
    ));
    // This exact classifier also guards dispatch's already-started shortcut.
    assert!(executor_start_receipt_available(&expired)
        .unwrap_err()
        .to_string()
        .contains("reconciliation required"));
    for boot in ["boot-b", "boot-a"] {
        let error = coordinator
            .begin_execution(context(), reference.clone(), &token, boot, 21)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("receipt expired") && error.contains("reconciliation required"),
            "{error}"
        );
    }
    assert_eq!(coordinator.record(&context(), &reference).unwrap(), before);
    let page = coordinator
        .retained_events(&context(), &reference, None, 10)
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].payload["worker_boot_id"], "boot-a");
    coordinator
        .stop(
            context(),
            reference.clone(),
            before.revision,
            "expired-start-cleanup".into(),
            super::super::CloudInterruption {
                cancelled: false,
                uncertain_effects: json!(["start proof expired"]),
            },
            22,
        )
        .unwrap();
    assert_eq!(
        coordinator.record(&context(), &reference).unwrap().terminal,
        Some(EventKind::Interrupted)
    );
}
