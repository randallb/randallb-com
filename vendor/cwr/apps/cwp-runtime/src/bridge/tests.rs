use super::*;
use cwr::{
    open_responses::v2026_04_24::AssistantPhase,
    v2026_09::{conversation::RetainedConversation, host::ConversationLiveOutput},
};
use std::{io::Write, net::Shutdown, os::unix::net::UnixListener};

const SOURCE_REF: &str = "cwp://example.test/runtime/sources/conversations";

#[test]
fn desktop_projection_keeps_only_root_conversations() {
    let root = conversation("root", None);
    let child = conversation("child", Some(&root.conversation_ref));

    let snapshot = root_conversation_snapshot(SOURCE_REF, vec![root.clone(), child.clone()]);
    let HostEvent::ConversationSnapshot { conversations, .. } = snapshot else {
        panic!("projection must produce a conversation snapshot")
    };
    assert_eq!(conversations, vec![root]);

    assert!(root_conversation_upsert(SOURCE_REF, child).is_none());
    root_conversation_upsert(SOURCE_REF, conversations[0].clone())
        .expect("root upsert remains host-visible")
        .validate()
        .expect("root upsert validates at the host boundary");
}

#[test]
fn completion_metadata_is_emitted_only_for_version_two_reads() {
    let page = cwr::v2026_09::conversation::ConversationSummaryPage {
        membership_revision: "membership:v1:stable".into(),
        revision: "1".into(),
        summaries: vec![cwr::v2026_09::conversation::ConversationSummary {
            completed_work: vec![cwr::v2026_09::conversation::CompletedWorkSummary {
                invocation_ref: "cwp://example.test/runtime/invocations/one".into(),
                sequence: 1,
            }],
            key: cwr::v2026_09::conversation::ConversationSummaryKey {
                creation_order: 0,
                conversation_ref: "cwp://example.test/runtime/conversations/one".into(),
            },
            title: None,
            archived: false,
            last_message_ref: None,
            last_message_preview: None,
            completed_work_refs: vec!["cwp://example.test/runtime/invocations/one".into()],
            revision: "1".into(),
        }],
        has_next_page: false,
        has_previous_page: false,
        total_count: 1,
        archived_count: 0,
    };

    let legacy = pending::registry();
    pending::reserve_request(&legacy, "legacy", None, false).unwrap();
    let legacy_event =
        project_root_conversations_page(&legacy, SOURCE_REF, "legacy".into(), page.clone())
            .unwrap()
            .unwrap();
    let legacy_json = serde_json::to_value(&legacy_event).unwrap();
    assert!(legacy_json["page"].get("revision").is_none());
    assert!(legacy_json["page"].get("membership_revision").is_none());
    assert!(legacy_json["page"]["summaries"][0]
        .get("completed_work")
        .is_none());
    assert!(
        serde_json::to_value(legacy_event).unwrap()["page"]["summaries"][0]
            .get("completed_work_refs")
            .is_none()
    );

    let current = pending::registry();
    pending::reserve_request(&current, "current", None, true).unwrap();
    let current_event =
        project_root_conversations_page(&current, SOURCE_REF, "current".into(), page.clone())
            .unwrap()
            .unwrap();
    assert_eq!(
        serde_json::to_value(&current_event).unwrap()["page"]["summaries"][0]
            ["completed_work_refs"][0],
        "cwp://example.test/runtime/invocations/one"
    );
    let current_json = serde_json::to_value(current_event).unwrap();
    assert!(current_json["page"].get("revision").is_none());
    assert!(current_json["page"].get("membership_revision").is_none());
    assert!(current_json["page"]["summaries"][0]
        .get("completed_work")
        .is_none());
    let sidebar = pending::registry();
    pending::reserve_request(&sidebar, "sidebar", None, true).unwrap();
    pending::mark_sidebar_metadata(&sidebar, "sidebar").unwrap();
    let event = project_root_conversations_page(&sidebar, SOURCE_REF, "sidebar".into(), page)
        .unwrap()
        .unwrap();
    let json = serde_json::to_value(event).unwrap();
    assert_eq!(json["page"]["revision"], "1");
    assert_eq!(json["page"]["membership_revision"], "membership:v1:stable");
    assert_eq!(
        json["page"]["summaries"][0]["completed_work"][0]["sequence"],
        1
    );
}

#[test]
fn runtime_live_output_is_already_a_valid_desktop_projection() {
    let event = HostEvent::ConversationLiveOutput {
        source_ref: SOURCE_REF.into(),
        conversation_ref: "cwp://example.test/runtime/conversations/one".into(),
        output: Some(ConversationLiveOutput {
            invocation_ref: "cwp://example.test/runtime/invocations/one".into(),
            generation: 1,
            target_actor_ref: crate::test_refs::actor_ref("helper").as_str().into(),
            phase: AssistantPhase::Commentary,
            text: "Reading the workspace".into(),
        }),
    };

    event
        .validate()
        .expect("runtime live output validates at the Desktop host boundary");
    let value = serde_json::to_value(event).expect("encode Desktop live output");
    assert!(value["output"].get("provider").is_none());
}

#[test]
fn request_registry_rejects_duplicate_in_flight_ids() {
    let pending = pending::registry();
    pending::reserve_request(&pending, "same-request", None, false).expect("reserve first request");

    let error =
        pending::reserve_request(&pending, "same-request", Some("conversation".into()), false)
            .expect_err("duplicate request must be rejected");

    assert!(error.to_string().contains("already in flight"));
    assert_eq!(
        pending::conversation_ref(&pending, "same-request"),
        Some(None)
    );

    let error = pending::reserve_request(&pending, INTERNAL_SNAPSHOT_REQUEST_ID, None, false)
        .expect_err("internal snapshot request ID must be reserved");
    assert!(error.to_string().contains("is reserved"));
    let error =
        pending::reserve_request(&pending, INTERNAL_DESKTOP_SNAPSHOT_REQUEST_ID, None, false)
            .expect_err("internal Desktop snapshot request ID must be reserved");
    assert!(error.to_string().contains("is reserved"));
    let error = pending::reserve_request(&pending, INTERNAL_TRANSFER_PROBE_REQUEST_ID, None, false)
        .expect_err("internal transfer probe request ID must be reserved");
    assert!(error.to_string().contains("is reserved"));
}

#[test]
fn identity_probe_request_id_is_reserved() {
    let error = pending::reserve_request(
        &pending::registry(),
        INTERNAL_BUILD_IDENTITY_REQUEST_ID,
        None,
        false,
    )
    .expect_err("identity probe ID must not be accepted from a host");
    assert!(error.to_string().contains("is reserved"));
}

#[test]
fn non_desktop_ready_omits_build_identity_for_both_transfer_outcomes() {
    for transfer in [
        RuntimeServiceEvent::DrainStatus {
            request_id: INTERNAL_TRANSFER_PROBE_REQUEST_ID.into(),
            remaining: 0,
        },
        RuntimeServiceEvent::CommandFailed {
            request_id: INTERNAL_TRANSFER_PROBE_REQUEST_ID.into(),
            error: "unsupported".into(),
        },
    ] {
        let source = projected_source_for_host(
            vec![
                RuntimeServiceEvent::BuildIdentity {
                    request_id: INTERNAL_BUILD_IDENTITY_REQUEST_ID.into(),
                    identity: crate::build_identity::current(),
                },
                transfer,
            ],
            false,
            None,
        );
        let wire = serde_json::to_value(source).unwrap();
        assert!(wire.get("build_identity").is_none());
    }
}

#[test]
fn indexed_history_hosts_use_metadata_bootstrap() {
    assert!(uses_metadata_bootstrap(WORKLOOP_DESKTOP_INGRESS_REF));
    assert!(uses_metadata_bootstrap(
        "cwp://org.coworkerprotocol.local/integrations/tui"
    ));
    assert!(!uses_metadata_bootstrap(
        "cwp://org.coworkerprotocol.local/integrations/host-stdio"
    ));
    assert!(!uses_metadata_bootstrap(HOST_SOCKET_INGRESS_REF));
}

#[test]
fn request_registry_bounds_unanswered_host_commands() {
    let pending = pending::registry();
    for index in 0..pending::MAX_IN_FLIGHT_HOST_REQUESTS {
        pending::reserve_request(&pending, &format!("request-{index}"), None, false)
            .expect("reserve within the in-flight limit");
    }

    let error = pending::reserve_request(&pending, "one-too-many", None, false)
        .expect_err("in-flight request limit must be enforced");

    assert_eq!(
        error.downcast_ref::<io::Error>().unwrap().kind(),
        io::ErrorKind::WouldBlock
    );
    assert!(error.to_string().contains("requests in flight"));
    assert_eq!(pending::len(&pending), pending::MAX_IN_FLIGHT_HOST_REQUESTS);
}

#[test]
fn bridge_returns_when_the_runtime_connection_closes() {
    let temporary = tempfile::tempdir().expect("temporary runtime socket");
    let socket_path = temporary.path().join("runtime.sock");
    let listener = UnixListener::bind(&socket_path).expect("bind fake runtime service");
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept runtime bridge");
        let mut reader = BufReader::new(stream);
        let mut identity_probe = String::new();
        reader
            .read_line(&mut identity_probe)
            .expect("read identity query");
        assert!(matches!(
            serde_json::from_str::<RuntimeServiceCommand>(identity_probe.trim()).unwrap(),
            RuntimeServiceCommand::Drain {operation, ..} if operation == "supports-build-identity"
        ));
        let mut fork_probe = String::new();
        reader
            .read_line(&mut fork_probe)
            .expect("read bridge fork probe");
        assert!(matches!(
            serde_json::from_str::<RuntimeServiceCommand>(fork_probe.trim())
                .expect("parse bridge fork probe"),
            RuntimeServiceCommand::Drain { operation, .. }
                if operation == "supports-conversation-fork"
        ));

        let mut transfer_probe = String::new();
        reader
            .read_line(&mut transfer_probe)
            .expect("read bridge transfer probe");
        assert!(matches!(
            serde_json::from_str::<RuntimeServiceCommand>(&transfer_probe).unwrap(),
            RuntimeServiceCommand::Drain { operation, .. }
                if operation == "supports-conversation-transfer"
        ));
        let mut snapshot = String::new();
        reader
            .read_line(&mut snapshot)
            .expect("read bridge snapshot request");
        assert!(matches!(
            serde_json::from_str::<RuntimeServiceCommand>(&snapshot).unwrap(),
            RuntimeServiceCommand::Snapshot {
                metadata_only: true,
                ..
            }
        ));
        let mut selection = String::new();
        reader
            .read_line(&mut selection)
            .expect("read bridge actor selection");
        assert!(matches!(
            serde_json::from_str::<RuntimeServiceCommand>(&selection).unwrap(),
            RuntimeServiceCommand::InspectActor { .. }
        ));
    });
    let (input_sender, input) = mpsc::channel();

    let error = run_with_input(
        &socket_path,
        "cwp://example.test/runtime",
        WORKLOOP_DESKTOP_INGRESS_REF,
        Some(crate::test_refs::actor_ref("person").as_str()),
        None,
        input,
        output::host_writer_for(io::sink()).unwrap(),
    )
    .expect_err("runtime socket loss must end the bridge");

    drop(input_sender);
    server.join().expect("fake runtime server");
    assert!(
        error
            .to_string()
            .contains("runtime service connection closed"),
        "{error}"
    );
}

#[derive(Clone)]
struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

impl Write for SharedBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn projected_source_for_host(
    probe_responses: Vec<RuntimeServiceEvent>,
    advertise_fork_capability: bool,
    performance_database_path: Option<std::path::PathBuf>,
) -> ConversationSource {
    let (client, mut server) = UnixStream::pair().expect("runtime event wire pair");
    let service = thread::spawn(move || {
        serde_json::to_writer(
            &mut server,
            &RuntimeServiceEvent::Ready {
                runtime_ref: "cwp://example.test/runtime".into(),
            },
        )
        .unwrap();
        server.write_all(b"\n").unwrap();
        for response in probe_responses {
            serde_json::to_writer(&mut server, &response).unwrap();
            server.write_all(b"\n").unwrap();
        }
        server.shutdown(Shutdown::Write).unwrap();
    });
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let output = output::host_writer_for(SharedBuffer(Arc::clone(&bytes))).unwrap();

    read_runtime_events(
        client,
        output,
        pending::registry(),
        "cwp://example.test/runtime".into(),
        advertise_fork_capability,
        performance_database_path,
    )
    .expect_err("event reader ends after the fake runtime closes");
    service.join().unwrap();

    let output = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
    let ready = output
        .lines()
        .map(|line| serde_json::from_str::<HostEvent>(line).unwrap())
        .find_map(|event| match event {
            HostEvent::Ready { sources } => Some(sources),
            _ => None,
        })
        .expect("bridge publishes ready after the transfer probe");
    ready.into_iter().next().expect("bridge has one source")
}

fn projected_capabilities(
    probe_responses: Vec<RuntimeServiceEvent>,
) -> ConversationSourceCapabilities {
    projected_source_for_host(probe_responses, true, None).capabilities
}

fn projected_capabilities_for_host(
    probe_responses: Vec<RuntimeServiceEvent>,
    advertise_fork_capability: bool,
) -> ConversationSourceCapabilities {
    projected_source_for_host(probe_responses, advertise_fork_capability, None).capabilities
}

#[test]
fn transfer_probe_negotiates_new_and_legacy_residents() {
    let current = projected_capabilities(vec![
        RuntimeServiceEvent::DrainStatus {
            request_id: INTERNAL_FORK_PROBE_REQUEST_ID.into(),
            remaining: 0,
        },
        RuntimeServiceEvent::DrainStatus {
            request_id: INTERNAL_TRANSFER_PROBE_REQUEST_ID.into(),
            remaining: 0,
        },
    ]);
    assert!(current.can_transfer);
    assert!(current.can_fork);
    assert!(current.can_read_timeline);

    let transfer_only = projected_capabilities(vec![
        RuntimeServiceEvent::CommandFailed {
            request_id: INTERNAL_FORK_PROBE_REQUEST_ID.into(),
            error: "unknown runtime drain operation".into(),
        },
        RuntimeServiceEvent::DrainStatus {
            request_id: INTERNAL_TRANSFER_PROBE_REQUEST_ID.into(),
            remaining: 0,
        },
    ]);
    assert!(transfer_only.can_transfer);
    assert!(!transfer_only.can_fork);

    let legacy = projected_capabilities(vec![RuntimeServiceEvent::CommandFailed {
        request_id: INTERNAL_TRANSFER_PROBE_REQUEST_ID.into(),
        error: "unknown runtime drain operation".into(),
    }]);
    assert!(!legacy.can_transfer);
    assert!(!legacy.can_fork);
    assert!(legacy.can_read_timeline);

    let non_desktop = projected_capabilities_for_host(
        vec![
            RuntimeServiceEvent::DrainStatus {
                request_id: INTERNAL_FORK_PROBE_REQUEST_ID.into(),
                remaining: 0,
            },
            RuntimeServiceEvent::DrainStatus {
                request_id: INTERNAL_TRANSFER_PROBE_REQUEST_ID.into(),
                remaining: 0,
            },
        ],
        false,
    );
    assert!(!non_desktop.can_fork);
}

#[test]
fn desktop_ready_source_exposes_its_local_performance_database() {
    let source = projected_source_for_host(
        vec![RuntimeServiceEvent::DrainStatus {
            request_id: INTERNAL_TRANSFER_PROBE_REQUEST_ID.into(),
            remaining: 0,
        }],
        true,
        Some(Path::new("/tmp/cwp-performance.sqlite3").into()),
    );
    assert_eq!(
        source.performance_database_path.as_deref(),
        Some("/tmp/cwp-performance.sqlite3")
    );
}

#[test]
fn stream_input_distinguishes_write_half_close_from_disconnect() {
    let (proxy, resident) = UnixStream::pair().unwrap();
    let graceful = start_stream_input_reader(resident).unwrap();
    proxy.shutdown(Shutdown::Write).unwrap();
    assert!(matches!(
        graceful.recv().unwrap(),
        HostInput::Closed(Ok(()))
    ));

    let (client, resident) = UnixStream::pair().unwrap();
    let disconnected = start_stream_input_reader(resident).unwrap();
    drop(client);
    assert!(matches!(
        disconnected.recv().unwrap(),
        HostInput::Disconnected(Ok(()))
    ));
}

#[test]
fn runtime_event_reader_accepts_a_terminal_frame_over_the_host_input_limit() {
    let (client, mut server) = UnixStream::pair().expect("runtime event wire pair");
    let request_id = "large-runtime-terminal";
    let event = RuntimeServiceEvent::CommandFailed {
        request_id: request_id.into(),
        error: "x".repeat(crate::jsonl::MAX_JSONL_LINE_BYTES + 1),
    };
    let service = thread::spawn(move || {
        serde_json::to_writer(&mut server, &event).unwrap();
        server.write_all(b"\n").unwrap();
        server.shutdown(Shutdown::Write).unwrap();
    });
    let pending = pending::registry();
    pending::reserve_request(&pending, request_id, None, false).unwrap();
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let output = output::host_writer_for(SharedBuffer(Arc::clone(&bytes))).unwrap();

    let error = read_runtime_events(
        client,
        output,
        Arc::clone(&pending),
        "cwp://example.test/runtime".into(),
        true,
        None,
    )
    .expect_err("event reader ends after the oversized terminal frame");
    service.join().unwrap();

    assert!(error
        .to_string()
        .contains("runtime service connection closed"));
    assert!(pending::is_empty(&pending).unwrap());
    assert!(bytes.lock().unwrap().len() > crate::jsonl::MAX_JSONL_LINE_BYTES);
}

#[test]
fn runtime_event_reader_reports_a_partial_frame_as_transport_truncation() {
    let (client, mut server) = UnixStream::pair().expect("runtime event wire pair");
    server
        .write_all(br#"{\"kind\":\"command_failed\",\"request_id\":\"partial\""#)
        .unwrap();
    server.shutdown(Shutdown::Write).unwrap();

    let error = read_runtime_events(
        client,
        output::host_writer_for(io::sink()).unwrap(),
        pending::registry(),
        "cwp://example.test/runtime".into(),
        true,
        None,
    )
    .expect_err("partial runtime frame must end the bridge");

    assert_eq!(
        error.downcast_ref::<io::Error>().unwrap().kind(),
        io::ErrorKind::UnexpectedEof
    );
    assert!(
        error
            .to_string()
            .contains("runtime service connection closed with an incomplete JSONL frame"),
        "{error}"
    );
}

fn conversation(slug: &str, parent_conversation_ref: Option<&str>) -> RetainedConversation {
    RetainedConversation {
        owner_context_ref: None,
        conversation_ref: format!("cwp://example.test/runtime/conversations/{slug}"),
        title: None,
        archived: false,
        sidebar_eligible: None,
        parent_conversation_ref: parent_conversation_ref.map(str::to_owned),
        participant_actor_refs: vec![crate::test_refs::actor_ref("person").as_str().into()],
        messages: Vec::new(),
        timeline_connection_ref: None,
        activity: Vec::new(),
    }
}

#[test]
fn build_identity_is_from_the_resident_and_absent_for_older_residents() {
    let identity = cwr::v2026_09::host::BuildIdentity {
        version: "older-resident-version".into(),
        revision: Some("different-resident-revision".into()),
        dirty: Some(true),
        built_at: "earlier-build-time".into(),
        profile: "debug".into(),
    };
    let source = projected_source_for_host(
        vec![
            RuntimeServiceEvent::BuildIdentity {
                request_id: INTERNAL_BUILD_IDENTITY_REQUEST_ID.into(),
                identity: identity.clone(),
            },
            RuntimeServiceEvent::DrainStatus {
                request_id: INTERNAL_TRANSFER_PROBE_REQUEST_ID.into(),
                remaining: 0,
            },
        ],
        true,
        None,
    );
    assert_eq!(source.build_identity, Some(identity));
    let legacy = projected_source_for_host(
        vec![
            RuntimeServiceEvent::CommandFailed {
                request_id: INTERNAL_BUILD_IDENTITY_REQUEST_ID.into(),
                error: "unknown runtime drain operation".into(),
            },
            RuntimeServiceEvent::CommandFailed {
                request_id: INTERNAL_TRANSFER_PROBE_REQUEST_ID.into(),
                error: "unknown runtime drain operation".into(),
            },
        ],
        true,
        None,
    );
    assert_eq!(legacy.build_identity, None);
    assert!(!legacy.capabilities.can_fork);
}

mod actions;
