use super::*;
use crate::v2026_09::conversation::ConversationTimelineAnchor;

fn key(anchor: &ConversationTimelineAnchor) -> &crate::v2026_09::conversation::ConversationPositionKey {
    match anchor { ConversationTimelineAnchor::Message { key, .. } | ConversationTimelineAnchor::Turn { key, .. } => key }
}

fn messages(service: &LocalConversationModule<'_>, reference: &str) -> Vec<ConversationMessage> {
    let mut request = ConversationRecordPageRequest { first: Some(1), ..Default::default() };
    let mut messages = Vec::new();
    loop {
        let page = service.legacy_timeline(reference, &request, Instant::now()+Duration::from_secs(2)).unwrap();
        for anchor in &page.anchors {
            if let ConversationTimelineAnchor::Message { key, message_ref } = anchor {
                messages.push(service.legacy_message(reference, key, message_ref, Instant::now()+Duration::from_secs(2)).unwrap().unwrap());
            }
        }
        if !page.has_next_page {break;}
        request.after = Some(key(page.anchors.last().unwrap()).clone());
    }
    messages
}

#[test]
fn imported_module_reads_rootless_history_across_rollout_and_rejects_stale_cursors() {
    let temporary = independent_conversation_fixture();
    let workspace = temporary.path().join("CWR.toml");
    let graph = resolve_local_graph(&workspace).unwrap();
    let person = graph.actors.iter().find(|actor| actor.actor_name.as_deref()==Some("Person")).unwrap().actor_ref.clone();
    let helper = graph.actors.iter().find(|actor| actor.actor_name.as_deref()==Some("Helper")).unwrap().actor_ref.clone();
    let identity = || LocalHostIdentity::new(&person, "cwp://example.test/ingress");
    let path = temporary.path().join("retained.sqlite");
    let options = |rollout| LocalRuntimeOptions { conversation_timeline_rollout: rollout, ..Default::default() };
    let runtime = LocalRuntime::open_with_state_path_and_options(&workspace, &path, identity(), options(ConversationTimelineRollout::Disabled)).unwrap();
    let service = runtime.conversation_module().unwrap();
    let admission = service.publish("rootless", None, "@cwr--list--coworkers").unwrap();
    let reference = admission.conversation_ref().to_owned();
    assert!(service.timeline_layout(&reference, Instant::now()+Duration::from_secs(2)).unwrap().is_none());
    runtime.execute_admission(&admission, |_| {}).unwrap();
    let retained = messages(&service, &reference);
    assert!(retained.iter().any(|message| matches!(&message.action, Some(crate::v2026_09::conversation::ConversationActionEvent::Result { .. }))));
    let first = service.legacy_timeline(&reference, &ConversationRecordPageRequest{first:Some(1),..Default::default()}, Instant::now()+Duration::from_secs(2)).unwrap();
    let old_key = key(&first.anchors[0]).clone();
    let next = service.publish("rootless-followup", Some(&reference), "@cwr--list--coworkers").unwrap();
    runtime.execute_admission(&next, |_| {}).unwrap();
    assert!(service.legacy_timeline(&reference, &ConversationRecordPageRequest{after:Some(old_key.clone()),first:Some(1),..Default::default()}, Instant::now()+Duration::from_secs(2)).is_err());
    assert!(service.legacy_message(&reference, &old_key, &admission.record.source_message_ref, Instant::now()+Duration::from_secs(2)).is_err());
    let before_restart = messages(&service, &reference);
    service.set_archived(&reference, true).unwrap();
    service.set_archived(&reference, false).unwrap();
    drop(runtime);
    let runtime = LocalRuntime::open_with_state_path_and_options(&workspace, &path, identity(), options(ConversationTimelineRollout::NewConversations)).unwrap();
    let service = runtime.conversation_module().unwrap();
    assert!(service.timeline_layout(&reference, Instant::now()+Duration::from_secs(2)).unwrap().is_none());
    assert_eq!(messages(&service, &reference), before_restart);
    drop(runtime);
    let denied = LocalRuntime::open_with_state_path_and_options(&workspace, &path, LocalHostIdentity::new(&helper, "cwp://example.test/ingress"), options(ConversationTimelineRollout::Disabled)).unwrap();
    let service = denied.conversation_module().unwrap();
    assert!(service.timeline_layout(&reference, Instant::now()+Duration::from_secs(2)).is_err());
    assert!(service.legacy_timeline(&reference, &ConversationRecordPageRequest{first:Some(1),..Default::default()}, Instant::now()+Duration::from_secs(2)).is_err());
    assert!(service.legacy_message(&reference, &old_key, &admission.record.source_message_ref, Instant::now()+Duration::from_secs(2)).is_err());
    drop(denied);
    let migrated = LocalRuntime::open_with_state_path_and_options(&workspace, &path, identity(), options(ConversationTimelineRollout::MigrateLegacy)).unwrap();
    let service = migrated.conversation_module().unwrap();
    assert!(service.timeline_layout(&reference, Instant::now()+Duration::from_secs(2)).unwrap().is_some());
    assert!(service.legacy_timeline(&reference, &ConversationRecordPageRequest{first:Some(1),..Default::default()}, Instant::now()+Duration::from_secs(2)).is_err());
    assert!(service.canonical_timeline(&reference, None, &module_page(), Instant::now()+Duration::from_secs(2)).is_ok());
}
