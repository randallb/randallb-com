use super::*;
use cwr::v2026_09::upstream::{UpstreamCoworker, UpstreamEvent};

fn copy_actor_fixture(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_actor_fixture(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[test]
fn archive_controls_reject_a_retained_actor_removed_from_the_graph() {
    let temporary = tempfile::tempdir().unwrap();
    let state = runtime_state(&temporary);
    let manifest = temporary.path().join("CWR.toml");
    let fixture = fixture_workspace().parent().unwrap().to_path_buf();
    copy_actor_fixture(&fixture, temporary.path());
    let person_import = "type = \"cwp://org.coworkerprotocol/2026-09/schemas\"\n\n[[imports]]\npath = \"./coworkers/person/COWORKER.toml\"\nas = \"person\"\n";
    fs::write(
        &manifest,
        format!(
            "{person_import}\n[[imports]]\npath = \"./coworkers/helper/coworker.toml\"\nas = \"helper\"\n"
        ),
    )
    .unwrap();
    let person = cwr::v2026_09::graph::resolve_graph(&manifest)
        .unwrap()
        .actors
        .into_iter()
        .find(|actor| actor.coworker_slug == "person")
        .unwrap()
        .actor_ref;
    let runtime = LocalRuntime::open_with_state_path_and_runtime_ref(
        &manifest,
        temporary.path().join("removed-actor.json"),
        "cwp://example.test/runtime",
        LocalHostIdentity::new(&person, "cwp://example.test/integrations/runtime"),
    )
    .unwrap();
    let helper = runtime
        .graph()
        .unwrap()
        .actors
        .into_iter()
        .find(|actor| actor.coworker_slug == "helper")
        .unwrap()
        .actor_ref;
    let root = runtime
        .submit_text_as(
            "cwp://example.test/ingress",
            "retained-helper",
            helper.as_str(),
            None,
            "@helper work",
        )
        .unwrap();
    let before = runtime
        .conversation(root.conversation_ref())
        .unwrap()
        .unwrap();
    assert!(before
        .participant_actor_refs
        .iter()
        .any(|p| p == helper.as_str()));
    fs::write(&manifest, person_import).unwrap();
    let person_source = temporary.path().join("coworkers/person/COWORKER.toml");
    let person_document = fs::read_to_string(&person_source).unwrap();
    fs::write(
        &person_source,
        person_document.split("[[imports]]").next().unwrap(),
    )
    .unwrap();
    runtime.reload_graph().unwrap();
    assert!(runtime.catalog_as(helper.as_str()).is_err());
    lock_state(&state).unwrap().runtime = runtime.clone();
    let (client, server) = wire::wire_pair();
    let responses = wire::read_events(client, 2);
    let writer = client_writer(server).unwrap();
    for command in [
        IntrinsicCwrCommand::ArchiveConversation,
        IntrinsicCwrCommand::RestoreConversation,
    ] {
        crate::server::intrinsic::execute_archive_in_background(
            &state,
            &writer,
            "removed-actor-control".into(),
            None,
            command,
            helper.as_str().into(),
            Some(root.conversation_ref().into()),
        )
        .unwrap();
    }
    assert!(responses
        .join()
        .unwrap()
        .iter()
        .all(|event| matches!(event, RuntimeServiceEvent::CommandFailed { .. })));
    assert_eq!(
        runtime
            .conversation(root.conversation_ref())
            .unwrap()
            .unwrap(),
        before
    );
}

#[test]
fn queued_archive_controls_share_one_worker_keep_activity_and_finish_in_order() {
    let temporary = tempfile::tempdir().unwrap();
    let state = runtime_state(&temporary);
    let runtime = lock_state(&state).unwrap().runtime.clone();
    let person = crate::test_refs::actor_ref("person");
    let actor = "cwp://example.test/actors/provider";
    let (sender, receiver) = std::sync::mpsc::channel();
    let (deadline, _deadline_receiver) = std::sync::mpsc::sync_channel(1);
    let session = runtime
        .register_upstream_provider(
            "connection",
            vec![UpstreamCoworker {
                actor_ref: actor.into(),
                coworker_ref: "cwp://example.test/coworkers/provider".into(),
                handle: "@provider".into(),
                name: "Provider".into(),
                actions: vec![],
            }],
            Arc::new(move |event| {
                sender
                    .send(event)
                    .map_err(|error| std::io::Error::other(error.to_string()).into())
            }),
            deadline,
        )
        .unwrap();
    runtime
        .enable_upstream_coworker("connection", &session, actor, person.as_str(), true)
        .unwrap();
    let root = runtime
        .submit_text_in_upstream_scope(
            "cwp://example.test/ingress",
            "control-work",
            person.as_str(),
            None,
            "@provider work",
            Some("connection"),
        )
        .unwrap();
    let active_runtime = runtime.clone();
    let active_root = root.clone();
    let active = thread::spawn(move || active_runtime.execute_admission(&active_root, |_| {}));
    assert!(matches!(
        receiver.recv_timeout(Duration::from_secs(2)).unwrap(),
        UpstreamEvent::WorkRequested { .. }
    ));
    let (client, server) = wire::wire_pair();
    let responses = wire::read_events(client, 65);
    let writer = client_writer(server).unwrap();
    crate::server::intrinsic::execute_archive_in_background(
        &state,
        &writer,
        "archive-first".into(),
        None,
        IntrinsicCwrCommand::ArchiveConversation,
        person.as_str().into(),
        Some(root.conversation_ref().into()),
    )
    .unwrap();
    assert!(matches!(
        receiver.recv_timeout(Duration::from_secs(2)).unwrap(),
        UpstreamEvent::StopRequested { .. }
    ));
    for index in 0..64 {
        crate::server::intrinsic::execute_archive_in_background(
            &state,
            &writer,
            format!("control-{index}"),
            None,
            if index % 2 == 0 {
                IntrinsicCwrCommand::RestoreConversation
            } else {
                IntrinsicCwrCommand::ArchiveConversation
            },
            person.as_str().into(),
            Some(root.conversation_ref().into()),
        )
        .unwrap();
    }
    {
        let locked = lock_state(&state).unwrap();
        assert_eq!(locked.conversation_controls.len(), 1);
        assert_eq!(
            locked.conversation_controls[root.conversation_ref()].len(),
            64
        );
    }
    assert!(crate::server::intrinsic::execute_archive_in_background(
        &state,
        &writer,
        "overflow".into(),
        None,
        IntrinsicCwrCommand::RestoreConversation,
        person.as_str().into(),
        Some(root.conversation_ref().into()),
    )
    .is_err());
    state.activity.exit_when_idle();
    // Even without counted invocation work or a connected socket consumer,
    // the accepted control jobs prevent the runtime's idle exit.
    assert!(!state.activity.should_exit(true, || Ok(false)).unwrap());
    runtime
        .acknowledge_upstream_stop("connection", root.invocation_ref().unwrap())
        .unwrap();
    let responses = responses.join().unwrap();
    let ids = responses
        .into_iter()
        .map(|event| {
            let RuntimeServiceEvent::CommandSucceeded { request_id, .. } = event else {
                panic!("control failed")
            };
            request_id
        })
        .collect::<Vec<_>>();
    let expected = std::iter::once("archive-first".into())
        .chain((0..64).map(|index| format!("control-{index}")))
        .collect::<Vec<_>>();
    assert_eq!(ids, expected);
    active.join().unwrap().unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !lock_state(&state).unwrap().conversation_controls.is_empty() {
        assert!(Instant::now() < deadline, "control queue did not finish");
        thread::sleep(Duration::from_millis(1));
    }
    assert!(
        runtime
            .conversation(root.conversation_ref())
            .unwrap()
            .unwrap()
            .archived
    );
    assert!(state.activity.should_exit(true, || Ok(false)).unwrap());
}
