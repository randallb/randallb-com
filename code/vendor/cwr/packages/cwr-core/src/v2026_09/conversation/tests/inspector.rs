fn inspector_snapshot(store:&ConversationAdmissionStore,actor:&str)->ConversationPublicationSnapshot {
    let (path,runtime)=store.read_store_identity();
    ConversationPublicationSnapshot::open(&path,&runtime,actor,std::time::Instant::now()+std::time::Duration::from_secs(10),ConversationReadCancellation::pending()).unwrap()
}
fn inspector_tool(store:&mut ConversationAdmissionStore,invocation:&str,id:&str,completed:bool,output:&str)->String {
    store.append_execution_activity(invocation,ConversationActivityEvent::ProviderEvent{
        provider:"codex".into(),method:if completed {"item/completed"} else {"item/started"}.into(),
        data:json!({"item":{"id":id,"type":"commandExecution","command":"echo private","aggregatedOutput":output,"exitCode":0,"status":if completed {"completed"} else {"inProgress"}}})
    }).unwrap();
    store.state.tool_operations.values().filter(|op| op.invocation_ref==invocation && op.provider_item_id==id).max_by_key(|op| op.first_sequence).unwrap().activity.activity_ref.clone()
}
#[test]
fn inspector_activity_pages_keep_identity_through_updates_and_snapshots() {
    use ConversationInspectorCollection::Activities;
    let (_dir,mut store,_conversation,_message,invocation,_root,_parent)=populated_canonical_timeline();
    store.claim_invocation(&invocation,100).unwrap();
    let first=inspector_tool(&mut store,&invocation,"a",false,"");
    let second=inspector_tool(&mut store,&invocation,"b",false,"");
    let old=inspector_snapshot(&store,crate::v2026_09::test_fixture::actor_ref("person"));
    let request=ConversationInspectorRequest{first:Some(1),..Default::default()};
    let page=old.inspector_page(&invocation,Activities,&request).unwrap().unwrap();
    assert!(!page.complete);assert!(page.has_next_page);assert_eq!(page.entries.len(),1);
    assert_eq!(page.entries[0].0.reference,first);
    let key=page.entries[0].0.clone();
    inspector_tool(&mut store,&invocation,"a",true,&"🦀".repeat(5000));
    let fresh=inspector_snapshot(&store,crate::v2026_09::test_fixture::actor_ref("person"));
    let updated=fresh.inspector_page(&invocation,Activities,&request).unwrap().unwrap();
    assert_eq!(updated.entries[0].0,key);
    assert!(matches!(&updated.entries[0].1,ConversationInspectorEntry::Activity{state,active:false,..} if state=="completed"));
    assert!(matches!(&old.inspector_page(&invocation,Activities,&request).unwrap().unwrap().entries[0].1,ConversationInspectorEntry::Activity{state,active:true,..} if state=="running"));
    let next=fresh.inspector_page(&invocation,Activities,&ConversationInspectorRequest{last:Some(1),after:Some(key),..Default::default()}).unwrap().unwrap();
    assert_eq!(next.entries[0].0.reference,second);assert!(!next.has_next_page);
    let preview=fresh.inspector_text(&invocation,&first,None,ConversationInspectorTextField::Output).unwrap().unwrap();
    assert!(preview.truncated);assert_eq!(preview.text.len(),16384);assert!(preview.text.ends_with('🦀'));
    assert!(old.inspector_text(&invocation,&first,None,ConversationInspectorTextField::Output).unwrap().is_none());
    assert!(fresh.inspector_page(&invocation,Activities,&ConversationInspectorRequest{first:Some(0),..Default::default()}).is_err());
    assert!(fresh.inspector_page(&invocation,Activities,&ConversationInspectorRequest{after:Some(ConversationInspectorKey{scope:"foreign".into(),position:1,reference:"foreign".into()}),..Default::default()}).is_err());
    // Repeated payload selections share the same response budget.
    for _ in 0..40 {if fresh.inspector_text(&invocation,&first,None,ConversationInspectorTextField::Output).is_err() {return;}}
    panic!("inspector response budget did not limit repeated previews");
}
#[test]
fn inspector_payload_visibility_is_narrower_than_conversation_participation() {
    let (_dir,mut store,conversation,_message,invocation,_root,_parent)=populated_canonical_timeline();
    store.claim_invocation(&invocation,100).unwrap();
    let activity=inspector_tool(&mut store,&invocation,"private",true,"secret output");
    let reviewer="cwp://com.example.fixture/actors/reviewer";
    store.transact(|state| {state.conversations.find_mut(|c|c.conversation_ref==conversation).unwrap().participant_actor_refs.push(reviewer.into());Ok(())}).unwrap();
    let participant=inspector_snapshot(&store,reviewer);
    let page=participant.inspector_page(&invocation,ConversationInspectorCollection::Activities,&Default::default()).unwrap().unwrap();
    assert_eq!(page.entries.len(),1);
    assert!(matches!(&page.entries[0].1,ConversationInspectorEntry::Activity{label,details_available:false,..} if label=="Tool activity"));
    assert!(participant.inspector_text(&invocation,&activity,None,ConversationInspectorTextField::Command).unwrap().is_none());
    assert!(participant.inspector_files(&invocation,&activity,&Default::default()).unwrap().is_none());
    let stranger=inspector_snapshot(&store,"cwp://com.example.fixture/actors/stranger");
    assert!(stranger.inspector_page(&invocation,ConversationInspectorCollection::Activities,&Default::default()).unwrap().is_none());
    assert!(stranger.inspector_text(&invocation,&activity,None,ConversationInspectorTextField::Output).unwrap().is_none());
}
#[test]
fn inspector_files_are_paged_and_diffs_are_selected_separately() {
    let (_dir,mut store,_conversation,_message,invocation,_root,_parent)=populated_canonical_timeline();
    store.claim_invocation(&invocation,100).unwrap();
    store.append_execution_activity(&invocation,ConversationActivityEvent::ProviderEvent{provider:"codex".into(),method:"item/completed".into(),data:json!({"item":{"id":"edit","type":"fileChange","status":"completed","changes":[{"path":"one.rs","kind":"update","diff":"large diff"},{"path":"two.rs","kind":"add","diff":"+new"}]}})}).unwrap();
    let activity=store.state.tool_operations.values().find(|op| op.provider_item_id=="edit").unwrap().activity.activity_ref.clone();
    let snapshot=inspector_snapshot(&store,crate::v2026_09::test_fixture::actor_ref("person"));
    let files=snapshot.inspector_files(&invocation,&activity,&ConversationInspectorRequest{last:Some(1),..Default::default()}).unwrap().unwrap();
    assert!(files.has_previous_page);assert_eq!(files.entries.len(),1);
    assert!(matches!(&files.entries[0].1,ConversationInspectorEntry::File{path,..} if path=="two.rs"));
    let diff=snapshot.inspector_text(&invocation,&activity,Some(&files.entries[0].0),ConversationInspectorTextField::Diff).unwrap().unwrap();
    assert_eq!(diff.text,"+new");assert!(!diff.truncated);
}
#[test]
fn inspector_retries_do_not_merge_provider_item_ids_from_old_attempts() {
    let (_dir,mut store,_conversation,_message,invocation,_root,_parent)=populated_canonical_timeline();
    store.claim_invocation(&invocation,100).unwrap();
    let old=inspector_tool(&mut store,&invocation,"same-id",false,"");
    store.recover_interrupted_invocation(&invocation).unwrap();
    store.claim_invocation(&invocation,200).unwrap();
    let next=inspector_tool(&mut store,&invocation,"same-id",true,"retry");
    let snapshot=inspector_snapshot(&store,crate::v2026_09::test_fixture::actor_ref("person"));
    let page=snapshot.inspector_page(&invocation,ConversationInspectorCollection::Activities,&Default::default()).unwrap().unwrap();
    assert_eq!(page.entries.len(),2);
    assert_eq!(page.entries[0].0.reference,old);assert_eq!(page.entries[1].0.reference,next);
    assert!(matches!(&page.entries[0].1,ConversationInspectorEntry::Activity{active:false,..}));
}
#[test]
fn inspector_calls_and_retained_waits_are_distinct_and_hide_private_children() {
    use crate::v2026_09::invocation::{StartActionInvocation,InvocationOrigin};
    let (_dir,mut store,conversation,message,invocation,_root,_parent)=populated_canonical_timeline();
    store.claim_invocation(&invocation,100).unwrap();
    let other="cwp://com.example.runtime/conversations/private-child-root".to_owned();
    store.transact(|state| {
        let mut alternate=state.conversations.iter().find(|c|c.conversation_ref==conversation).unwrap().clone();
        alternate.conversation_ref=other.clone();
        alternate.timeline_connection_ref=None;
        alternate.activity.clear();
        alternate.participant_actor_refs=vec![crate::v2026_09::test_fixture::actor_ref("helper").into()];
        state.conversations.push(alternate);
        Ok(())
    }).unwrap();
    for index in 0..3 {
        let child=format!("cwp://com.example.runtime/invocations/inspector-child-{index}");
        store.start_child_invocation(StartActionInvocation{
                invocation_ref:child.clone(),origin:InvocationOrigin::HostCall{caller_invocation_ref:invocation.clone(),call_id:format!("start-{index}")},
                requester_actor_ref:crate::v2026_09::test_fixture::actor_ref("person").into(),
                target_actor_ref:crate::v2026_09::test_fixture::actor_ref("helper").into(),conversation_ref:Some(if index==1 {other.clone()} else {conversation.clone()}),actor_integration_ref:None,
                action_name:"review--work".into(),input:json!({"private":"input"}),parent_invocation_ref:Some(invocation.clone()),admitted_at_unix_ms:200+index,
            }, &message, crate::adapter::AdapterInvocation::new(&child,"review--work",json!({"private":"input"}),json!({"type":"object"}))).unwrap();
        if index!=1 {store.retain_invocation_timeline_publication(&child,&message,&format!("{conversation}/publications/child-invocation-{index}"),ConversationTimelineDisposition::Root).unwrap();}
    }
    // Existing waits are retained legacy records, independent of start call identity.
    let old=store.state.clone();
    let mut new=old.clone();
    {
        let state=&mut new;
        let mut wire=serde_json::to_value(&state.invocation_ledger).unwrap();
        wire["waits"]=json!([{"waiter_invocation_ref":invocation,"awaited_invocation_ref":"cwp://com.example.runtime/invocations/inspector-child-0","call_id":"wait-0","state":"abandoned"}]);
        state.invocation_ledger=serde_json::from_value(wire).unwrap();
    }
    store.commit_revision = crate::cwpdb::test_support::conversation::abandoned_wait(
        &store.state_path, &store.state.runtime_root_ref, &invocation,
        "cwp://com.example.runtime/invocations/inspector-child-0", "wait-0",
    ).unwrap();
    store.state=new;

    let snapshot=inspector_snapshot(&store,crate::v2026_09::test_fixture::actor_ref("person"));
    let page=snapshot.inspector_page(&invocation,ConversationInspectorCollection::Calls,&ConversationInspectorRequest{first:Some(2),..Default::default()}).unwrap().unwrap();
    assert!(page.complete);assert!(page.has_next_page);assert_eq!(page.entries.len(),2);
    assert_eq!(page.entries[1].0.reference,"start-1");
    assert!(!serde_json::to_string(&page.entries[1].0).unwrap().contains("inspector-child-1"));
    assert!(matches!(&page.entries[0].1,ConversationInspectorEntry::Call{call_id,callee_invocation_ref:Some(_)} if call_id=="start-0"));
    assert!(matches!(&page.entries[1].1,ConversationInspectorEntry::Call{call_id,callee_invocation_ref:None} if call_id=="start-1"));
    let last=snapshot.inspector_page(&invocation,ConversationInspectorCollection::Calls,&ConversationInspectorRequest{last:Some(1),after:Some(page.entries[1].0.clone()),..Default::default()}).unwrap().unwrap();
    assert!(matches!(&last.entries[0].1,ConversationInspectorEntry::Call{call_id,..} if call_id=="start-2"));
    let waits=snapshot.inspector_page(&invocation,ConversationInspectorCollection::Waits,&Default::default()).unwrap().unwrap();
    assert!(waits.complete);assert_eq!(waits.entries.len(),1);
    assert!(matches!(&waits.entries[0].1,ConversationInspectorEntry::Wait{call_id,state:crate::v2026_09::invocation::InvocationWaitState::Abandoned,..} if call_id=="wait-0"));
}
#[test]
fn inspector_reopens_canonical_retry_operations_without_rebuilding() {
    let (_dir,mut store,_conversation,_message,invocation,_root,_parent)=populated_canonical_timeline();
    store.claim_invocation(&invocation,100).unwrap();
    let first=inspector_tool(&mut store,&invocation,"same-id",false,"");
    store.recover_interrupted_invocation(&invocation).unwrap();
    store.claim_invocation(&invocation,200).unwrap();
    let next=inspector_tool(&mut store,&invocation,"same-id",true,"retry");
    let (path,runtime)=store.read_store_identity();drop(store);
    let store=ConversationAdmissionStore::open(path,runtime).unwrap();
    let snapshot=inspector_snapshot(&store,crate::v2026_09::test_fixture::actor_ref("person"));
    let page=snapshot.inspector_page(&invocation,ConversationInspectorCollection::Activities,&Default::default()).unwrap().unwrap();
    assert_eq!(page.entries.len(),2);
    assert_eq!(page.entries[0].0.reference,first);assert_eq!(page.entries[1].0.reference,next);
}
#[test]
fn inspector_originating_requester_can_read_delegated_tool_details() {
    use crate::v2026_09::invocation::{StartActionInvocation,InvocationOrigin};
    let (_dir,mut store,conversation,_message,invocation,_root,_parent)=populated_canonical_timeline();
    store.claim_invocation(&invocation,100).unwrap();
    let helper=crate::v2026_09::test_fixture::actor_ref("helper");
    let reviewer="cwp://com.example.fixture/actors/reviewer";
    let child="cwp://com.example.runtime/invocations/delegated-inspector";
    store.append_execution_activity(&invocation,ConversationActivityEvent::ProviderEvent {
        provider:"codex".into(),method:"item/started".into(),data:json!({"item":{"id":"provider-delegate","callId":"delegate","type":"dynamicToolCall","tool":"cwr.invoke_action"}}),
    }).unwrap();
    let call=store.append_host_action_call(ConversationHostActionCall{
        conversation_ref:&conversation,caller_invocation_ref:&invocation,host_call_id:"delegate",nested_invocation_ref:child,
        caller_actor_ref:helper,target_actor_ref:reviewer,activation:"@reviewer--review--work",input:json!({}),text:"Review".into(),
    }).unwrap();
    store.start_child_invocation(StartActionInvocation{invocation_ref:child.into(),
        origin:InvocationOrigin::HostCall{caller_invocation_ref:invocation.clone(),call_id:"delegate".into()},
        requester_actor_ref:helper.into(),target_actor_ref:reviewer.into(),conversation_ref:Some(conversation.clone()),actor_integration_ref:Some("cwp://com.example.fixture/actor_integrations/reviewer_local_execution".into()),
        action_name:"review--work".into(),input:json!({}),parent_invocation_ref:Some(invocation.clone()),admitted_at_unix_ms:200},
        &call.message_ref,crate::adapter::AdapterInvocation::new(child,"review--work",json!({}),json!({"type":"object"}))).unwrap();
    store.retain_invocation_timeline_publication(child,&call.message_ref,&format!("{conversation}/publications/delegated"),ConversationTimelineDisposition::Root).unwrap();
    store.claim_invocation(child,300).unwrap();
    let parent=inspector_snapshot(&store,crate::v2026_09::test_fixture::actor_ref("person"));
    let page=parent.inspector_page(&invocation,ConversationInspectorCollection::Activities,&Default::default()).unwrap().unwrap();
    assert!(matches!(&page.entries[0].1,ConversationInspectorEntry::Activity{child_invocation_ref:Some(reference),..} if reference==child));
    drop(parent);
    // Completion arriving after admission also preserves the child association.
    store.append_execution_activity(&invocation,ConversationActivityEvent::ProviderEvent {
        provider:"codex".into(),method:"item/completed".into(),data:json!({"item":{"id":"provider-delegate","callId":"delegate","type":"dynamicToolCall","tool":"cwr.invoke_action"}}),
    }).unwrap();
    assert!(store.state.tool_operations.values().any(|operation|operation.activity.child_invocation_ref.as_deref()==Some(child)));

    let activity=inspector_tool(&mut store,child,"command",true,"delegated output");
    let snapshot=inspector_snapshot(&store,crate::v2026_09::test_fixture::actor_ref("person"));
    assert_eq!(snapshot.inspector_text(child,&activity,None,ConversationInspectorTextField::Output).unwrap().unwrap().text,"delegated output");
}

#[test]
fn inspector_capture_is_sampled_at_attempt_boundary_and_never_backfills() {
    let (_dir,mut store,_conversation,_message,invocation,_root,_parent)=populated_canonical_timeline();
    store.timeline_rollout=Some(ConversationTimelineRollout::Disabled);
    store.claim_invocation(&invocation,100).unwrap();
    store.append_execution_activity(&invocation,ConversationActivityEvent::ProviderEvent {
        provider:"codex".into(),method:"item/started".into(),
        data:json!({"item":{"id":"old","type":"commandExecution","command":"old"}}),
    }).unwrap();
    store.timeline_rollout=None;
    let snapshot=inspector_snapshot(&store,crate::v2026_09::test_fixture::actor_ref("person"));
    assert!(snapshot.inspector_page(&invocation,ConversationInspectorCollection::Activities,&Default::default()).unwrap().is_none());
    drop(snapshot);
    store.recover_interrupted_invocation(&invocation).unwrap();
    store.claim_invocation(&invocation,200).unwrap();
    // A captured attempt with no supported tools is available and empty.
    let snapshot=inspector_snapshot(&store,crate::v2026_09::test_fixture::actor_ref("person"));
    assert!(snapshot.inspector_page(&invocation,ConversationInspectorCollection::Activities,&Default::default()).unwrap().unwrap().entries.is_empty());
    drop(snapshot);
    store.timeline_rollout=Some(ConversationTimelineRollout::Disabled);
    let activity=inspector_tool(&mut store,&invocation,"new",false,"");
    inspector_tool(&mut store,&invocation,"new",true,"done");
    // A replayed start cannot reopen a completed canonical operation.
    inspector_tool(&mut store,&invocation,"new",false,"");
    assert_eq!(store.state.tool_operations.get(&activity).unwrap().activity.state,"completed");
    store.recover_interrupted_invocation(&invocation).unwrap();
    store.claim_invocation(&invocation,300).unwrap();
    store.append_execution_activity(&invocation,ConversationActivityEvent::ProviderEvent {
        provider:"codex".into(),method:"item/started".into(),
        data:json!({"item":{"id":"disabled","type":"commandExecution","command":"disabled"}}),
    }).unwrap();
    assert_eq!(store.state.tool_operations.len(),1);
    let (path,runtime)=store.read_store_identity();drop(store);
    let store=ConversationAdmissionStore::open(path,runtime).unwrap();
    let snapshot=inspector_snapshot(&store,crate::v2026_09::test_fixture::actor_ref("person"));
    let page=snapshot.inspector_page(&invocation,ConversationInspectorCollection::Activities,&Default::default()).unwrap().unwrap();
    assert_eq!(page.entries.len(),1);
    assert_eq!(page.entries[0].0.reference,activity);
    assert!(!page.complete);
}

#[test]
fn inspector_canonical_capture_rolls_back_with_failed_event_write() {
    let (_dir,mut store,_conversation,_message,invocation,_root,_parent)=populated_canonical_timeline();
    store.claim_invocation(&invocation,100).unwrap();
    let before=store.execution_conversation(&invocation).unwrap().activity.len();
    crate::cwpdb::test_support::conversation::reject_tool_operation_writes(&store.state_path, true).unwrap();
    let result=store.append_execution_activity(&invocation,ConversationActivityEvent::ProviderEvent {
        provider:"codex".into(),method:"item/started".into(),
        data:json!({"item":{"id":"rollback","type":"commandExecution","command":"test"}}),
    });
    assert!(result.is_err());
    assert!(store.state.tool_operations.is_empty());
    assert_eq!(store.execution_conversation(&invocation).unwrap().activity.len(),before);
    crate::cwpdb::test_support::conversation::reject_tool_operation_writes(&store.state_path, false).unwrap();
    let activity=inspector_tool(&mut store,&invocation,"rollback",true,"done");
    assert!(store.state.tool_operations.contains_key(&activity));
}

#[test]
fn inspector_rejects_unsupported_record_schemas_on_reopen() {
    for kind in ["tool_attempt","tool_operation","tool_file"] {
        let (_dir,mut store,_conversation,_message,invocation,_root,_parent)=populated_canonical_timeline();
        store.claim_invocation(&invocation,100).unwrap();
        store.append_execution_activity(&invocation,ConversationActivityEvent::ProviderEvent {
            provider:"codex".into(),method:"item/completed".into(),data:json!({"item":{"id":"file","type":"fileChange","changes":[{"path":"file.rs","kind":"update","diff":"diff"}]}}),
        }).unwrap();
        let fixture_kind = match kind {
            "tool_attempt" => crate::cwpdb::test_support::conversation::InspectorRecordKind::Attempt,
            "tool_operation" => crate::cwpdb::test_support::conversation::InspectorRecordKind::Operation,
            "tool_file" => crate::cwpdb::test_support::conversation::InspectorRecordKind::File,
            _ => unreachable!(),
        };
        let changed=crate::cwpdb::test_support::conversation::unsupported_inspector_schema(&store.state_path, fixture_kind).unwrap();
        assert!(changed>0);
        let (path,runtime)=store.read_store_identity();drop(store);
        let result=ConversationAdmissionStore::open(path,runtime);
        match result { Ok(_)=>panic!("accepted unsupported {kind} schema"),Err(error)=>assert!(error.to_string().contains("unsupported record schema")) }
    }
}
