use super::*;
use crate::v2026_09::conversation::{ConversationRecordPageRequest, ConversationTimelineRollout};
use crate::v2026_09::local::{
    resolve_local_graph, LocalHostIdentity, LocalRuntime, LocalRuntimeOptions,
};
const CONVERSATION: &str = "cwp://com.example.runtime/conversations/legacy-paging";
const PAGE: usize = 37;

fn key(anchor: &ConversationTimelineAnchor) -> ConversationPositionKey {
    match anchor {
        ConversationTimelineAnchor::Message { key, .. } => key.clone(),
        _ => panic!("expected message"),
    }
}
#[test]
fn exported_legacy_module_pages_large_histories_with_fixed_work() {
    let mut measurements = Vec::new();
    for count in [1_201, 6_001] {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().join("CWR.toml");
        let module =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/base/conversations/CWR.toml");
        std::fs::create_dir_all(temporary.path().join("module/definitions")).unwrap();
        for name in ["CWR.toml", "service.toml", "relationships.toml"] {
            std::fs::copy(
                module.parent().unwrap().join(name),
                temporary.path().join("module").join(name),
            )
            .unwrap();
        }
        for entry in std::fs::read_dir(module.parent().unwrap().join("definitions")).unwrap() {
            let entry = entry.unwrap();
            std::fs::copy(
                entry.path(),
                temporary
                    .path()
                    .join("module/definitions")
                    .join(entry.file_name()),
            )
            .unwrap();
        }
        std::fs::write(temporary.path().join("person.toml"), "type='cwp://org.coworkerprotocol/2026-09/schemas/coworker'\nname='Person'\nslug='person'\n").unwrap();
        std::fs::write(&workspace, format!("type='cwp://org.coworkerprotocol/2026-09/schemas'\n[[imports]]\npath={:?}\nas='conversations'\n[[imports]]\npath='./person.toml'\nas='person'\n", "./module/CWR.toml")).unwrap();
        let graph = resolve_local_graph(&workspace).unwrap();
        let person = graph
            .actors
            .iter()
            .find(|a| a.actor_name.as_deref() == Some("Person"))
            .unwrap()
            .actor_ref
            .clone();
        let path = temporary.path().join("retained.sqlite");
        let mut writer = ConversationAdmissionStore::open(&path, &graph.runtime_ref).unwrap();
        let mut state = writer.state.clone();
        state.conversations = vec![serde_json::from_value(serde_json::json!({
            "conversation_ref":CONVERSATION, "participant_actor_refs":[person],
            "messages":(0..count).map(|index| serde_json::json!({
                "message_ref":format!("{CONVERSATION}/messages/{index:06}"),
                "author_actor_ref":person,"addressed_actor_refs":[],"text":format!("Message {index:06}")
            })).collect::<Vec<_>>()
        })).unwrap()].into();
        writer.replace_state(state).unwrap();
        drop(writer);
        let runtime = LocalRuntime::open_with_state_path_and_runtime_ref_and_options(
            &workspace,
            &path,
            &graph.runtime_ref,
            LocalHostIdentity::new(&person, "cwp://example.test/ingress"),
            LocalRuntimeOptions {
                conversation_timeline_rollout: ConversationTimelineRollout::Disabled,
                ..Default::default()
            },
        )
        .unwrap();
        let service = runtime.conversation_module().unwrap();
        assert_eq!(
            service
                .timeline_layout(CONVERSATION, Instant::now() + Duration::from_secs(10))
                .unwrap(),
            None
        );
        let (first, steps) = super::super::step_measurement::measure(|| {
            service
                .legacy_timeline(
                    CONVERSATION,
                    &ConversationRecordPageRequest {
                        first: Some(PAGE),
                        ..Default::default()
                    },
                    Instant::now() + Duration::from_secs(10),
                )
                .unwrap()
        });
        assert_eq!(first.anchors.len(), PAGE);
        measurements.push((count, steps, serde_json::to_vec(&first).unwrap().len()));
        for backwards in [false, true] {
            let mut boundary = None;
            let mut observed = Vec::new();
            let mut identities = std::collections::BTreeSet::new();
            loop {
                let request = if backwards {
                    ConversationRecordPageRequest {
                        before: boundary.clone(),
                        last: Some(PAGE),
                        ..Default::default()
                    }
                } else {
                    ConversationRecordPageRequest {
                        after: boundary.clone(),
                        first: Some(PAGE),
                        ..Default::default()
                    }
                };
                let page = service
                    .legacy_timeline(
                        CONVERSATION,
                        &request,
                        Instant::now() + Duration::from_secs(10),
                    )
                    .unwrap();
                assert!(!page.anchors.is_empty());
                for anchor in &page.anchors {
                    let ConversationTimelineAnchor::Message { message_ref, .. } = anchor else {
                        panic!("expected message")
                    };
                    assert!(identities.insert(message_ref.clone()), "duplicate message");
                }
                if backwards {
                    observed.extend(page.anchors.iter().rev().map(|anchor| key(anchor).position));
                    boundary = page.anchors.first().map(key);
                    if !page.has_previous_page {
                        break;
                    }
                } else {
                    observed.extend(page.anchors.iter().map(|anchor| key(anchor).position));
                    boundary = page.anchors.last().map(key);
                    if !page.has_next_page {
                        break;
                    }
                }
            }
            let mut expected = (0..count as i64).collect::<Vec<_>>();
            if backwards {
                expected.reverse();
            }
            assert_eq!(observed, expected);
            assert_eq!(identities.len(), count);
        }
        let ConversationTimelineAnchor::Message { key, message_ref } = &first.anchors[0] else {
            panic!("expected message")
        };
        let message = service
            .legacy_message(
                CONVERSATION,
                key,
                message_ref,
                Instant::now() + Duration::from_secs(10),
            )
            .unwrap()
            .unwrap();
        assert_eq!(message.message_ref, *message_ref);
        assert_eq!(message.author_actor_ref, person);
        assert_eq!(message.text, "Message 000000");
    }
    eprintln!("exported legacy module page measurements: {measurements:?}");
    assert!(
        measurements[1].1 <= measurements[0].1 + 500,
        "legacy page work grew: {measurements:?}"
    );
    assert_eq!(measurements[0].2, measurements[1].2);
}
