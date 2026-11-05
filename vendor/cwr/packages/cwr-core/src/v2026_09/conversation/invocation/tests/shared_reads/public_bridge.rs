//! Opt-in actual HTTP Worker coverage of the ordinary CWR read facade.
use super::*;
use cwpdb_store::{
    conversation::{NativeConversationStore, OpenOptions, Scope},
    invocation::{Authorizer, Context},
};
struct BridgeAuthority(Context);
impl Authorizer for BridgeAuthority {
    fn authorize(
        &self,
        context: &Context,
        _reference: &str,
        _write: bool,
    ) -> cwpdb_store::CwpdbResult<()> {
        if context != &self.0 {
            return Err("bridge context mismatch".into());
        }
        Ok(())
    }
}
#[test]
#[ignore = "requires CWPDB_FIXTURE_ORIGIN pointing to actual local workerd"]
fn public_cwr_facade_runs_native_declared_and_actual_worker_remote() {
    let endpoint =
        std::env::var("CWPDB_FIXTURE_ORIGIN").expect("explicit local workerd endpoint required");
    for canonical in [false, true] {
        for mode in ["native", "declared-native", "worker-remote"] {
            eprintln!("public facade: {mode}, canonical={canonical}");
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("bridge.sqlite");
            let rollout = if canonical {
                ConversationTimelineRollout::NewConversations
            } else {
                ConversationTimelineRollout::Disabled
            };
            let context = Context {
                workspace_ref: format!(
                    "cwp://com.example.fixture/workspaces/bridge-{}",
                    uuid::Uuid::new_v4()
                ),
                caller_ref: RUNTIME_ROOT.into(),
            };
            let scope = Scope {
                context: context.clone(),
                runtime_root_ref: RUNTIME_ROOT.into(),
            };
            let mut store = match mode {
                "native" => ConversationAdmissionStore::open_with_timeline_rollout(
                    &path,
                    RUNTIME_ROOT,
                    rollout,
                )
                .unwrap(),
                "declared-native" => {
                    let storage = NativeConversationStore::open(
                        &path,
                        scope.clone(),
                        OpenOptions {
                            runtime_state_version: crate::v2026_09::conversation::STATE_VERSION
                                .into(),
                            workspace_binding: None,
                            timeline_rollout: Some(
                                if canonical {
                                    "new_conversations"
                                } else {
                                    "disabled"
                                }
                                .into(),
                            ),
                        },
                    )
                    .unwrap();
                    ConversationAdmissionStore::open_declared(
                        storage,
                        scope,
                        BridgeAuthority(context),
                        rollout,
                    )
                    .unwrap()
                }
                _ => {
                    let transport = crate::cloud_execution::HttpDeclaredTransport::new(
                        &endpoint,
                        "fictional-local-cwpdb-conformance-only".into(),
                    )
                    .unwrap();
                    let storage = cwpdb_store::remote::RemoteStore::new(transport);
                    ConversationAdmissionStore::open_declared(
                        storage,
                        scope,
                        BridgeAuthority(context),
                        rollout,
                    )
                    .unwrap()
                }
            };
            store.commit(prepared_root()).unwrap();
            store.claim_invocation(ROOT_INVOCATION_REF, 11).unwrap();
            store
                .publish_root_message_completed(
                    ROOT_INVOCATION_REF,
                    ActionResponseStatus::COMPLETED,
                    json!({"text":"Bridge retained reply needle 🌵"}),
                    "Bridge retained reply needle 🌵".into(),
                    50,
                )
                .unwrap();
            assert_public_reads(&store, canonical);
            assert_selection_revision_fence(&mut store, mode == "native");
        }
    }
}

fn assert_public_reads(store: &ConversationAdmissionStore, canonical: bool) {
    let mut reader = store.read_store().unwrap();
    let operation = reader.begin_operation(Duration::from_secs(30)).unwrap();
    assert!(operation
        .conversation_current(CONVERSATION_REF)
        .unwrap()
        .active_invocations
        .is_empty());
    assert_eq!(
        operation
            .conversation_invocation(CONVERSATION_REF, ROOT_INVOCATION_REF)
            .unwrap()
            .unwrap()
            .state,
        ActionInvocationState::Completed
    );
    for unknown in [
        "missing-owner",
        "disabled-owner",
        "urn:uuid:installed-pack-owner",
        "cwp://fixture/missing#member",
    ] {
        assert!(operation.conversation_resource(unknown).unwrap().is_none());
    }
    assert!(operation
        .conversation_resource(SOURCE_MESSAGE_REF)
        .unwrap()
        .is_some());
    assert!(operation
        .root_conversation_summary(CONVERSATION_REF)
        .unwrap()
        .is_some());
    let window = ConversationRecordWindow {
        first: Some(20),
        ..Default::default()
    };
    let content = operation
        .conversation_content(CONVERSATION_REF, window)
        .unwrap();
    assert_eq!(content.content.len(), 2);
    assert!(content
        .content
        .iter()
        .any(|item| item.message.text.contains("needle")));
    assert_eq!(
        operation
            .conversation_history(CONVERSATION_REF, window)
            .unwrap()
            .content
            .len(),
        2
    );
    assert_eq!(
        operation
            .conversation_browse_page(CONVERSATION_REF, window, 512)
            .unwrap()
            .content
            .len(),
        2
    );
    assert!(!operation
        .conversation_history_around(CONVERSATION_REF, SOURCE_MESSAGE_REF, 1, 1, 512)
        .unwrap()
        .content
        .is_empty());
    assert!(!operation
        .conversation_message_excerpt(CONVERSATION_REF, SOURCE_MESSAGE_REF, 0, 0, 512)
        .unwrap()
        .text_excerpt
        .is_empty());
    assert_eq!(
        operation
            .conversation_history_search(CONVERSATION_REF, "needle", None, None, 10, 512)
            .unwrap()
            .content
            .len(),
        1
    );
    operation
        .conversation_message_links(CONVERSATION_REF, SOURCE_MESSAGE_REF, None, None, 10)
        .unwrap();
    assert!(!operation
        .conversation_timeline(CONVERSATION_REF, window)
        .unwrap()
        .anchors
        .is_empty());
    operation
        .conversation_completions(CONVERSATION_REF, RUNTIME_ROOT, 0, None, None, 20)
        .unwrap();
    if canonical {
        let page = operation
            .conversation_timeline_root_page(
                CONVERSATION_REF,
                &crate::v2026_09::conversation::ConversationTimelinePageRequest {
                    first: Some(20),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(!page.edges.is_empty());
        assert_eq!(
            operation
                .conversation_timeline_connection_count(CONVERSATION_REF, &page.connection_ref)
                .unwrap(),
            page.edges.len()
        );
        let item = &page.edges[0].item;
        assert!(operation
            .conversation_timeline_item(CONVERSATION_REF, &item.item_ref)
            .unwrap()
            .is_some());
    } else {
        assert!(!operation
            .conversation_legacy_timeline_page(
                CONVERSATION_REF,
                &ConversationRecordPageRequest {
                    first: Some(20),
                    ..Default::default()
                }
            )
            .unwrap()
            .anchors
            .is_empty());
    }
    assert!(operation
        .conversation_resource("cwp://com.example.fixture/messages/missing")
        .unwrap()
        .is_none());
    operation.cancellation().cancel();
    assert!(operation
        .conversation_content(CONVERSATION_REF, window)
        .is_err());
}

fn assert_selection_revision_fence(store: &mut ConversationAdmissionStore, frozen_native: bool) {
    let mut reader = store.read_store().unwrap();
    let operation = reader.begin_operation(Duration::from_secs(30)).unwrap();
    let window = ConversationRecordWindow {
        first: Some(20),
        ..Default::default()
    };
    assert_eq!(
        operation
            .conversation_content(CONVERSATION_REF, window)
            .unwrap()
            .total_count,
        2
    );
    store
        .append_action_result(
            CONVERSATION_REF,
            "cwp://com.example.fixture/actors/worker",
            "cwp://com.example.fixture/actors/requester",
            "@worker--draft--brief",
            json!({}),
            ConversationActionOutcome::Succeeded,
            "Concurrent retained result".into(),
            SOURCE_MESSAGE_REF,
        )
        .unwrap();
    let old = operation.conversation_content(CONVERSATION_REF, window);
    if frozen_native {
        assert_eq!(old.unwrap().total_count, 2);
    } else {
        assert!(
            old.is_err(),
            "declared selection must reject commit drift rather than mix snapshots"
        );
    }
    drop(operation);
    let fresh = reader.begin_operation(Duration::from_secs(30)).unwrap();
    assert_eq!(
        fresh
            .conversation_content(CONVERSATION_REF, window)
            .unwrap()
            .total_count,
        3
    );
}
