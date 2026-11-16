fn assert_create_command_round_trip() {
    let create = HostCommand::CreateConversation {
        version: HOST_COMMAND_VERSION,
        request_id: "desktop-create-1".into(),
        source_ref: SOURCE_REF.into(),
        author_actor_ref: crate::v2026_09::test_fixture::actor_ref("person").into(),
        text: "Start here".into(),
    };
    create.validate().expect("validate create command");
    let wire = serde_json::to_value(&create).expect("serialize create command");
    assert_eq!(wire["kind"], "create_conversation");
    assert_eq!(
        serde_json::from_value::<HostCommand>(wire).expect("decode create command"),
        create
    );

    for source_ref in [Some(SOURCE_REF.into()), None] {
    let reload = HostCommand::ReloadGraph {
        version: HOST_COMMAND_VERSION,
        request_id: "desktop-reload-1".into(),
        source_ref,
    };
    reload.validate().expect("validate reload command");
    let wire = serde_json::to_value(&reload).expect("serialize reload command");
    assert_eq!(wire["kind"], "reload_graph");
    assert_eq!(
        serde_json::from_value::<HostCommand>(wire).expect("decode reload command"),
        reload
    );
    }
}
