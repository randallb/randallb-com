#[test]
fn intrinsic_action_renames_the_requesting_conversation() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("runtime.sqlite"),
        fixture_host_identity(),
    )
    .unwrap();
    let original = runtime.admit_text(None, "Initial message").unwrap();
    let conversation_ref = original.conversation_ref().to_owned();
    let message_count = original.conversation.messages.len();
    let rename = runtime
        .admit_action(
            Some(&conversation_ref),
            crate::intrinsic_cwr::RENAME_CONVERSATION_ACTIVATION,
            json!({"title": "  Apple local adapter design  "}),
        )
        .unwrap();
    let mut task = runtime.try_schedule_execution().unwrap().unwrap();
    assert_eq!(task.record().invocation_ref, rename.invocation_ref().unwrap());
    task.execute_with_model_output(&AdapterModelOutputMailbox::discard(), |_| {})
        .unwrap();

    let renamed = runtime.conversation(&conversation_ref).unwrap().unwrap();
    assert_eq!(renamed.title.as_deref(), Some("Apple local adapter design"));
    assert_eq!(renamed.conversation_ref, conversation_ref);
    assert_eq!(renamed.messages.len(), message_count + 2);
}

#[test]
fn conversational_action_accepts_a_separated_trimmed_title() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("runtime.sqlite"),
        fixture_host_identity(),
    )
    .unwrap();
    let original = runtime.admit_text(None, "Initial message").unwrap();
    let conversation_ref = original.conversation_ref().to_owned();
    let title = "a".repeat(crate::v2026_09::conversation::MAX_CONVERSATION_TITLE_CHARS);
    let rename = runtime
        .admit_text(
            Some(&conversation_ref),
            format!("@cwr--rename--conversation --title '  {title}  '"),
        )
        .unwrap();
    let mut task = runtime.try_schedule_execution().unwrap().unwrap();
    assert_eq!(task.record().invocation_ref, rename.invocation_ref().unwrap());
    task.execute_with_model_output(&AdapterModelOutputMailbox::discard(), |_| {})
        .unwrap();

    let renamed = runtime.conversation(&conversation_ref).unwrap().unwrap();
    assert_eq!(renamed.title.as_deref(), Some(title.as_str()));
}
