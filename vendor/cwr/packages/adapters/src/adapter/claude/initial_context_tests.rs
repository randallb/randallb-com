use super::*;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[test]
fn context_receipt_waits_through_empty_polls_until_the_source_is_complete() {
    let (sender, receiver) = mpsc::sync_channel(1);
    let sender = thread::spawn(move || {
        thread::sleep(Duration::from_millis(70));
        sender
            .send(Input::Host(AdapterHostMessage::Invoke {
                protocol: PROCESS_ADAPTER_PROTOCOL_REF.into(),
                invocation: Box::new(crate::adapter::AdapterInvocation::new(
                    "cwp://example.test/invocations/one",
                    "work",
                    json!({}),
                    json!({}),
                )),
            }))
            .unwrap();
    });
    let result = receive_initial_context(&receiver, || false).unwrap();
    assert!(matches!(
        result,
        Input::Host(AdapterHostMessage::Invoke { .. })
    ));
    sender.join().unwrap();
}

#[test]
fn stop_interrupts_initial_context_receipt_without_waiting_for_the_sender() {
    let (_sender, receiver) = mpsc::sync_channel(1);
    let stopped = Arc::new(AtomicBool::new(false));
    let receiver_stop = stopped.clone();
    let waiter = thread::spawn(move || {
        receive_initial_context(&receiver, || receiver_stop.load(Ordering::Relaxed))
            .err()
            .unwrap()
            .to_string()
    });
    stopped.store(true, Ordering::Relaxed);
    let started = Instant::now();
    assert!(waiter
        .join()
        .unwrap()
        .contains("stopped while receiving context"));
    assert!(started.elapsed() < Duration::from_millis(500));
}

#[test]
fn disconnected_context_source_fails_without_starting_a_provider() {
    let (sender, receiver) = mpsc::sync_channel(1);
    drop(sender);
    assert!(receive_initial_context(&receiver, || false).is_err());
}
