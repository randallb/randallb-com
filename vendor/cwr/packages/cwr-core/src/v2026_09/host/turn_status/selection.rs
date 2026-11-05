/// Selects status owners affected by changed invocation or conversation identities.
/// Invocation ancestry also reaches shared owners of nested and forwarded work;
/// conversation ancestry handles callers that know only a private execution ref.
/// Terminal records remain relevant so their previously published turns clear.
pub fn affected_turn_status_conversations(
    store: &ConversationAdmissionStore,
    invocation_refs: &BTreeSet<String>,
    conversation_refs: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut selected = conversation_refs.clone();
    let mut pending = invocation_refs.clone();
    for record in store.invocation_records() {
        if record
            .conversation_ref
            .as_ref()
            .is_some_and(|reference| conversation_refs.contains(reference))
            || store
                .execution_conversation_ref(&record.invocation_ref)
                .is_some_and(|reference| conversation_refs.contains(reference))
        {
            pending.insert(record.invocation_ref.clone());
        }
    }
    let mut visited = BTreeSet::new();
    while let Some(reference) = pending.pop_first() {
        if !visited.insert(reference.clone()) {
            continue;
        }
        let Some(record) = store.invocation_record(&reference) else {
            continue;
        };
        selected.extend(record.conversation_ref.iter().cloned());
        pending.extend(record.parent_invocation_ref.iter().cloned());
    }
    let mut pending = selected.clone();
    while let Some(reference) = pending.pop_first() {
        let parent = store
            .conversation_header(&reference)
            .and_then(|conversation| conversation.parent_conversation_ref.as_ref());
        if let Some(parent) = parent.filter(|parent| selected.insert((*parent).clone())) {
            pending.insert(parent.clone());
        }
    }
    selected
}
