use super::*;

#[test]
fn selective_followup_does_not_replay_the_previous_observation() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = ConversationAdmissionStore::open(
        directory.path().join("followup-selective.sqlite"),
        "cwp://com.example.runtime",
    ).unwrap();
    let first = selective_working_turn(&mut store, None, "first");
    let claimed = claim_working(&mut store, &first);
    settle_working(&mut store, &claimed);
    let second = selective_working_turn(&mut store, Some(&first.record.conversation_ref), "second");
    let next = claim_working(&mut store, &second);
    let messages = next.adapter_invocation.thread["messages"].as_array().unwrap();
    assert!(messages.iter().any(|message| message["text"] == "second"));
    assert!(!messages.iter().any(|message| message["text"] == "first"));
    settle_working(&mut store, &next);
    let third = selective_working_turn(&mut store, Some(&first.record.conversation_ref), "third");
    let next = claim_working(&mut store, &third);
    let messages = next.adapter_invocation.thread["messages"].as_array().unwrap();
    assert!(messages.iter().any(|message| message["text"] == "third"));
    assert!(!messages.iter().any(|message| message["text"] == "second" || message["text"] == "first"));
}
