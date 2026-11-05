//! Remote refusal is delivery control; it cannot undo a server's durable commit.
use super::*;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
struct Active(Arc<AtomicBool>);
impl Authorizer for Active {
    fn authorize(&self, _: &Context, _: &str, _: bool) -> CwpdbResult<()> {
        if self.0.load(Ordering::SeqCst) {
            Err("normal CWR stopped".into())
        } else {
            Ok(())
        }
    }
}
struct StopAfterServerReply {
    server: Arc<Mutex<NativeStore>>,
    stopped: Arc<AtomicBool>,
}
impl crate::remote::DeclaredTransport for StopAfterServerReply {
    fn call(&mut self, body: serde_json::Value) -> CwpdbResult<serde_json::Value> {
        let mut server = self.server.lock().unwrap();
        let response = match body["operation"].as_str() {
            Some("mutate_invocation") => {
                let request: Mutation = serde_json::from_value(body["invocation_request"].clone())?;
                serde_json::to_value(server.mutate(&request, &Allow, 10)?)?
            }
            Some("invocation_receipt") => {
                let context: Context = serde_json::from_value(body["context"].clone())?;
                serde_json::to_value(server.receipt(
                    &context,
                    body["invocation_ref"].as_str().unwrap(),
                    body["request_id"].as_str().unwrap(),
                    &Allow,
                )?)?
            }
            _ => return Err("unexpected authority fixture operation".into()),
        };
        self.stopped.store(true, Ordering::SeqCst);
        Ok(response)
    }
}
fn setup() -> (Arc<Mutex<NativeStore>>, Mutation) {
    let mut server = NativeStore::open_in_memory().unwrap();
    let admitted = committed(server.mutate(&admit(), &Allow, 10).unwrap());
    let assigned = assign(&mut server, admitted.revision);
    (
        Arc::new(Mutex::new(server)),
        append(assigned.revision, 0, EventKind::Progress),
    )
}
#[test]
fn remote_append_refuses_late_delivery_without_claiming_remote_rollback() {
    let (server, append) = setup();
    let stopped = Arc::new(AtomicBool::new(false));
    let mut remote = crate::remote::RemoteStore::new(StopAfterServerReply {
        server: server.clone(),
        stopped: stopped.clone(),
    });
    assert!(remote
        .mutate(&append, &Active(stopped.clone()), 10)
        .unwrap_err()
        .to_string()
        .contains("stopped"));
    let mut retained = server.lock().unwrap();
    // The remote transaction completed before host authority changed. The host
    // refused delivery; the durable private journal remains for reconciliation.
    assert_eq!(
        retained
            .invocation(&context(), &append.invocation_ref, &Allow)
            .unwrap()
            .unwrap()
            .next_sequence,
        1
    );
    assert!(matches!(
        retained
            .receipt(
                &context(),
                &append.invocation_ref,
                &append.request_id,
                &Allow
            )
            .unwrap(),
        ReceiptLookup::Retained { .. }
    ));
    drop(retained);
    assert!(remote.mutate(&append, &Active(stopped), 10).is_err());
}
#[test]
fn remote_receipt_rechecks_active_authority_after_server_read() {
    let (server, append) = setup();
    server.lock().unwrap().mutate(&append, &Allow, 10).unwrap();
    let stopped = Arc::new(AtomicBool::new(false));
    let mut remote = crate::remote::RemoteStore::new(StopAfterServerReply {
        server,
        stopped: stopped.clone(),
    });
    assert!(remote
        .receipt(
            &context(),
            &append.invocation_ref,
            &append.request_id,
            &Active(stopped)
        )
        .unwrap_err()
        .to_string()
        .contains("stopped"));
}
