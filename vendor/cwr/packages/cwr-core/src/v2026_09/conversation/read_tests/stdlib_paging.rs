use super::*;
use crate::v2026_09::conversation::{
    ConversationTimelineConnectionPage, ConversationTimelineDisposition,
    ConversationTimelinePageRequest,
};
use std::sync::atomic::Ordering;

const CONVERSATION: &str = "cwp://com.example.runtime/conversations/paging";
const ROOT: &str = "cwp://com.example.runtime/conversations/paging/timeline";
const MESSAGE: &str = "cwp://com.example.runtime/messages/repeated";
const PAGE: u32 = 37;

// A single declaration-backed transaction creates fictional retained history.
// Repeated content deliberately has distinct placement/publication identities.
fn retained_history(count: usize) -> (tempfile::TempDir, ConversationAdmissionStore) {
    let directory = tempfile::tempdir().unwrap();
    let mut store =
        ConversationAdmissionStore::open(directory.path().join("paging.sqlite"), RUNTIME).unwrap();
    populate_history(&mut store, count, "cwp://example.test/actors/person");
    (directory, store)
}

fn populate_history(store: &mut ConversationAdmissionStore, count: usize, participant: &str) {
    let mut state = store.state.clone();
    state.conversations = vec![serde_json::from_value(serde_json::json!({
        "conversation_ref": CONVERSATION, "timeline_connection_ref": ROOT,
        "participant_actor_refs": [participant], "messages": [{"message_ref":MESSAGE,
        "author_actor_ref":"cwp://example.test/actors/person", "addressed_actor_refs":[], "text":"Repeated content"}]
    })).unwrap()].into();
    let items = (1..=count).map(|sequence| {
        let publication = format!("{CONVERSATION}/publications/{sequence:06}");
        serde_json::json!({"item_ref":format!("{publication}/item"), "publication_ref":publication,
            "connection_ref":ROOT,"sibling_sequence":sequence,
            "content":{"kind":"message","message_ref":MESSAGE}})
    }).collect::<Vec<_>>();
    state.timeline_store = serde_json::from_value(serde_json::json!({CONVERSATION: {
        "root_connection_ref":ROOT, "connections":[{"connection_ref":ROOT,
        "owner":{"kind":"conversation","conversation_ref":CONVERSATION},
        "revision":count,"next_sequence":count+1,"items":items}]
    }}))
    .unwrap();
    store.replace_state(state).unwrap();
}

fn connection_page(
    operation: &ConversationReadOperation<'_>,
    request: ConversationTimelinePageRequest,
) -> ConversationTimelineConnectionPage {
    operation
        .conversation_timeline_connection(CONVERSATION, ROOT, &request)
        .unwrap()
}

fn exhaust(operation: &ConversationReadOperation<'_>, count: usize, backwards: bool) {
    let mut boundary = None;
    let mut sequences = Vec::new();
    let mut identities = std::collections::BTreeSet::new();
    loop {
        let page = connection_page(
            operation,
            if backwards {
                ConversationTimelinePageRequest {
                    before: boundary.clone(),
                    last: Some(PAGE),
                    ..Default::default()
                }
            } else {
                ConversationTimelinePageRequest {
                    after: boundary.clone(),
                    first: Some(PAGE),
                    ..Default::default()
                }
            },
        );
        assert!(!page.edges.is_empty());
        assert!(page.edges.len() <= PAGE as usize);
        for edge in &page.edges {
            assert!(
                identities.insert(edge.item.item_ref.clone()),
                "duplicate placement"
            );
            assert_eq!(edge.item.content.reference(), MESSAGE);
        }
        if backwards {
            sequences.extend(
                page.edges
                    .iter()
                    .rev()
                    .map(|edge| edge.item.sibling_sequence),
            );
            boundary = page.page_info.start_cursor;
            if !page.page_info.has_previous_page {
                break;
            }
        } else {
            sequences.extend(page.edges.iter().map(|edge| edge.item.sibling_sequence));
            boundary = page.page_info.end_cursor;
            if !page.page_info.has_next_page {
                break;
            }
        }
    }
    let mut expected = (1..=count as u64).collect::<Vec<_>>();
    if backwards {
        expected.reverse();
    }
    assert_eq!(sequences, expected);
    assert_eq!(identities.len(), count);
}

#[test]
fn stdlib_retained_paging_exhausts_large_histories_with_fixed_page_work() {
    let mut measurements = Vec::new();
    for count in [1_201, 6_001] {
        let (_directory, mut writer) = retained_history(count);
        let mut reader = writer.read_store().unwrap();
        let ((work, bytes), _) = step_measurement::measure(|| {
            let operation = reader.begin_operation(Duration::from_secs(30)).unwrap();
            let steps = step_measurement::current().expect("registered read work counter");
            // Exclude selection setup, matching the original page-only budget.
            // The storage-owned connection reports its actual VM work through ReadControl.
            steps.store(0, Ordering::Relaxed);
            let first = connection_page(
                &operation,
                ConversationTimelinePageRequest {
                    first: Some(PAGE),
                    ..Default::default()
                },
            );
            let work = steps.load(Ordering::Relaxed);
            let bytes = serde_json::to_vec(&first).unwrap().len();
            assert_eq!(first.edges.len(), PAGE as usize);
            assert!(work > 0, "finite storage read work must be measured");
            exhaust(&operation, count, false);
            exhaust(&operation, count, true);
            let cursor = first.page_info.end_cursor.unwrap();
            let mut bad_identity = cursor.clone();
            bad_identity.item_ref = format!("{CONVERSATION}/missing");
            let mut bad_version = cursor.clone();
            bad_version.version += 1;
            let mut bad_connection = cursor.clone();
            bad_connection.connection_ref = format!("{ROOT}/other");
            for invalid in [bad_identity, bad_version, bad_connection] {
                assert!(operation
                    .conversation_timeline_connection(
                        CONVERSATION,
                        ROOT,
                        &ConversationTimelinePageRequest {
                            after: Some(invalid),
                            first: Some(PAGE),
                            ..Default::default()
                        }
                    )
                    .is_err());
            }
            // Flat membership cursors invalidate on append, while a retained
            // connection cursor keeps its item identity and remains usable.
            let flat = operation
                .conversation_timeline(
                    CONVERSATION,
                    ConversationRecordWindow {
                        first: Some(1),
                        ..Default::default()
                    },
                )
                .unwrap();
            let flat_key = match &flat.anchors[0] {
                ConversationTimelineAnchor::Message { key, .. }
                | ConversationTimelineAnchor::Turn { key, .. } => key.clone(),
            };
            drop(operation);
            writer
                .publish_timeline_message(
                    CONVERSATION,
                    &format!("{CONVERSATION}/publications/appended"),
                    MESSAGE,
                    ConversationTimelineDisposition::Root,
                )
                .unwrap();
            let operation = reader.begin_operation(Duration::from_secs(30)).unwrap();
            assert!(operation
                .conversation_timeline(
                    CONVERSATION,
                    ConversationRecordWindow {
                        after: Some(&flat_key),
                        first: Some(1),
                        ..Default::default()
                    }
                )
                .is_err());
            let resumed = connection_page(
                &operation,
                ConversationTimelinePageRequest {
                    after: Some(cursor),
                    first: Some(PAGE),
                    ..Default::default()
                },
            );
            assert_eq!(resumed.edges[0].item.sibling_sequence, PAGE as u64 + 1);
            (work, bytes)
        });
        measurements.push((count, work, bytes));
    }
    eprintln!("retained page measurements (placements, SQLite VM steps, serialized output bytes): {measurements:?}");
    let (_, small_steps, small_bytes) = measurements[0];
    let (_, large_steps, large_bytes) = measurements[1];
    assert!(
        large_steps <= small_steps + 500,
        "fixed page work grew with history: {measurements:?}"
    );
    assert_eq!(
        small_bytes, large_bytes,
        "fixed-size page output grew with history"
    );
}

#[test]
fn exported_module_pages_large_histories_without_flattening() {
    use crate::v2026_09::local::{resolve_local_graph, LocalHostIdentity, LocalRuntime};
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
        populate_history(&mut writer, count, &person);
        drop(writer);
        let runtime = LocalRuntime::open_with_state_path_and_runtime_ref(
            &workspace,
            &path,
            &graph.runtime_ref,
            LocalHostIdentity::new(&person, "cwp://example.test/ingress"),
        )
        .unwrap();
        let service = runtime.conversation_module().unwrap();
        let request = ConversationTimelinePageRequest {
            first: Some(PAGE),
            ..Default::default()
        };
        let (first, steps) = super::super::step_measurement::measure(|| {
            service
                .canonical_timeline(
                    CONVERSATION,
                    None,
                    &request,
                    Instant::now() + Duration::from_secs(10),
                )
                .unwrap()
        });
        assert_eq!(first.edges.len(), PAGE as usize);
        measurements.push((count, steps, serde_json::to_vec(&first).unwrap().len()));
        for backwards in [false, true] {
            let mut boundary = None;
            let mut observed = Vec::new();
            let mut refs = std::collections::BTreeSet::new();
            loop {
                let request = if backwards {
                    ConversationTimelinePageRequest {
                        before: boundary.clone(),
                        last: Some(PAGE),
                        ..Default::default()
                    }
                } else {
                    ConversationTimelinePageRequest {
                        after: boundary.clone(),
                        first: Some(PAGE),
                        ..Default::default()
                    }
                };
                let page = service
                    .canonical_timeline(
                        CONVERSATION,
                        None,
                        &request,
                        Instant::now() + Duration::from_secs(10),
                    )
                    .unwrap();
                assert!(!page.edges.is_empty());
                for edge in &page.edges {
                    assert!(refs.insert(edge.item.item_ref.clone()));
                }
                if backwards {
                    observed.extend(
                        page.edges
                            .iter()
                            .rev()
                            .map(|edge| edge.item.sibling_sequence),
                    );
                    boundary = page.page_info.start_cursor;
                    if !page.page_info.has_previous_page {
                        break;
                    }
                } else {
                    observed.extend(page.edges.iter().map(|edge| edge.item.sibling_sequence));
                    boundary = page.page_info.end_cursor;
                    if !page.page_info.has_next_page {
                        break;
                    }
                }
            }
            let mut expected = (1..=count as u64).collect::<Vec<_>>();
            if backwards {
                expected.reverse();
            }
            assert_eq!(observed, expected);
        }
        assert_eq!(
            service
                .message(
                    CONVERSATION,
                    &first.edges[0].item.item_ref,
                    Instant::now() + Duration::from_secs(2)
                )
                .unwrap()
                .unwrap()
                .text,
            "Repeated content"
        );
        assert!(service
            .canonical_timeline(
                CONVERSATION,
                Some("cwp://example.test/foreign"),
                &request,
                Instant::now() + Duration::from_secs(2)
            )
            .is_err());
    }
    eprintln!("exported module page measurements: {measurements:?}");
    assert!(
        measurements[1].1 <= measurements[0].1 + 500,
        "public module page work grew: {measurements:?}"
    );
    assert_eq!(measurements[0].2, measurements[1].2);
}
