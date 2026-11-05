use super::*;
use crate::v2026_09::conversation::ConversationAdmissionStore;
use std::time::Duration;

const RUNTIME: &str = "cwp://com.example.runtime";
const TIMEOUT: Duration = Duration::from_secs(2);

#[test]
fn participant_pages_hide_other_conversations_before_pagination() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("conversation.sqlite");
    drop(ConversationAdmissionStore::open(&path, RUNTIME).unwrap());
    cwpdb_store::test_support::summary_pages::participant_roots(&path, RUNTIME).unwrap();
    let participant = format!("{RUNTIME}/actors/person");

    let mut store = ConversationReadStore::open(&path, RUNTIME).unwrap();
    let operation = store.begin_operation(TIMEOUT).unwrap();
    let first = operation
        .participant_root_conversations(
            &participant,
            None,
            ConversationSummaryWindow {
                first: Some(2),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(orders(&first), vec![5, 3]);
    assert_eq!(first.total_count, 3);
    assert_eq!(first.archived_count, 1);
    assert!(first.has_next_page);
    let second = operation
        .participant_root_conversations(
            &participant,
            None,
            ConversationSummaryWindow {
                after: first.summaries.last().map(|summary| &summary.key),
                first: Some(2),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(orders(&second), vec![1]);
    assert!(!second.has_next_page);
    assert!(!second.has_previous_page);

    let error = operation
        .participant_root_conversations(
            &participant,
            None,
            ConversationSummaryWindow {
                after: Some(&ConversationSummaryKey {
                    creation_order: 4,
                    conversation_ref: format!("{RUNTIME}/conversations/00004"),
                }),
                first: Some(1),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("boundary is not in the root conversation relation"));

    let archived = operation
        .participant_root_conversations(
            &participant,
            Some(true),
            ConversationSummaryWindow {
                first: Some(10),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(orders(&archived), vec![3]);
    assert_eq!(archived.total_count, 1);
    assert_eq!(archived.archived_count, 1);

    let active = operation
        .participant_root_conversations(
            &participant,
            Some(false),
            ConversationSummaryWindow {
                first: Some(10),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(orders(&active), vec![5, 1]);
    assert_eq!(active.total_count, 2);
    assert_eq!(active.archived_count, 0);
}

fn orders(page: &ConversationSummaryPage) -> Vec<i64> {
    page.summaries
        .iter()
        .map(|summary| summary.key.creation_order)
        .collect()
}
