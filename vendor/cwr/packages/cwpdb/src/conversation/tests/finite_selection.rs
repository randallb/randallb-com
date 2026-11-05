use super::*;
fn control() -> ReadControl {
    ReadControl {
        cancellation: ReadCancellation::default(),
        deadline: std::time::Instant::now() + std::time::Duration::from_secs(10),
        vm_steps: None,
    }
}
fn selection(operation: ReadSelectionOperation, revision: Option<String>) -> ReadSelectionRequest {
    ReadSelectionRequest {
        scope: scope(),
        expected_commit_revision: revision,
        selection: operation,
        deadline_unix_ms: i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis(),
        )
        .unwrap()
            + 10_000,
        max_bytes: MAX_WIRE_BYTES,
    }
}
#[test]
fn native_finite_selection_works_in_memory_and_restores_connection_after_refusal() {
    let mut store = NativeConversationStore::in_memory(scope(), options()).unwrap();
    let monitor = control();
    let cancelled = control();
    cancelled.cancellation.cancel();
    assert!(store
        .read_selection(
            &selection(ReadSelectionOperation::Position, None),
            &cancelled,
            &Allow
        )
        .is_err());
    let mut expired = selection(ReadSelectionOperation::Position, None);
    expired.deadline_unix_ms = 0;
    assert!(store.read_selection(&expired, &monitor, &Allow).is_err());
    let position = store
        .read_selection(
            &selection(ReadSelectionOperation::Position, None),
            &monitor,
            &Allow,
        )
        .unwrap();
    let bad = selection(
        ReadSelectionOperation::Current {
            conversation_ref: header().reference,
        },
        Some("stale".into()),
    );
    assert!(store
        .read_selection(&bad, &monitor, &Allow)
        .unwrap_err()
        .to_string()
        .contains("revision conflict"));
    let revision = store
        .read(&request(Selection::Bootstrap), &Allow)
        .unwrap()
        .revision;
    store.commit(&mutation(revision), &Allow).unwrap();
    let now = store
        .read_selection(
            &selection(ReadSelectionOperation::Position, None),
            &monitor,
            &Allow,
        )
        .unwrap();
    assert_ne!(position.commit_revision, now.commit_revision);
    assert!(store
        .get(&scope(), &header().reference, &Allow)
        .unwrap()
        .is_some());
    let read = selection(
        ReadSelectionOperation::Current {
            conversation_ref: header().reference,
        },
        Some(now.commit_revision),
    );
    assert!(store
        .read_selection(&read, &monitor, &Allow)
        .unwrap()
        .result
        .as_array()
        .unwrap()
        .is_empty());
}
#[test]
fn native_finite_selection_rechecks_domain_authorization_before_publication() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Revoke(AtomicUsize);
    impl Authorizer for Revoke {
        fn authorize(&self, _: &Context, reference: &str, _: bool) -> CwpdbResult<()> {
            if reference == header().reference && self.0.fetch_add(1, Ordering::SeqCst) > 0 {
                return Err("domain access revoked".into());
            }
            Ok(())
        }
    }
    let mut store = NativeConversationStore::in_memory(scope(), options()).unwrap();
    let request = selection(
        ReadSelectionOperation::Current {
            conversation_ref: header().reference,
        },
        Some("0".into()),
    );
    let error = store
        .read_selection(&request, &control(), &Revoke(AtomicUsize::new(0)))
        .unwrap_err();
    assert!(error.to_string().contains("domain access revoked"));
    assert!(store
        .read_selection(&request, &control(), &Allow)
        .unwrap()
        .result
        .as_array()
        .unwrap()
        .is_empty());
    let mut foreign = request;
    foreign.scope.context.workspace_ref = "cwp://foreign/workspace".into();
    assert!(store.read_selection(&foreign, &control(), &Allow).is_err());
}
