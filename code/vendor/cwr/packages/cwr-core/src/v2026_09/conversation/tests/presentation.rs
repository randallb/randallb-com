fn presentation_snapshot(store:&ConversationAdmissionStore,actor:&str)->ConversationPublicationSnapshot {
    let (path,runtime)=store.read_store_identity();
    ConversationPublicationSnapshot::open(&path,&runtime,actor,std::time::Instant::now()+std::time::Duration::from_secs(10),ConversationReadCancellation::pending()).unwrap()
}
#[test]
fn presentation_metadata_authorizes_artifacts_and_retains_actor_classification() {
    let (_directory,mut store,conversation,message,_invocation,_root,_parent)=populated_canonical_timeline();
    let actor=crate::v2026_09::test_fixture::actor_ref("person");
    store.retain_local_identities(&[crate::v2026_09::local::LocalActorIdentity {actor_ref:actor.into(),coworker_ref:actor.into(),handle:"@person".into(),name:"Person".into(),category:None,provider_model:None}]).unwrap();
    let artifact="cwp://com.example.runtime/artifacts/visible";
    let orphan="cwp://com.example.runtime/artifacts/private";
    store.transact(|state|{
        let retained=state.conversations.find_mut(|c|c.conversation_ref==conversation).unwrap();
        let source=retained.messages.iter_mut().find(|m|m.message_ref==message).unwrap();
        source.visualizations.push(ConversationVisualization {marker_index:0,title:Some("Chart".into()),mode:None,artifact_ref:Some(artifact.into()),error:None});
        for reference in [artifact,orphan] {state.visualization_artifacts.insert(reference.into(),ConversationVisualizationArtifact{artifact_ref:reference.into(),html:"<p>retained content</p>".into()});}
        Ok(())
    }).unwrap();
    let snapshot=presentation_snapshot(&store,actor);
    assert_eq!(snapshot.attachments(&message).unwrap().unwrap().len(),1);
    assert_eq!(snapshot.attachment(artifact).unwrap().unwrap().title.as_deref(),Some("Chart"));
    assert_eq!(snapshot.attachment_html(artifact).unwrap().as_deref(),Some("<p>retained content</p>"));
    assert!(snapshot.attachment_html(orphan).unwrap().is_none());
    let known=snapshot.related_actor(&message,actor).unwrap().unwrap();
    assert!(known.coworker); assert_eq!(known.name.as_deref(),Some("Person"));
    assert!(snapshot.actor(actor).unwrap().unwrap().coworker);
    assert!(snapshot.actor(&conversation).unwrap().is_none());
    assert!(snapshot.actor(artifact).unwrap().is_none());
    assert!(snapshot.actor(crate::v2026_09::test_fixture::actor_ref("helper")).unwrap().is_some());
    let hidden=presentation_snapshot(&store,"cwp://com.example.runtime/actors/stranger");
    assert!(hidden.message_metadata(&message).unwrap().is_none());
    assert!(hidden.attachments(&message).unwrap().is_none());
    assert!(hidden.attachment_html(artifact).unwrap().is_none());
    assert!(hidden.actor(actor).unwrap().is_none());
    drop(snapshot);
    store.retain_local_identities(&[]).unwrap();
    assert!(presentation_snapshot(&store,actor).actor(actor).unwrap().unwrap().coworker);
}
#[test]
fn presentation_filters_scan_multiple_batches_and_exclude_descendants_from_counts() {
    let (_directory,mut store,conversation,message,_invocation,root,parent)=populated_canonical_timeline();
    for index in 0..205 {
        store.publish_timeline_message(&conversation,&format!("{conversation}/publications/repeat-{index}"),&message,ConversationTimelineDisposition::Child {parent_item_ref:parent.item_ref.clone()}).unwrap();
    }
    let snapshot=presentation_snapshot(&store,crate::v2026_09::test_fixture::actor_ref("person"));
    let filter=ConversationPublicationFilter {source_types:vec![ConversationTimelineSourceType::Message],..Default::default()};
    let request=ConversationTimelinePageRequest {last:Some(2),..Default::default()};
    let children=snapshot.filtered_page(&conversation,parent.child_connection_ref.as_deref(),&request,&filter,true).unwrap().unwrap();
    assert_eq!(children.total_count,Some(206));
    assert_eq!(children.publications.len(),2);
    assert!(children.page.page_info.has_previous_page);
    assert!(children.publications[0].item.sibling_sequence<children.publications[1].item.sibling_sequence);
    let root=snapshot.filtered_page(&conversation,Some(&root),&request,&filter,true).unwrap().unwrap();
    assert_eq!(root.total_count,Some(2));
    let empty=snapshot.filtered_page(&conversation,parent.child_connection_ref.as_deref(),&ConversationTimelinePageRequest{first:Some(1),after:children.page.page_info.end_cursor,..Default::default()},&filter,true).unwrap().unwrap();
    assert!(empty.publications.is_empty());assert_eq!(empty.total_count,Some(206));
}
#[test]
fn child_role_filters_page_matching_replies_without_loading_other_messages() {
    use crate::open_responses::v2026_04_24::AssistantPhase;

    let directory = tempfile::tempdir().unwrap();
    let mut store = ConversationAdmissionStore::open_with_timeline_rollout(
        directory.path().join("child-role.sqlite"),
        "cwp://com.example.runtime",
        ConversationTimelineRollout::NewConversations,
    )
    .unwrap();
    let turn = working_turn(&mut store, None, "child-role");
    let claimed = claim_working(&mut store, &turn);
    let conversation = &turn.record.conversation_ref;
    let invocation = turn.record.invocation_ref.as_ref().unwrap();
    let parent = store
        .retained_timeline(conversation)
        .unwrap()
        .connections
        .iter()
        .flat_map(|connection| &connection.items)
        .find(|item| item.content.reference() == invocation)
        .unwrap()
        .clone();
    let child_connection = parent.child_connection_ref.clone().unwrap();
    let parent_item_ref = parent.item_ref;

    store
        .record_invocation_progress_with_publication(invocation, 102, "Earlier progress", true)
        .unwrap();
    let earlier = store
        .conversation(conversation)
        .unwrap()
        .messages
        .iter()
        .find(|message| message.text == "Earlier progress")
        .unwrap()
        .message_ref
        .clone();
    store
        .record_invocation_progress_with_publication(invocation, 102, "Latest progress", true)
        .unwrap();
    let latest = store
        .conversation(conversation)
        .unwrap()
        .messages
        .iter()
        .find(|message| message.text == "Latest progress")
        .unwrap()
        .message_ref
        .clone();
    let output = stream_output(&claimed, "answer", AssistantPhase::FinalAnswer, 1, "Partial answer");
    store.record_streaming_output(Some(&output), &[]).unwrap();
    let answer = store.state.streaming_messages.keys().next().unwrap().clone();
    let target = claimed.claim.invocation.target_actor_ref.clone();
    let source = store
        .conversation(conversation)
        .unwrap()
        .messages
        .iter()
        .find(|message| message.message_ref == turn.record.source_message_ref)
        .unwrap()
        .clone();
    let mut wrong_actor = source.clone();
    wrong_actor.message_ref = format!("{conversation}/messages/wrong-actor");
    wrong_actor.reply_to_message_ref = Some(source.message_ref.clone());
    wrong_actor.text = "Different actor".into();
    let mut wrong_source = source.clone();
    wrong_source.message_ref = format!("{conversation}/messages/wrong-source");
    wrong_source.author_actor_ref = target.clone();
    wrong_source.reply_to_message_ref = None;
    wrong_source.text = "No source reply".into();
    let mut action_call = source.clone();
    action_call.message_ref = format!("{conversation}/messages/action-call");
    action_call.author_actor_ref = target;
    action_call.reply_to_message_ref = Some(source.message_ref);
    action_call.text = "Action call".into();
    action_call.action = Some(ConversationActionEvent::Call {
        activation: "test".into(),
        input: json!({}),
    });
    let excluded = [wrong_actor, wrong_source, action_call];
    store
        .transact(|state| {
            state
                .conversations
                .find_mut(|entry| entry.conversation_ref == *conversation)
                .unwrap()
                .messages
                .extend(excluded.iter().cloned());
            Ok(())
        })
        .unwrap();
    for message in &excluded {
        store
            .publish_timeline_message(
                conversation,
                &format!("{conversation}/publications/{}", message.message_ref.rsplit('/').next().unwrap()),
                &message.message_ref,
                ConversationTimelineDisposition::Child {
                    parent_item_ref: parent_item_ref.clone(),
                },
            )
            .unwrap();
    }

    let snapshot = presentation_snapshot(&store, crate::v2026_09::test_fixture::actor_ref("person"));
    let window = ConversationTimelinePageRequest { last: Some(1), ..Default::default() };
    let commentary = snapshot
        .filtered_page(
            conversation,
            Some(&child_connection),
            &window,
            &ConversationPublicationFilter {
                child_role: Some(ConversationTimelineChildRole::Commentary),
                ..Default::default()
            },
            true,
        )
        .unwrap()
        .unwrap();
    assert_eq!(commentary.total_count, Some(2));
    assert_eq!(commentary.publications[0].item.content.reference(), latest);
    assert!(commentary.page.page_info.has_previous_page);
    assert!(!snapshot.resource_was_loaded(&earlier));
    assert!(!snapshot.resource_was_loaded(&answer));
    let final_reply = snapshot
        .filtered_page(
            conversation,
            Some(&child_connection),
            &window,
            &ConversationPublicationFilter {
                child_role: Some(ConversationTimelineChildRole::FinalReply),
                ..Default::default()
            },
            true,
        )
        .unwrap()
        .unwrap();
    assert_eq!(final_reply.total_count, Some(1));
    assert_eq!(final_reply.publications[0].item.content.reference(), answer);
    assert!(!final_reply.page.page_info.has_previous_page);
    drop(snapshot);

    settle_working(&mut store, &claimed);
    let published = presentation_snapshot(&store, crate::v2026_09::test_fixture::actor_ref("person"))
        .filtered_page(
            conversation,
            Some(&child_connection),
            &window,
            &ConversationPublicationFilter {
                child_role: Some(ConversationTimelineChildRole::FinalReply),
                ..Default::default()
            },
            false,
        )
        .unwrap()
        .unwrap();
    assert_eq!(published.publications[0].item.content.reference(), answer);
}
#[test]
fn presentation_stream_search_is_snapshot_coherent_and_progress_keeps_public_response() {
    use crate::open_responses::v2026_04_24::AssistantPhase;
    let directory=tempfile::tempdir().unwrap();let mut store=ConversationAdmissionStore::open(directory.path().join("presentation.sqlite"),"cwp://com.example.runtime").unwrap();
    store.configure_timeline_rollout(ConversationTimelineRollout::NewConversations).unwrap();
    let turn=working_turn(&mut store,None,"presentation");let claimed=claim_working(&mut store,&turn);
    let mut output=stream_output(&claimed,"answer",AssistantPhase::FinalAnswer,1,"Partial match");
    store.record_streaming_output(Some(&output),&[]).unwrap();
    let reference=store.state.streaming_messages.keys().next().unwrap().clone();
    let frozen=presentation_snapshot(&store,crate::v2026_09::test_fixture::actor_ref("person"));
    let parent=frozen.page(&turn.record.conversation_ref,None,&ConversationTimelinePageRequest{first:Some(10),..Default::default()},false).unwrap().unwrap().publications.into_iter().find(|p|matches!(p.item.content,ConversationTimelineContent::Invocation{..})).unwrap();
    assert_eq!(frozen.message_metadata(&reference).unwrap().unwrap().streaming_phase.as_deref(), Some("final_answer"));
    output.sequence_number=2;output.text="Changed answer".into();store.record_streaming_output(Some(&output),&[]).unwrap();
    let request=ConversationTimelinePageRequest{first:Some(10),..Default::default()};let filter=ConversationPublicationFilter{search:Some("partial".into()),..Default::default()};
    let page=frozen.filtered_page(&turn.record.conversation_ref,parent.item.child_connection_ref.as_deref(),&request,&filter,true).unwrap().unwrap();
    assert_eq!(page.total_count,Some(1));assert_eq!(page.publications[0].item.content.reference(),reference);
    let fresh=presentation_snapshot(&store,crate::v2026_09::test_fixture::actor_ref("person"));
    assert_eq!(fresh.filtered_page(&turn.record.conversation_ref,parent.item.child_connection_ref.as_deref(),&request,&filter,true).unwrap().unwrap().total_count,Some(0));
    store.record_invocation_progress_with_publication(&output.invocation_ref,102,"Public progress summary",true).unwrap();
    let message=store.state.conversations.iter().flat_map(|c|&c.messages).find(|m|m.progress.is_some()).unwrap().message_ref.clone();
    let fresh=presentation_snapshot(&store,crate::v2026_09::test_fixture::actor_ref("person"));
    let metadata=fresh.message_metadata(&message).unwrap().unwrap();
    assert_eq!(metadata.progress_summary.as_deref(),Some("Public progress summary"));assert_eq!(metadata.progress_response_status,Some(102));
}

#[test]
fn presentation_running_activity_stops_after_interruption_and_cannot_resume_on_retry() {
    let directory=tempfile::tempdir().unwrap();
    let mut store=ConversationAdmissionStore::open(directory.path().join("timing.sqlite"),"cwp://com.example.runtime").unwrap();
    store.configure_timeline_rollout(ConversationTimelineRollout::NewConversations).unwrap();
    let turn=working_turn(&mut store,None,"activity-timing");let claimed=claim_working(&mut store,&turn);
    let reference=claimed.adapter_invocation.invocation_ref.clone();
    let activity=store.append_execution_activity(&reference,ConversationActivityEvent::WorkUpdate{
        invocation_ref:reference.clone(),source_message_ref:turn.record.source_message_ref.clone(),parent_invocation_ref:None,wait:None,
        actor_ref:claimed.claim.invocation.target_actor_ref.clone(),status:ActionResponseStatus::PROGRESS,
        text:Some("Working".into()),text_kind:Some(WorkUpdateTextKind::Progress),
        activity:Some(Box::new(WorkActivity{activity_ref:"fixture-tool".into(),kind:"command".into(),label:"Working".into(),state:"running".into(),started_at_unix_ms:100,child_invocation_ref:None,duration_ms:None,command:None,output:None,error:None,exit_code:None,files:vec![]})),
    }).unwrap();
    store.transact(|state|working::publish_progress(state,&reference,&activity,"Working".into(),Some(ActionResponseStatus::PROGRESS))).unwrap();
    let message=store.state.conversations.iter().flat_map(|c|&c.messages).find(|m|m.progress.is_some()).unwrap().message_ref.clone();
    let actor=crate::v2026_09::test_fixture::actor_ref("person");
    let active=presentation_snapshot(&store,actor);
    assert!(active.message_metadata(&message).unwrap().unwrap().progress_timing().unwrap().is_some());
    assert!(store.recover_interrupted_invocation(&reference).unwrap());
    let inactive=presentation_snapshot(&store,actor);
    assert!(inactive.message_metadata(&message).unwrap().unwrap().progress_timing().unwrap().is_none());
    let _retry=claim_working(&mut store,&turn);
    assert!(presentation_snapshot(&store,actor).message_metadata(&message).unwrap().unwrap().progress_timing().unwrap().is_none());
    assert!(active.message_metadata(&message).unwrap().unwrap().progress_timing().unwrap().is_some());
}

#[test]
fn publication_layout_keeps_an_explicitly_empty_canonical_root_authoritative() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = ConversationAdmissionStore::open(directory.path().join("empty-canonical.sqlite"), "cwp://com.example.runtime").unwrap();
    let turn = working_turn(&mut store, None, "empty-canonical");
    let conversation = turn.record.conversation_ref;
    let root = format!("{conversation}/empty-root");
    store.create_retained_timeline(&conversation, &root).unwrap();
    let snapshot = presentation_snapshot(&store, crate::v2026_09::test_fixture::actor_ref("person"));
    assert_eq!(snapshot.timeline_layout(&conversation).unwrap().as_deref(), Some(root.as_str()));
    assert!(snapshot.page(&conversation, None, &ConversationTimelinePageRequest { last: Some(10), ..Default::default() }, false).unwrap().unwrap().page.edges.is_empty());
    assert!(snapshot.legacy_page(&conversation, &ConversationRecordPageRequest { last: Some(10), ..Default::default() }).unwrap().is_none());
}
