struct DeclaredTestAuthorizer(cwpdb_store::invocation::Context);
impl cwpdb_store::invocation::Authorizer for DeclaredTestAuthorizer {
    fn authorize(&self,context:&cwpdb_store::invocation::Context,_:&str,_:bool)->cwpdb_store::CwpdbResult<()> {
        if context != &self.0 {return Err("test caller mismatch".into());} Ok(())
    }
}
fn declared_store(path: &Path) -> ConversationAdmissionStore {
    use cwpdb_store::{conversation::{NativeConversationStore, OpenOptions, Scope}, invocation::Context};
    let root = "cwp://com.example.runtime";
    let context = Context {workspace_ref:root.into(),caller_ref:root.into()};
    let scope = Scope {context:context.clone(),runtime_root_ref:root.into()};
    let storage=NativeConversationStore::open(path,scope.clone(),OpenOptions {runtime_state_version:STATE_VERSION.into(),workspace_binding:None,timeline_rollout:Some("migrate_legacy".into())}).unwrap();
    ConversationAdmissionStore::open_declared(storage,scope,DeclaredTestAuthorizer(context),ConversationTimelineRollout::MigrateLegacy).unwrap()
}

#[test]
fn declared_backend_runs_normal_admission_completion_and_replacement() {
    let temporary=tempfile::tempdir().unwrap();
    let path=temporary.path().join("declared.sqlite");
    let mut store=declared_store(&path);
    assert!(!store.backend.lock().unwrap().is_native());
    let first=working_turn(&mut store,None,"declared-first");
    let first_claim=claim_working(&mut store,&first);
    settle_working(&mut store,&first_claim);
    let root=first.record.conversation_ref.clone();
    let original_message=first.record.source_message_ref.clone();
    drop(store);
    let mut store=declared_store(&path);
    assert!(store.full_conversation(&root).unwrap().unwrap().messages.iter().any(|message|message.message_ref==original_message));
    let next=working_turn(&mut store,Some(&root),"declared-replacement");
    let next_claim=claim_working(&mut store,&next);
    assert_ne!(first_claim.adapter_invocation.invocation_ref,next_claim.adapter_invocation.invocation_ref);
    settle_working(&mut store,&next_claim);
    assert!(store.retained_timeline(&root).is_some());
}

#[test]
fn declared_backend_files_remain_private_and_intact_after_reopen() {
    let temporary=tempfile::tempdir().unwrap();let path=temporary.path().join("declared.sqlite");
    let owner="cwp://com.example.fixture/actors/owner";
    let mut store=declared_store(&path);
    let file=store.ingest_file(owner,"source.txt","text/plain",b"private source").unwrap();
    drop(store);let store=declared_store(&path);
    assert_eq!(store.read_file(owner,&file.file_ref).unwrap().content_hex,hex::encode(b"private source"));
    assert!(store.read_file("cwp://com.example.fixture/actors/outsider",&file.file_ref).is_err());
}

#[test]
fn declared_summary_rechecks_exact_participation_and_workspace_owner() {
    let temporary=tempfile::tempdir().unwrap();
    let mut store=declared_store(&temporary.path().join("summary.sqlite"));
    let person="cwp://com.example.fixture/actors/person";
    let helper="cwp://com.example.fixture/actors/helper";
    let outsider="cwp://com.example.fixture/actors/outsider";
    let exchange=store.record_host_exchange(None,person,helper,"Question".into(),"Visible reply".into()).unwrap();
    let reference=exchange.conversation.conversation_ref;
    let first=store.scoped_conversation_summary(&reference,None,person).unwrap().unwrap();
    assert_eq!(first.last_message_preview.as_deref(),Some("Visible reply"));
    assert!(store.scoped_conversation_summary(&reference,None,outsider).unwrap().is_none());
    let owner="cwp://com.example.fixture/contexts/private";
    store.transact(|state| {
        let row=state.conversations.find_mut(|row|row.conversation_ref==reference).unwrap();
        row.owner_context_ref=Some(owner.into());Ok(())
    }).unwrap();
    assert!(store.scoped_conversation_summary(&reference,None,person).unwrap().is_none());
    assert!(store.scoped_conversation_summary(&reference,Some("cwp://com.example.fixture/contexts/other"),person).unwrap().is_none());
    let changed=store.scoped_conversation_summary(&reference,Some(owner),person).unwrap().unwrap();
    assert_ne!(first.revision,changed.revision);
    store.transact(|state| {
        let row=state.conversations.find_mut(|row|row.conversation_ref==reference).unwrap();
        row.participant_actor_refs.retain(|actor|actor!=person);Ok(())
    }).unwrap();
    assert!(store.scoped_conversation_summary(&reference,Some(owner),person).unwrap().is_none());
    assert!(store.scoped_conversation_summary(&reference,Some(owner),helper).unwrap().is_some());
}
