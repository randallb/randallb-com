fn independent_conversation_fixture() -> tempfile::TempDir {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("CWR.toml");
    let module =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/base/conversations/CWR.toml");
    // A separately authored pack imports the module through normal composition.
    fs::create_dir_all(temporary.path().join("module/definitions")).unwrap();
    for name in ["CWR.toml", "service.toml", "relationships.toml"] {
        fs::copy(
            module.parent().unwrap().join(name),
            temporary.path().join("module").join(name),
        )
        .unwrap();
    }
    for entry in fs::read_dir(module.parent().unwrap().join("definitions")).unwrap() {
        let entry = entry.unwrap();
        fs::copy(
            entry.path(),
            temporary
                .path()
                .join("module/definitions")
                .join(entry.file_name()),
        )
        .unwrap();
    }
    for (slug, name) in [("person", "Person"), ("helper", "Helper")] {
        fs::write(temporary.path().join(format!("{slug}.toml")), format!(
            "type = \"cwp://org.coworkerprotocol/2026-09/schemas/coworker\"\nname = {name:?}\nslug = {slug:?}\n")).unwrap();
    }
    fs::write(
        &workspace,
        r#"type = "cwp://org.coworkerprotocol/2026-09/schemas"
[[imports]]
path = "./module/CWR.toml"
as = "conversations"
[[imports]]
path = "./person.toml"
as = "person"
[[imports]]
path = "./helper.toml"
as = "helper"
"#,
    )
    .unwrap();
    temporary
}

fn module_page() -> crate::v2026_09::conversation::ConversationTimelinePageRequest {
    crate::v2026_09::conversation::ConversationTimelinePageRequest { first: Some(37), ..Default::default() }
}

fn module_messages(service: &LocalConversationModule<'_>, reference: &str) -> Vec<ConversationMessage> {
    let mut connections = VecDeque::from([None]);
    let mut messages = Vec::new();
    while let Some(connection) = connections.pop_front() {
        let mut request = module_page();
        loop {
            let page = service.canonical_timeline(reference, connection.as_deref(), &request, Instant::now() + Duration::from_secs(5)).unwrap();
            for edge in &page.edges {
                if let Some(child) = &edge.item.child_connection_ref { connections.push_back(Some(child.clone())); }
                if let Some(message) = service.message(reference, &edge.item.item_ref, Instant::now() + Duration::from_secs(5)).unwrap() { messages.push(message); }
            }
            if !page.page_info.has_next_page { break; }
            request.after = page.page_info.end_cursor;
        }
    }
    messages
}

#[test]
fn imported_conversations_module_runs_without_desktop_and_reopens_history() {
    let temporary = independent_conversation_fixture();
    let workspace = temporary.path().join("CWR.toml");
    let graph = resolve_local_graph(&workspace).unwrap();
    let person = graph.actors.iter().find(|actor| actor.actor_name.as_deref() == Some("Person")).unwrap().actor_ref.clone();
    let helper = graph.actors.iter().find(|actor| actor.actor_name.as_deref() == Some("Helper")).unwrap().actor_ref.clone();
    let identity = || LocalHostIdentity::new(&person, "cwp://org.coworkerprotocol.local/integrations/test");
    let path = temporary.path().join("retained.sqlite");
    let runtime = LocalRuntime::open_with_state_path(&workspace, &path, identity()).unwrap();
    reject_tampered_conversation_definition(&runtime);
    let service = runtime.conversation_module().unwrap();
    let admission = service.publish("fictional-request", None, "@cwr--list--coworkers").unwrap();
    let reference = admission.conversation_ref().to_owned();
    let original_message = admission.record.source_message_ref.clone();
    let retry = service.publish("fictional-request", None, "@cwr--list--coworkers").unwrap();
    assert_eq!(retry.record.source_message_ref, original_message);
    let page = service.canonical_timeline(&reference, None, &module_page(), Instant::now() + Duration::from_secs(2)).unwrap();
    assert_eq!(page.edges.len(), 2); // Request placement and queued work anchor.
    let messages = module_messages(&service, &reference);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].message_ref, original_message);
    let retained_count = execute_module_fixture_response(&runtime, &service, &reference, admission.invocation_ref().unwrap());
    service.set_archived(&reference, true).unwrap();
    service.set_archived(&reference, false).unwrap();
    drop(runtime);
    let reopened = LocalRuntime::open_with_state_path(&workspace, &path, identity()).unwrap();
    let retained = module_messages(&reopened.conversation_module().unwrap(), &reference);
    assert_eq!(retained.len(), retained_count);
    assert!(retained.iter().any(|message| message.progress.is_some()));
    assert_eq!(retained[0].message_ref, original_message);
    drop(reopened);
    let denied = LocalRuntime::open_with_state_path(&workspace, &path, LocalHostIdentity::new(&helper, "cwp://org.coworkerprotocol.local/integrations/test")).unwrap();
    let service = denied.conversation_module().unwrap();
    assert!(service.canonical_timeline(&reference, None, &module_page(), Instant::now()+Duration::from_secs(2)).is_err());
    assert!(service.message(&reference, &page.edges[0].item.item_ref, Instant::now()+Duration::from_secs(2)).is_err());
    assert!(service.publish("denied-request", Some(&reference), "Cannot publish").is_err());
    assert!(service.set_archived(&reference, true).is_err());
}

#[test]
fn conversations_service_requires_an_actual_module_import() {
    let (_temporary, runtime) = participation_runtime();
    assert!(runtime.conversation_module().is_err());
}

#[test]
fn conversations_service_rejects_changed_relationship_contract() {
    let temporary = independent_conversation_fixture();
    let workspace = temporary.path().join("CWR.toml");
    let graph = resolve_local_graph(&workspace).unwrap();
    let person = graph.actors.iter()
        .find(|actor| actor.actor_name.as_deref() == Some("Person"))
        .unwrap().actor_ref.clone();
    let runtime = LocalRuntime::open_with_state_path(
        &workspace, temporary.path().join("retained.sqlite"),
        LocalHostIdentity::new(&person, "cwp://example.test/ingress"),
    ).unwrap();
    assert!(runtime.conversation_module().is_ok());
    let mut state = runtime.lock().unwrap();
    let resource = state.graph.resources.iter_mut().find(|resource| {
        resource.reference == "cwp://org.coworkerprotocol/cwr/relationships/conversations"
    }).unwrap();
    resource.document["unsupported"] = json!(true);
    drop(state);
    assert!(runtime.conversation_module().is_err());
}

#[test]
fn module_publication_selection_preserves_caller_snapshot_and_cancellation() {
    let temporary = independent_conversation_fixture();
    let workspace = temporary.path().join("CWR.toml");
    let graph = resolve_local_graph(&workspace).unwrap();
    let actor = |name| graph.actors.iter()
        .find(|actor| actor.actor_name.as_deref() == Some(name))
        .unwrap().actor_ref.clone();
    let person = actor("Person");
    let helper = actor("Helper");
    let ingress = "cwp://org.coworkerprotocol.local/integrations/test";
    let runtime = LocalRuntime::open_with_state_path(
        &workspace,
        temporary.path().join("retained.sqlite"),
        LocalHostIdentity::new(&person, ingress),
    ).unwrap();
    let service = runtime.conversation_module().unwrap();
    let admission = service.publish("snapshot-first", None, "@cwr--list--coworkers").unwrap();
    let reference = admission.conversation_ref();
    let cancellation = RefCell::new(None);
    let snapshot = service.publication_snapshot_in_context(
        None, Instant::now() + Duration::from_secs(5), || false,
        |read| *cancellation.borrow_mut() = Some(read),
    ).unwrap();
    let original = snapshot.page(reference, None, &module_page(), false).unwrap().unwrap();
    service.publish("snapshot-second", Some(reference), "@cwr--list--coworkers").unwrap();
    let retained = snapshot.page(reference, None, &module_page(), false).unwrap().unwrap();
    assert_eq!(original.page.edges, retained.page.edges);
    assert_eq!(original.revision, retained.revision);
    let fresh = service.publication_snapshot_in_context(
        None, Instant::now() + Duration::from_secs(5), || false, |_| {},
    ).unwrap();
    assert!(fresh.page(reference, None, &module_page(), false).unwrap().unwrap().page.edges.len() > original.page.edges.len());
    let other = runtime.bundled_conversation_module_as(ingress, &helper).unwrap();
    let denied = other.publication_snapshot_in_context(
        None, Instant::now() + Duration::from_secs(5), || false, |_| {},
    ).unwrap();
    assert!(denied.page(reference, None, &module_page(), false).unwrap().is_none());
    cancellation.borrow().as_ref().unwrap().cancel();
    assert!(snapshot.page(reference, None, &module_page(), false).is_err());
    assert!(service.publication_snapshot_in_context(
        None, Instant::now() + Duration::from_secs(5), || true, |_| {},
    ).is_err());
    assert!(service.publication_snapshot_in_context(
        None, Instant::now() - Duration::from_secs(1), || false, |_| {},
    ).is_err());
}

#[test]
fn module_publication_selection_requires_software_authors_connection_owner() {
    let (_temporary, runtime) = participation_runtime();
    let owner = "fictional-publication-provider";
    let author = "cwp://example.test/software/authors/publication-reader";
    register_module_software_author(&runtime, owner, author);
    let service = runtime.bundled_conversation_module_as("cwp://example.test/ingress", author).unwrap();
    let admission = service.publish_message_in_context(
        "scoped-publication", None, "Fictional scoped note", Vec::new(), Some(owner), None,
    ).unwrap();
    let snapshot = service.publication_snapshot_in_context(
        Some(owner), Instant::now() + Duration::from_secs(5), || false, |_| {},
    ).unwrap();
    assert!(snapshot.conversation(admission.conversation_ref()).unwrap().is_some());
    for wrong_owner in [None, Some("another-provider")] {
        assert!(service.publication_snapshot_in_context(
            wrong_owner, Instant::now() + Duration::from_secs(5), || false, |_| {},
        ).is_err());
    }
}

fn reject_tampered_conversation_definition(runtime: &LocalRuntime) {
    // Canonical refs with incompatible declarations must not bind native code.
    {
        let mut state = runtime.lock().unwrap();
        let entity = state
            .graph
            .resources
            .iter_mut()
            .find(|resource| {
                resource.reference == "cwp://org.coworkerprotocol/cwr/entities/conversation"
            })
            .unwrap();
        let original = entity.document.clone();
        entity.document["record_kind"] = json!("unrelated-record");
        drop(state);
        assert!(runtime.conversation_module().is_err());
        let mut state = runtime.lock().unwrap();
        state
            .graph
            .resources
            .iter_mut()
            .find(|resource| {
                resource.reference == "cwp://org.coworkerprotocol/cwr/entities/conversation"
            })
            .unwrap()
            .document = original;
    }
}

fn execute_module_fixture_response(runtime: &LocalRuntime, service: &LocalConversationModule<'_>, reference: &str, invocation_ref: &str) -> usize {
    let mut task = runtime.try_schedule_execution().unwrap().unwrap();
    let mut saw_streamed_update = false;
    task.execute_with_model_output(&AdapterModelOutputMailbox::discard(), |event| {
        if matches!(event, LocalRuntimeEvent::InvocationStarted { .. }) {
            runtime.record_progress_activity(invocation_ref, 102, "Collecting fictional coworkers".into(), true, None).unwrap();
            assert!(module_messages(service, reference).iter().any(|message| message.progress.is_some()));
            saw_streamed_update = true;
        }
    }).unwrap();
    assert!(saw_streamed_update);
    let changed = module_messages(service, reference);
    assert!(changed.iter().any(|message| matches!(&message.action, Some(crate::v2026_09::conversation::ConversationActionEvent::Result { .. }))));
    changed.len()
}

#[test]
fn bundled_conversations_preserves_owner_scoped_upstream_authorship() {
    let (_temporary, runtime) = participation_runtime();
    let author = "cwp://example.test/software/authors/pack-manager";
    let owner = "fictional-provider-connection";
    register_module_software_author(&runtime, owner, author);
    assert!(!runtime
        .graph()
        .unwrap()
        .actors
        .iter()
        .any(|actor| actor.actor_ref == author));
    // This is the owner-scoped admission contract used before the module port.
    let direct = runtime
        .submit_message_in_context(
            "cwp://example.test/ingress",
            "direct-software-note",
            author,
            None,
            "Fictional software note",
            Vec::new(),
            Some(owner),
            None,
        )
        .unwrap();
    let service = runtime
        .bundled_conversation_module_as("cwp://example.test/ingress", author)
        .unwrap();
    let continued = service
        .publish_message_in_context(
            "module-software-note",
            Some(direct.conversation_ref()),
            "Follow up from software",
            Vec::new(),
            Some(owner),
            None,
        )
        .unwrap();
    assert_eq!(continued.conversation_ref(), direct.conversation_ref());
    assert_eq!(
        continued
            .conversation
            .messages
            .last()
            .unwrap()
            .author_actor_ref,
        author
    );
    // Import/binding does not make the actor available outside its connection.
    assert!(service
        .publish_message_in_context(
            "wrong-owner-software-note",
            None,
            "Unavailable connection",
            Vec::new(),
            Some("another-connection"),
            None
        )
        .is_err());
    assert!(service
        .publish("ownerless-software-note", None, "No connection scope")
        .is_err());
    let unknown = runtime
        .bundled_conversation_module_as(
            "cwp://example.test/ingress",
            "cwp://example.test/software/authors/unregistered",
        )
        .unwrap();
    assert!(unknown
        .publish_message_in_context(
            "unknown-software-note",
            None,
            "Unregistered actor",
            Vec::new(),
            Some(owner),
            None
        )
        .is_err());
}

fn register_module_software_author(runtime: &LocalRuntime, owner: &str, author: &str) {
    let (stops, _stop_events) = std::sync::mpsc::sync_channel(1);
    let session = runtime
        .register_upstream_provider(
            owner,
            vec![UpstreamCoworker {
                actor_ref: author.into(),
                coworker_ref: "cwp://example.test/software/pack-manager".into(),
                handle: "@pack_manager".into(),
                name: "Fictional pack manager".into(),
                actions: Vec::new(),
            }],
            Arc::new(|_| Ok(())),
            stops,
        )
        .unwrap();
    runtime
        .enable_upstream_coworker(
            owner,
            &session,
            author,
            crate::v2026_09::test_fixture::actor_ref("person"),
            true,
        )
        .unwrap();
}

#[path = "conversation_module_archive.rs"]
mod conversation_module_archive;

#[path = "conversation_module_legacy.rs"]
mod conversation_module_legacy;
