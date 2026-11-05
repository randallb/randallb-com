std::thread_local! {
    static PROJECTED: std::cell::RefCell<Option<Vec<(String, usize)>>> = const {
        std::cell::RefCell::new(None)
    };
}

/// Records worker-history projector calls made by the closure on this thread.
/// Each sample names the status conversation and counts retained message/activity
/// items supplied by that conversation and its direct execution conversations.
/// This opt-in test instrumentation does not retain production derived state.
pub fn measure_turn_status_projection<T>(run: impl FnOnce() -> T) -> (T, Vec<(String, usize)>) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            PROJECTED.with(|slot| *slot.borrow_mut() = None);
        }
    }
    PROJECTED.with(|slot| {
        assert!(slot.borrow().is_none(), "turn-status projection measurement is already active");
        *slot.borrow_mut() = Some(Vec::new());
    });
    let _reset = Reset;
    let result = run();
    let samples = PROJECTED.with(|slot| slot.borrow_mut().take().unwrap_or_default());
    (result, samples)
}

pub(super) fn record(store: &ConversationAdmissionStore, conversation_ref: &str) {
    PROJECTED.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(samples) = slot.as_mut() else { return; };
        let items = store.hydrated_conversations()
            .filter(|conversation| {
                conversation.conversation_ref == conversation_ref
                    || conversation.parent_conversation_ref.as_deref() == Some(conversation_ref)
            })
            .map(|conversation| conversation.messages.len() + conversation.activity.len())
            .sum();
        samples.push((conversation_ref.into(), items));
    });
}
