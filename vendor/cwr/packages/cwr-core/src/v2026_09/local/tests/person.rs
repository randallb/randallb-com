fn person_workspace(root: &Path, enabled: bool) -> PathBuf {
    let workspace = root.join("workspace");
    copy_workspace(fixture_workspace().parent().unwrap(), &workspace);
    let coworker = workspace.join("coworkers/person/COWORKER.toml");
    let mut contents = fs::read_to_string(&coworker).unwrap();
    contents.push_str("\n[[integrations]]\npath = './integration.toml'\nas = 'person'\ndefault = true\n");
    fs::write(coworker, contents).unwrap();
    fs::write(workspace.join("coworkers/person/ROLE.toml"), format!("type = 'cwp://org.coworkerprotocol/2026-09/schemas/role'\n[[adapters]]\nref = '{}'\n", crate::adapter::PERSON_ADAPTER_REF)).unwrap();
    fs::write(workspace.join("coworkers/person/integration.toml"), format!("type = 'cwp://org.coworkerprotocol/2026-09/schemas/integration'\n[adapter]\nref = '{}'\n[params]\nhuman_response_actions = {enabled}\n", crate::adapter::PERSON_ADAPTER_REF)).unwrap();
    workspace.join("CWR.toml")
}

#[test]
fn person_response_persists_and_completes_exactly_once() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = person_workspace(temp.path(), true);
    let person = load_default_local_acting_actor_ref(&workspace).unwrap();
    let identity = LocalHostIdentity::new(&person, "cwp://org.coworkerprotocol.local/integrations/test");
    let path = temp.path().join("state.sqlite");
    let runtime = LocalRuntime::open_with_state_path(&workspace, &path, identity.clone()).unwrap();
    let question = runtime.admit_text(None, "@helper ask me a question").unwrap();
    runtime.lock().unwrap().admission.complete(question.invocation_ref().unwrap(), json!({"text":"Which color?"})).unwrap();
    let pending = runtime.lock().unwrap().admission.invocation_records().find(|r| r.state == ActionInvocationState::AwaitingPerson).cloned().unwrap();
    assert!(runtime.try_schedule_execution().unwrap().is_none());
    let conversation_ref = question.conversation_ref();
    assert_eq!(runtime.conversation(conversation_ref).unwrap().unwrap().messages.len(), 2);
    assert!(runtime.reply_person_request(&pending.requester_actor_ref, conversation_ref, &pending.invocation_ref, "Blue".into()).is_err());
    assert!(!runtime.has_unfinished_work().unwrap());
    runtime.shutdown(Duration::from_secs(1)).unwrap();
    drop(runtime);
    let runtime = LocalRuntime::open_with_state_path(&workspace, &path, identity).unwrap();
    assert!(!runtime.has_unfinished_work().unwrap());
    let reply = runtime.reply_person_request(&person, conversation_ref, &pending.invocation_ref, "Blue".into()).unwrap();
    assert!(runtime.has_unfinished_work().unwrap());
    let replay = runtime.reply_person_request(&person, conversation_ref, &pending.invocation_ref, "Blue".into()).unwrap();
    assert_eq!(reply.record, replay.record);
    assert_eq!(replay.conversation.messages.len(), 3);
    let state = runtime.lock().unwrap();
    assert_eq!(state.admission.runnable_invocation_refs().len(), 1);
    let next = state.admission.invocation_records().find(|r| r.is_runnable()).unwrap();
    assert_eq!(next.input["source_message_ref"], replay.conversation.messages[2].message_ref);
    assert_eq!(replay.conversation.messages[2].author_actor_ref, person);
    assert_eq!(replay.conversation.messages[2].reply_to_message_ref.as_deref(), Some(replay.conversation.messages[1].message_ref.as_str()));
}

#[test]
fn person_response_flag_off_preserves_record_only_behavior() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = person_workspace(temp.path(), false);
    let person = load_default_local_acting_actor_ref(&workspace).unwrap();
    let runtime = LocalRuntime::open_with_state_path(&workspace, temp.path().join("state.sqlite"), LocalHostIdentity::new(&person, "cwp://org.coworkerprotocol.local/integrations/test")).unwrap();
    let helper = runtime.catalog().unwrap().actors.iter().find(|a| a.handle == "@helper").unwrap().actor_ref.clone();
    let request = runtime.submit_text_as("cwp://org.coworkerprotocol.local/integrations/test", "question", helper, None, "@person Question").unwrap();
    assert_eq!(request.record.state, ConversationAdmissionState::Recorded);
    assert!(request.invocation_ref().is_none());
}

#[test]
fn person_response_disable_keeps_pending_answerable_and_stops_new_requests() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = person_workspace(temp.path(), true);
    let person = load_default_local_acting_actor_ref(&workspace).unwrap();
    let identity = LocalHostIdentity::new(&person, "cwp://org.coworkerprotocol.local/integrations/test");
    let path = temp.path().join("state.sqlite");
    let runtime = LocalRuntime::open_with_state_path(&workspace, &path, identity.clone()).unwrap();
    let helper = runtime.catalog().unwrap().actors.iter().find(|a| a.handle == "@helper").unwrap().actor_ref.clone();
    let request = runtime.submit_text_as("cwp://org.coworkerprotocol.local/integrations/test", "one", &helper, None, "@person Question").unwrap();
    let second = runtime.submit_text_as("cwp://org.coworkerprotocol.local/integrations/test", "two", &helper, Some(request.conversation_ref()), "@person Another question").unwrap();
    assert!(runtime.try_schedule_execution().unwrap().is_none());
    drop(runtime);
    let integration = workspace.parent().unwrap().join("coworkers/person/integration.toml");
    fs::write(&integration, fs::read_to_string(&integration).unwrap().replace("= true", "= false")).unwrap();
    let runtime = LocalRuntime::open_with_state_path(&workspace, &path, identity).unwrap();
    runtime.reply_person_request(&person, request.conversation_ref(), second.invocation_ref().unwrap(), "Second answer".into()).unwrap();
    let continuation = runtime.lock().unwrap().admission.invocation_records().find(|r| r.is_runnable()).unwrap().invocation_ref.clone();
    runtime.lock().unwrap().admission.complete(&continuation, json!({"text":"Thanks"})).unwrap();
    let state = runtime.lock().unwrap();
    assert_eq!(state.admission.invocation_records().filter(|r| r.state == ActionInvocationState::AwaitingPerson).count(), 1);
    assert_eq!(state.admission.invocation_record(request.invocation_ref().unwrap()).unwrap().state, ActionInvocationState::AwaitingPerson);
    drop(state);
    runtime.stop_invocation_as(request.invocation_ref().unwrap(), &helper, None).unwrap();
    assert!(runtime.reply_person_request(&person, request.conversation_ref(), request.invocation_ref().unwrap(), "Late".into()).is_err());
}

#[test]
fn person_response_stop_and_reply_race_has_one_terminal_outcome() {
    for _ in 0..8 {
        let temp = tempfile::tempdir().unwrap();
        let workspace = person_workspace(temp.path(), true);
        let person = load_default_local_acting_actor_ref(&workspace).unwrap();
        let runtime = LocalRuntime::open_with_state_path(&workspace, temp.path().join("state.sqlite"), LocalHostIdentity::new(&person, "cwp://org.coworkerprotocol.local/integrations/test")).unwrap();
        let helper = runtime.catalog().unwrap().actors.iter().find(|a| a.handle == "@helper").unwrap().actor_ref.clone();
        let request = runtime.submit_text_as("cwp://org.coworkerprotocol.local/integrations/test", "one", &helper, None, "@person Question").unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        std::thread::scope(|scope| {
            let barrier2 = barrier.clone();
            let runtime2 = runtime.clone();
            let request2 = request.clone();
            scope.spawn(move || {
                barrier2.wait();
                runtime2.stop_invocation_as(request2.invocation_ref().unwrap(), &helper, None).unwrap();
            });
            barrier.wait();
            let _ = runtime.reply_person_request(&person, request.conversation_ref(), request.invocation_ref().unwrap(), "Answer".into());
        });
        let state = runtime.lock().unwrap();
        let record = state.admission.invocation_record(request.invocation_ref().unwrap()).unwrap();
        assert_eq!(record.responses.len(), 1);
        assert!(record.state.is_terminal());
        assert_eq!(state.admission.conversation(request.conversation_ref()).unwrap().messages.len(), 1 + usize::from(record.state == ActionInvocationState::Completed));
        assert_eq!(state.admission.runnable_invocation_refs().len(), usize::from(record.state == ActionInvocationState::Completed));
    }
}

#[test]
fn person_response_uses_preferred_binding_and_current_host_scope() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = person_workspace(temp.path(), true);
    let person = load_default_local_acting_actor_ref(&workspace).unwrap();
    let runtime = LocalRuntime::open_with_state_path(&workspace, temp.path().join("state.sqlite"), LocalHostIdentity::new(&person, "cwp://example.test/integrations/host")).unwrap();
    let helper = runtime.catalog().unwrap().actors.iter().find(|a| a.handle == "@helper").unwrap().actor_ref.clone();
    let request = runtime.submit_text_as("cwp://example.test/integrations/host", "one", &helper, None, "@person Question").unwrap();
    let secondary = {
        let mut state = runtime.lock().unwrap();
        let first = state.graph.actor_integrations.iter_mut().find(|b| b.actor_ref == helper).unwrap();
        first.adapter_ref = "cwp://example/adapters/unavailable".into();
        let primary = first.actor_integration_ref.clone();
        let mut second = first.clone();
        second.actor_integration_ref = format!("{helper}/bindings/secondary");
        second.adapter_ref = crate::adapter::LOOPBACK_ADAPTER_REF.into();
        let secondary = second.actor_integration_ref.clone();
        state.graph.actor_integrations.push(second);
        state.graph.actors.iter_mut().find(|a| a.actor_ref == helper).unwrap().message_binding_preference = vec![primary, secondary.clone()];
        secondary
    };
    runtime.reply_person_request_in_upstream_scope(&person, request.conversation_ref(), request.invocation_ref().unwrap(), "Answer".into(), Some("window-one")).unwrap();
    let state = runtime.lock().unwrap();
    let next = state.admission.invocation_records().find(|r| r.is_runnable()).unwrap();
    assert_eq!(next.actor_integration_ref.as_ref(), Some(&secondary));
    assert_eq!(state.host_local_invocation_owners.get(&next.invocation_ref).map(String::as_str), Some("window-one"));
}

#[test]
fn person_response_keeps_request_when_continuation_route_disappears_and_can_stop_conversation() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = person_workspace(temp.path(), true);
    let person = load_default_local_acting_actor_ref(&workspace).unwrap();
    let runtime = LocalRuntime::open_with_state_path(&workspace, temp.path().join("state.sqlite"), LocalHostIdentity::new(&person, "cwp://example.test/integrations/host")).unwrap();
    let helper = runtime.catalog().unwrap().actors.iter().find(|a| a.handle == "@helper").unwrap().actor_ref.clone();
    let request = runtime.submit_text_as("cwp://example.test/integrations/host", "one", &helper, None, "@person Question").unwrap();
    {
        let mut state = runtime.lock().unwrap();
        let actor = state.graph.actors.iter_mut().find(|a| a.actor_ref == helper).unwrap();
        actor.default_actor_integration_ref = None;
        actor.message_binding_preference.clear();
    }
    assert!(runtime.reply_person_request(&person, request.conversation_ref(), request.invocation_ref().unwrap(), "Answer".into()).is_err());
    assert_eq!(runtime.conversation(request.conversation_ref()).unwrap().unwrap().messages.len(), 1);
    assert_eq!(runtime.lock().unwrap().admission.invocation_record(request.invocation_ref().unwrap()).unwrap().state, ActionInvocationState::AwaitingPerson);
    runtime.stop_conversation_as(request.conversation_ref(), &person, None).unwrap();
    assert!(runtime.lock().unwrap().admission.invocation_record(request.invocation_ref().unwrap()).unwrap().state.is_terminal());
}

fn provider_question_call(runtime: &LocalRuntime, caller: &ActionInvocationRecord, key: &str, action: &str) -> CwrResult<Value> {
    let call = AdapterCall {invocation_ref:caller.invocation_ref.clone(),call_id:format!("{key}-{action}"),operation:crate::adapter::user_input::USER_INPUT_OPERATION.into(),
        input:json!({"key":key,"action":action,"questions":[{"id":"color","header":"Color","question":"Which color?","isOther":true,"isSecret":false,"options":[{"label":"Blue","description":"Ocean"}]}]})};
    let AdapterHostCallOutcome::Returned(value) = runtime.handle_provider_question(&call, caller)?;
    Ok(value)
}

#[test]
fn provider_question_routes_answers_to_live_request_without_new_coworker_turn() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = person_workspace(temp.path(), true);
    let person = load_default_local_acting_actor_ref(&workspace).unwrap();
    let runtime = LocalRuntime::open_with_state_path(&workspace, temp.path().join("state.sqlite"), LocalHostIdentity::new(&person,"cwp://example.test/integrations/host")).unwrap();
    let root = runtime.admit_text(None,"@helper ask a question").unwrap();
    let caller = {
        let mut state = runtime.lock().unwrap();
        let ConversationInvocationClaimOutcome::Claimed(claim) = state.admission.claim_invocation(root.invocation_ref().unwrap(),10).unwrap() else {panic!("claim")};
        claim.claim.invocation
    };
    assert_eq!(provider_question_call(&runtime,&caller,"one","start").unwrap()["state"],"pending");
    assert_eq!(provider_question_call(&runtime,&caller,"two","start").unwrap()["state"],"pending");
    let requests = runtime.lock().unwrap().admission.invocation_records().filter(|r| r.state == ActionInvocationState::AwaitingPerson).cloned().collect::<Vec<_>>();
    assert_eq!(requests.len(),2);
    let first = &requests[0];
    assert_eq!(first.requester_actor_ref,caller.target_actor_ref);
    let answers = json!({"answers":{"color":{"answers":["Blue"]}}}).to_string();
    assert!(runtime.reply_person_request(&caller.target_actor_ref,root.conversation_ref(),&first.invocation_ref,answers.clone()).is_err());
    assert!(runtime.reply_person_request(&person,root.conversation_ref(),&first.invocation_ref,"{\"answers\":{}}".into()).is_err());
    let reply = runtime.reply_person_request(&person,root.conversation_ref(),&first.invocation_ref,answers.clone()).unwrap();
    assert_eq!(runtime.reply_person_request(&person,root.conversation_ref(),&first.invocation_ref,answers).unwrap().record,reply.record);
    let states = [provider_question_call(&runtime,&caller,"one","poll").unwrap(),provider_question_call(&runtime,&caller,"two","poll").unwrap()];
    assert_eq!(states.iter().filter(|s| s["state"] == "answered").count(),1);
    assert_eq!(states.iter().find(|s|s["state"]=="answered").unwrap()["response"]["answers"]["color"]["answers"],json!(["Blue"]));
    assert_eq!(runtime.lock().unwrap().admission.invocation_records().count(),3,"no new coworker turn");
    runtime.stop_invocation_as(&caller.invocation_ref,&person,None).unwrap();
    assert!(!runtime.lock().unwrap().admission.invocation_records().any(|r|r.state==ActionInvocationState::AwaitingPerson));
    assert!(runtime.reply_person_request(&person,root.conversation_ref(),&requests[1].invocation_ref,json!({"answers":{"color":{"answers":["Red"]}}}).to_string()).is_err());
}

#[test]
fn provider_question_resolved_and_restart_remove_stale_requests() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = person_workspace(temp.path(), true);
    let person = load_default_local_acting_actor_ref(&workspace).unwrap();
    let identity = LocalHostIdentity::new(&person,"cwp://example.test/integrations/host");
    let path = temp.path().join("state.sqlite");
    let runtime = LocalRuntime::open_with_state_path(&workspace,&path,identity.clone()).unwrap();
    let root = runtime.admit_text(None,"@helper ask a question").unwrap();
    let caller = {
        let mut state = runtime.lock().unwrap();
        let ConversationInvocationClaimOutcome::Claimed(claim) = state.admission.claim_invocation(root.invocation_ref().unwrap(),10).unwrap() else {panic!("claim")};
        claim.claim.invocation
    };
    provider_question_call(&runtime,&caller,"resolved","start").unwrap();
    assert_eq!(provider_question_call(&runtime,&caller,"resolved","cancel").unwrap()["state"],"resolved");
    assert_eq!(provider_question_call(&runtime,&caller,"resolved","poll").unwrap()["state"],"resolved");
    provider_question_call(&runtime,&caller,"restart","start").unwrap();
    drop(runtime);
    let runtime = LocalRuntime::open_with_state_path(&workspace,&path,identity).unwrap();
    assert!(!runtime.lock().unwrap().admission.invocation_records().any(|r|r.state==ActionInvocationState::AwaitingPerson));
}

#[test]
fn experimental_inbox_settings_create_persist_reload_and_preserve_pending() {
    use crate::v2026_09::local::experimental_features::{Report, Request};
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("workspace");
    copy_workspace(fixture_workspace().parent().unwrap(), &directory);
    let workspace = directory.join("CWR.toml");
    let person = load_default_local_acting_actor_ref(&workspace).unwrap();
    let runtime = LocalRuntime::open_with_state_path(&workspace, temp.path().join("state.sqlite"), LocalHostIdentity::new(&person, "cwp://org.coworkerprotocol.local/integrations/test")).unwrap();
    let manage = |request: Request| -> Result<Report, String> {
        let (activation, input) = match request.enabled {
            Some(enabled) => ("@cwr--set--experimental_features", json!({"enabled": enabled, "revision": request.revision})),
            None => ("@cwr--read--experimental_features", json!({})),
        };
        let admission = runtime.admit_action(None, activation, input).unwrap();
        let mut task = runtime.try_schedule_execution().unwrap().unwrap();
        assert_eq!(task.record().invocation_ref, admission.invocation_ref().unwrap());
        task.execute_with_model_output(&AdapterModelOutputMailbox::discard(), |_| {}).unwrap();
        let state = runtime.lock().unwrap();
        let response = state.admission.invocation_record(admission.invocation_ref().unwrap()).unwrap().terminal_response().unwrap();
        if response.status.as_u16() >= 400 { return Err(format!("{:?}", response.error)); }
        serde_json::from_value(response.result.clone().unwrap()).map_err(|e| e.to_string())
    };
    let inspect = || manage(Request { path: workspace.clone(), actor_ref: person.clone(), enabled: None, revision: None }).unwrap();
    let initial = inspect();
    assert_eq!(initial.key, "desktop.human_reply_inbox");
    assert!(!initial.enabled);
    let original = fs::read_to_string(directory.join("coworkers/person/COWORKER.toml")).unwrap();
    let enabled = manage(Request { path: workspace.clone(), actor_ref: person.clone(), enabled: Some(true), revision: Some(initial.revision.clone()) }).unwrap();
    assert!(enabled.enabled);
    assert!(fs::read_to_string(directory.join("coworkers/person/COWORKER.toml")).unwrap().starts_with(&original));
    assert!(inspect().enabled);
    let helper = runtime.catalog().unwrap().actors.iter().find(|a| a.handle == "@helper").unwrap().actor_ref.clone();
    let pending = runtime.submit_text_as("cwp://org.coworkerprotocol.local/integrations/test", "inbox-on", &helper, None, "@person Question").unwrap();
    assert!(pending.invocation_ref().is_some());
    assert!(manage(Request { path: workspace.clone(), actor_ref: person.clone(), enabled: Some(false), revision: Some(initial.revision) }).is_err());
    let integration = directory.join("coworkers/person/inbox-integration.toml");
    let content = fs::read_to_string(&integration).unwrap();
    fs::write(&integration, format!("# Keep this comment\n{content}")).unwrap();
    let fresh = inspect();
    manage(Request { path: workspace.clone(), actor_ref: person.clone(), enabled: Some(false), revision: Some(fresh.revision) }).unwrap();
    assert!(fs::read_to_string(&integration).unwrap().starts_with("# Keep this comment"));
    assert!(!inspect().enabled);
    let off = runtime.submit_text_as("cwp://org.coworkerprotocol.local/integrations/test", "inbox-off", &helper, Some(pending.conversation_ref()), "@person Another question").unwrap();
    assert!(off.invocation_ref().is_none());
    runtime.reply_person_request(&person, pending.conversation_ref(), pending.invocation_ref().unwrap(), "Still answerable".into()).unwrap();
}

#[test]
fn experimental_inbox_rejects_a_change_between_save_and_apply() {
    use crate::v2026_09::local::experimental_features::{manage, Request};
    let temp = tempfile::tempdir().unwrap();
    let workspace = person_workspace(temp.path(), false);
    let person = load_default_local_acting_actor_ref(&workspace).unwrap();
    let runtime = LocalRuntime::open_with_state_path(&workspace, temp.path().join("state.sqlite"), LocalHostIdentity::new(&person, "cwp://org.example/integrations/test")).unwrap();
    let read = || manage(Request { path: workspace.clone(), actor_ref: person.clone(), enabled: None, revision: None }).unwrap();
    let initial = read();
    let saved = manage(Request { path: workspace.clone(), actor_ref: person.clone(), enabled: Some(true), revision: Some(initial.revision) }).unwrap();
    let integration = workspace.parent().unwrap().join("coworkers/person/integration.toml");
    fs::write(&integration, fs::read_to_string(&integration).unwrap().replace("= true", "= false")).unwrap();
    let error = runtime.apply_experimental_features(&saved).unwrap_err();
    assert!(error.to_string().contains("changed before"));
    assert!(!runtime.graph().unwrap().actor_integrations.iter().any(|binding| binding.actor_ref == person));
    let current = read();
    runtime.apply_experimental_features(&current).unwrap();
    let saved = manage(Request { path: workspace.clone(), actor_ref: person.clone(), enabled: Some(true), revision: Some(current.revision) }).unwrap();
    runtime.apply_experimental_features(&saved).unwrap();
    assert!(runtime.graph().unwrap().actor_integrations.iter().any(|binding| binding.actor_ref == person));
}
