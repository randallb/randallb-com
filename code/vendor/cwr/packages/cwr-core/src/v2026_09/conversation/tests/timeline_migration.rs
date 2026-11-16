fn migration_timeline_page(
    store: &ConversationAdmissionStore,
    conversation_ref: &str,
) -> ConversationTimelinePage {
    let mut reader = store.read_store().unwrap();
    let page = reader
        .begin_operation(std::time::Duration::from_secs(2))
        .unwrap()
        .conversation_timeline(
            conversation_ref,
            ConversationRecordWindow {
                first: Some(100),
                ..Default::default()
            },
        )
        .unwrap();
    page
}

fn migration_timeline_anchors(page: &ConversationTimelinePage) -> Vec<(&'static str, String)> {
    page.anchors
        .iter()
        .map(|anchor| match anchor {
            ConversationTimelineAnchor::Message { message_ref, .. } => {
                ("message", message_ref.clone())
            }
            ConversationTimelineAnchor::Turn { invocation_ref, .. } => {
                ("turn", invocation_ref.clone())
            }
        })
        .collect()
}

fn settled_legacy_turn(
    store: &mut ConversationAdmissionStore,
    conversation_ref: Option<&str>,
    event: &str,
) -> AdmittedConversationTurn {
    let turn = working_turn(store, conversation_ref, event);
    let claimed = claim_working(store, &turn);
    settle_working(store, &claimed);
    turn
}

#[test]
fn legacy_timeline_migration_is_atomic_restartable_and_order_preserving() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("legacy-timeline-migration.sqlite");
    let root = "cwp://com.example.runtime";
    let mut store = ConversationAdmissionStore::open(&path, root).unwrap();
    let first = settled_legacy_turn(&mut store, None, "migration-first");
    let conversation_ref = first.record.conversation_ref.clone();
    let second = settled_legacy_turn(&mut store, Some(&conversation_ref), "migration-second");
    let before = migration_timeline_page(&store, &conversation_ref);
    let before_anchors = migration_timeline_anchors(&before);
    assert!(!before_anchors.is_empty());

    let outcome = store
        .migrate_legacy_conversation_timeline(&conversation_ref)
        .unwrap();
    let ConversationTimelineMigrationOutcome::Migrated {
        root_connection_ref,
        item_count,
    } = outcome
    else {
        panic!("legacy conversation should migrate: {outcome:?}");
    };
    assert_eq!(item_count, before_anchors.len());
    let after = migration_timeline_page(&store, &conversation_ref);
    assert_eq!(migration_timeline_anchors(&after), before_anchors);
    assert_eq!(after.messages, before.messages);
    assert_eq!(after.activity, before.activity);
    assert_eq!(after.total_count, before.total_count);
    assert_eq!(after.has_next_page, before.has_next_page);
    assert_eq!(after.has_previous_page, before.has_previous_page);
    assert_eq!(
        store
            .conversation(&conversation_ref)
            .unwrap()
            .timeline_connection_ref
            .as_deref(),
        Some(root_connection_ref.as_str())
    );
    for invocation_ref in [
        first.record.invocation_ref.as_deref().unwrap(),
        second.record.invocation_ref.as_deref().unwrap(),
    ] {
        let invocation = store.invocation_record(invocation_ref).unwrap();
        assert!(invocation.source_message_ref.is_some());
        assert_eq!(invocation.timeline_publications.len(), 1);
    }
    assert_eq!(
        store
            .migrate_legacy_conversation_timeline(&conversation_ref)
            .unwrap(),
        ConversationTimelineMigrationOutcome::AlreadyCanonical {
            root_connection_ref: root_connection_ref.clone(),
        }
    );
    drop(store);

    let mut reopened = ConversationAdmissionStore::open(&path, root).unwrap();
    let reopened_page = migration_timeline_page(&reopened, &conversation_ref);
    assert_eq!(migration_timeline_anchors(&reopened_page), before_anchors);
    assert_eq!(reopened_page.messages, before.messages);
    assert_eq!(reopened_page.activity, before.activity);
    assert_eq!(
        reopened
            .migrate_legacy_conversation_timeline(&conversation_ref)
            .unwrap(),
        ConversationTimelineMigrationOutcome::AlreadyCanonical {
            root_connection_ref,
        }
    );
}

#[test]
fn active_legacy_conversation_is_left_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("active-legacy-timeline.sqlite");
    let root = "cwp://com.example.runtime";
    let mut store = ConversationAdmissionStore::open(&path, root).unwrap();
    let turn = working_turn(&mut store, None, "active-migration");
    let conversation_ref = turn.record.conversation_ref;
    let invocation_ref = turn.record.invocation_ref.unwrap();

    assert_eq!(
        store
            .migrate_legacy_conversation_timeline(&conversation_ref)
            .unwrap(),
        ConversationTimelineMigrationOutcome::Skipped {
            reason: ConversationTimelineMigrationSkipReason::ActiveInvocation { invocation_ref },
        }
    );
    assert!(store.retained_timeline(&conversation_ref).is_none());
    assert!(store
        .conversation(&conversation_ref)
        .unwrap()
        .timeline_connection_ref
        .is_none());
    drop(store);

    let reopened = ConversationAdmissionStore::open(&path, root).unwrap();
    assert!(reopened.retained_timeline(&conversation_ref).is_none());
}

#[test]
fn ambiguous_legacy_causal_source_is_left_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ambiguous-legacy-timeline.sqlite");
    let root = "cwp://com.example.runtime";
    let mut store = ConversationAdmissionStore::open(&path, root).unwrap();
    let turn = settled_legacy_turn(&mut store, None, "ambiguous-migration");
    let conversation_ref = turn.record.conversation_ref;
    let invocation_ref = turn.record.invocation_ref.unwrap();
    let source_message_ref = turn.record.source_message_ref;
    let alternate_source = store
        .conversation(&conversation_ref)
        .unwrap()
        .messages
        .iter()
        .find(|message| message.message_ref != source_message_ref)
        .unwrap()
        .message_ref
        .clone();
    let execution_ref = store
        .execution_conversation_ref(&invocation_ref)
        .unwrap()
        .to_owned();
    let mut state = store.state.clone();
    let execution = state
        .conversations
        .iter_mut()
        .find(|conversation| conversation.conversation_ref == execution_ref)
        .unwrap();
    execution.activity.push(ConversationActivity {
        activity_ref: format!("{invocation_ref}/conflicting-turn-start"),
        sequence: execution.activity.len() as u64 + 1,
        occurred_at_unix_ms: 1,
        event: ConversationActivityEvent::TurnStarted {
            invocation_ref: invocation_ref.clone(),
            source_message_ref: alternate_source,
            invocation: None,
        },
    });
    store.replace_state(state).unwrap();

    assert_eq!(
        store
            .migrate_legacy_conversation_timeline(&conversation_ref)
            .unwrap(),
        ConversationTimelineMigrationOutcome::Skipped {
            reason: ConversationTimelineMigrationSkipReason::AmbiguousCausalSource {
                invocation_ref,
            },
        }
    );
    assert!(store.retained_timeline(&conversation_ref).is_none());
    drop(store);

    let reopened = ConversationAdmissionStore::open(&path, root).unwrap();
    assert!(reopened.retained_timeline(&conversation_ref).is_none());
}

#[test]
fn migration_identity_collision_rolls_back_the_whole_conversation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("colliding-legacy-timeline.sqlite");
    let root = "cwp://com.example.runtime";
    let mut store = ConversationAdmissionStore::open(&path, root).unwrap();
    let turn = settled_legacy_turn(&mut store, None, "migration-collision");
    let conversation_ref = turn.record.conversation_ref;
    let invocation_ref = turn.record.invocation_ref.unwrap();
    let root_connection_ref =
        crate::cwpdb::derived_ref("legacy_conversation_timeline_root", &[&conversation_ref]);
    cwpdb_store::test_support::timeline_persistence::seed_message_identity_collision(&path, &root_connection_ref).unwrap();

    let error = store
        .migrate_legacy_conversation_timeline(&conversation_ref)
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("schema or kind conflict"));
    assert!(store.retained_timeline(&conversation_ref).is_none());
    assert!(store
        .conversation(&conversation_ref)
        .unwrap()
        .timeline_connection_ref
        .is_none());
    let invocation = store.invocation_record(&invocation_ref).unwrap();
    assert!(invocation.source_message_ref.is_none());
    assert!(invocation.timeline_publications.is_empty());
    drop(store);

    let reopened = ConversationAdmissionStore::open(&path, root).unwrap();
    assert!(reopened.retained_timeline(&conversation_ref).is_none());
    let invocation = reopened.invocation_record(&invocation_ref).unwrap();
    assert!(invocation.source_message_ref.is_none());
    assert!(invocation.timeline_publications.is_empty());
}

#[test]
fn stale_migration_writer_cannot_overwrite_the_canonical_cutover() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("stale-timeline-migration.sqlite");
    let root = "cwp://com.example.runtime";
    let mut initial = ConversationAdmissionStore::open(&path, root).unwrap();
    let turn = settled_legacy_turn(&mut initial, None, "stale-migration");
    let conversation_ref = turn.record.conversation_ref;
    drop(initial);

    let mut first = ConversationAdmissionStore::open(&path, root).unwrap();
    let mut stale = ConversationAdmissionStore::open(&path, root).unwrap();
    assert!(matches!(
        first
            .migrate_legacy_conversation_timeline(&conversation_ref)
            .unwrap(),
        ConversationTimelineMigrationOutcome::Migrated { .. }
    ));
    let error = stale
        .migrate_legacy_conversation_timeline(&conversation_ref)
        .unwrap_err();
    assert!(error.to_string().contains("another open writer"));
    assert!(stale.retained_timeline(&conversation_ref).is_none());

    let reopened = ConversationAdmissionStore::open(&path, root).unwrap();
    assert!(reopened.retained_timeline(&conversation_ref).is_some());
}

#[test]
fn new_conversation_rollout_leaves_legacy_history_untouched() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("new-only-timeline-rollout.sqlite");
    let root = "cwp://com.example.runtime";
    let mut store = ConversationAdmissionStore::open(&path, root).unwrap();
    let legacy = settled_legacy_turn(&mut store, None, "legacy-before-rollout");
    store
        .configure_timeline_rollout(ConversationTimelineRollout::NewConversations)
        .unwrap();
    let exchange = store
        .record_host_exchange(
            None,
            crate::v2026_09::test_fixture::actor_ref("person"),
            crate::v2026_09::test_fixture::actor_ref("helper"),
            "New request".into(),
            "New response".into(),
        )
        .unwrap();

    assert!(store
        .retained_timeline(&legacy.record.conversation_ref)
        .is_none());
    let timeline = store
        .retained_timeline(&exchange.conversation.conversation_ref)
        .unwrap();
    assert_eq!(
        exchange.conversation.timeline_connection_ref.as_deref(),
        Some(timeline.root_connection_ref.as_str())
    );
    let root = timeline.connection(&timeline.root_connection_ref).unwrap();
    assert_eq!(root.items.len(), 2);
    assert_eq!(
        root.items
            .iter()
            .map(|item| item.content.reference())
            .collect::<Vec<_>>(),
        [
            exchange.request_message_ref.as_str(),
            exchange.response_message_ref.as_str(),
        ]
    );
}

#[test]
fn migration_rollout_backfills_and_finishes_active_legacy_conversations() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("managed-migration-rollout.sqlite");
    let root = "cwp://com.example.runtime";
    let mut store = ConversationAdmissionStore::open(&path, root).unwrap();
    let settled = settled_legacy_turn(&mut store, None, "settled-before-rollout");
    let active = working_turn(&mut store, None, "active-before-rollout");
    let claimed = claim_working(&mut store, &active);

    store
        .configure_timeline_rollout(ConversationTimelineRollout::MigrateLegacy)
        .unwrap();
    store.migrate_eligible_legacy_timelines().unwrap();
    assert!(store
        .retained_timeline(&settled.record.conversation_ref)
        .is_some());
    assert!(store
        .retained_timeline(&active.record.conversation_ref)
        .is_none());

    settle_working(&mut store, &claimed);
    assert!(store
        .retained_timeline(&active.record.conversation_ref)
        .is_some());
    drop(store);

    let mut reopened = ConversationAdmissionStore::open_with_timeline_rollout(
        &path,
        root,
        ConversationTimelineRollout::MigrateLegacy,
    )
    .unwrap();
    reopened.migrate_eligible_legacy_timelines().unwrap();
    assert!(reopened
        .retained_timeline(&settled.record.conversation_ref)
        .is_some());
    assert!(reopened
        .retained_timeline(&active.record.conversation_ref)
        .is_some());
}

#[test]
fn migration_rollout_stage_survives_a_skipped_active_conversation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("durable-migration-rollout.sqlite");
    let root = "cwp://com.example.runtime";
    let mut store = ConversationAdmissionStore::open(&path, root).unwrap();
    let active = working_turn(&mut store, None, "active-durable-rollout");
    store
        .configure_timeline_rollout(ConversationTimelineRollout::MigrateLegacy)
        .unwrap();
    store.migrate_eligible_legacy_timelines().unwrap();
    assert!(store
        .retained_timeline(&active.record.conversation_ref)
        .is_none());
    drop(store);

    let reopened = ConversationAdmissionStore::open_with_timeline_rollout(
        &path,
        root,
        ConversationTimelineRollout::NewConversations,
    )
    .unwrap();
    assert!(reopened
        .retained_timeline(&active.record.conversation_ref)
        .is_none());
    drop(reopened);
    ConversationAdmissionStore::open_with_timeline_rollout(
        &path,
        root,
        ConversationTimelineRollout::MigrateLegacy,
    )
    .unwrap();
}

#[test]
fn rollout_mutations_return_the_committed_canonical_conversation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("mutation-rollout-return.sqlite");
    let root = "cwp://com.example.runtime";
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let mut store = ConversationAdmissionStore::open(&path, root).unwrap();
    let turn = settled_legacy_turn(&mut store, None, "mutation-before-rollout");
    let conversation_ref = turn.record.conversation_ref;
    store
        .configure_timeline_rollout(ConversationTimelineRollout::MigrateLegacy)
        .unwrap();

    let renamed = store
        .rename_conversation(&conversation_ref, person, "Canonical after rename")
        .unwrap();
    let timeline_ref = renamed
        .timeline_connection_ref
        .clone()
        .expect("rename returns the timeline created by the same transaction");
    assert_eq!(
        store
            .conversation(&conversation_ref)
            .unwrap()
            .timeline_connection_ref
            .as_deref(),
        Some(timeline_ref.as_str())
    );

    let archived = store
        .set_conversation_archived(&conversation_ref, person, true)
        .unwrap();
    assert!(archived.archived);
    assert_eq!(
        archived.timeline_connection_ref.as_deref(),
        Some(timeline_ref.as_str())
    );
}

#[test]
fn startup_rollout_migration_is_visible_to_independent_readers() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("startup-migration.sqlite");
    let root = "cwp://com.example.runtime";
    let mut store = ConversationAdmissionStore::open(&path, root).unwrap();
    let first = settled_legacy_turn(&mut store, None, "startup-first");
    let conversation = first.record.conversation_ref.clone();
    settled_legacy_turn(&mut store, Some(&conversation), "startup-second");
    drop(store);
    let mut reopened = ConversationAdmissionStore::open_with_timeline_rollout(
        &path,
        root,
        ConversationTimelineRollout::MigrateLegacy,
    )
    .unwrap();
    reopened.migrate_eligible_legacy_timelines().unwrap();
    let retained = reopened.retained_timeline(&conversation).unwrap();
    let mut reader = reopened.read_store().unwrap();
    let operation = reader
        .begin_operation(std::time::Duration::from_secs(2))
        .unwrap();
    let page = operation
        .conversation_timeline_connection(
            &conversation,
            &retained.root_connection_ref,
            &ConversationTimelinePageRequest {
                first: Some(100),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(!page.edges.is_empty());
}

include!("timeline_migration_progress.rs");
