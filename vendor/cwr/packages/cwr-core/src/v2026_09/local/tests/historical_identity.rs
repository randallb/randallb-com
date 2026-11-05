// These cases distinguish labels from the source-file identity defined by
// memos/policy/cwr/cwr-references.md. They do not introduce migration aliases.
#[derive(Clone, Copy)]
enum HistoricalIdentityChange {
    RenameHandle,
    ReassignHandle,
    MoveSource,
    ReplaceContents,
}

fn historical_identity_after_reopen(change: HistoricalIdentityChange) {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("workspace");
    copy_workspace(fixture_workspace().parent().unwrap(), &root);
    let manifest = root.join("CWR.toml");
    let database = temporary.path().join("history.sqlite");
    let person = load_default_local_acting_actor_ref(&manifest).unwrap();
    let identity = LocalHostIdentity::new(&person, "cwp://example.test/identity-ingress");
    let runtime =
        LocalRuntime::open_with_state_path(&manifest, &database, identity.clone()).unwrap();
    let original_actor = participation_ref(&runtime, "helper");
    let publish = |runtime: &LocalRuntime, request: &str| {
        runtime
            .submit_text_as(
                "cwp://example.test/identity-ingress",
                request,
                &person,
                None,
                "@helper retained question",
            )
            .unwrap()
    };
    let admitted = publish(&runtime, "original");
    runtime
        .lock()
        .unwrap()
        .admission
        .complete(
            admitted.invocation_ref().unwrap(),
            json!({"text": "retained answer"}),
        )
        .unwrap();
    let before = runtime
        .conversation(admitted.conversation_ref())
        .unwrap()
        .unwrap();
    assert_eq!(before.messages.len(), 2);
    assert_eq!(before.messages[0].author_actor_ref, person);
    assert_eq!(
        before.messages[0].addressed_actor_refs,
        vec![original_actor.clone()]
    );
    assert_eq!(before.messages[1].author_actor_ref, original_actor);
    assert_eq!(
        before.messages[1].reply_to_message_ref.as_deref(),
        Some(before.messages[0].message_ref.as_str())
    );
    drop(runtime);

    let helper = root.join("coworkers/helper/coworker.toml");
    match change {
        HistoricalIdentityChange::RenameHandle | HistoricalIdentityChange::ReassignHandle => {
            let contents = fs::read_to_string(&helper).unwrap();
            fs::write(
                &helper,
                contents.replace("slug = \"helper\"", "slug = \"original\""),
            )
            .unwrap();
            if matches!(change, HistoricalIdentityChange::ReassignHandle) {
                copy_workspace(
                    &root.join("coworkers/helper"),
                    &root.join("coworkers/replacement"),
                );
                let replacement = root.join("coworkers/replacement/coworker.toml");
                fs::write(
                    &replacement,
                    fs::read_to_string(&replacement)
                        .unwrap()
                        .replace("slug = \"original\"", "slug = \"helper\""),
                )
                .unwrap();
                let mut contents = fs::read_to_string(&manifest).unwrap();
                contents.push_str("\n[[imports]]\npath = './coworkers/replacement/coworker.toml'\nas = 'replacement'\n");
                fs::write(&manifest, contents).unwrap();
            }
        }
        HistoricalIdentityChange::MoveSource => {
            fs::rename(&helper, root.join("coworkers/helper/moved.toml")).unwrap();
            for path in [&manifest, &root.join("coworkers/person/COWORKER.toml")] {
                fs::write(
                    path,
                    fs::read_to_string(path)
                        .unwrap()
                        .replace("helper/coworker.toml", "helper/moved.toml"),
                )
                .unwrap();
            }
        }
        HistoricalIdentityChange::ReplaceContents => {
            // Current policy identifies a source file, not a content generation.
            fs::write(
                &helper,
                fs::read_to_string(&helper)
                    .unwrap()
                    .replace("name = \"Helper\"", "name = \"Replacement description\""),
            )
            .unwrap();
        }
    }
    let runtime = LocalRuntime::open_with_state_path(&manifest, &database, identity).unwrap();
    let current_actor = participation_ref(
        &runtime,
        if matches!(change, HistoricalIdentityChange::RenameHandle) {
            "original"
        } else {
            "helper"
        },
    );
    if matches!(
        change,
        HistoricalIdentityChange::ReassignHandle | HistoricalIdentityChange::MoveSource
    ) {
        assert_ne!(current_actor, original_actor);
    } else {
        assert_eq!(current_actor, original_actor);
    }
    let after = runtime
        .conversation(admitted.conversation_ref())
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(&after).unwrap(),
        serde_json::to_value(&before).unwrap()
    );
    let retry = publish(&runtime, "original");
    assert_eq!(retry.conversation_ref(), admitted.conversation_ref());
    assert_eq!(retry.invocation_ref(), admitted.invocation_ref());
    assert_eq!(
        retry.record.source_message_ref,
        admitted.record.source_message_ref
    );
    assert_eq!(
        retry.record.target_actor_ref.as_deref(),
        Some(original_actor.as_str())
    );
    assert_eq!(
        serde_json::to_value(&retry.conversation).unwrap(),
        serde_json::to_value(&before).unwrap()
    );
    assert_eq!(runtime.conversations().unwrap().len(), 2); // Root plus original execution conversation.
    if !matches!(change, HistoricalIdentityChange::RenameHandle) {
        let fresh = publish(&runtime, "fresh");
        assert_eq!(
            fresh.record.target_actor_ref.as_deref(),
            Some(current_actor.as_str())
        );
        assert_ne!(
            fresh.record.source_message_ref,
            admitted.record.source_message_ref
        );
    }
}

#[test]
fn historical_identity_handle_rename_preserves_history_and_retry() {
    historical_identity_after_reopen(HistoricalIdentityChange::RenameHandle);
}

#[test]
fn historical_identity_handle_reassignment_preserves_original_destination() {
    historical_identity_after_reopen(HistoricalIdentityChange::ReassignHandle);
}

#[test]
fn historical_identity_source_move_creates_new_identity_without_retargeting_retry() {
    historical_identity_after_reopen(HistoricalIdentityChange::MoveSource);
}

#[test]
fn historical_identity_same_source_content_change_keeps_identity() {
    historical_identity_after_reopen(HistoricalIdentityChange::ReplaceContents);
}
