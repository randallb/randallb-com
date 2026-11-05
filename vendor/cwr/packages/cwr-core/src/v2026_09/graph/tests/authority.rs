use super::route_authority;

#[test]
fn route_authority_is_the_actor_adapter_intersection_and_reports_requirements() {
    let (available, missing) = route_authority(
        &[
            "workspace.read".into(),
            "workspace.write".into(),
            "process.execute".into(),
        ],
        &[
            "workspace.read".into(),
            "workspace.write".into(),
            "process.execute".into(),
        ],
        &["workspace.read".into(), "workspace.write".into()],
    );

    assert_eq!(available, ["workspace.read", "workspace.write"]);
    assert_eq!(missing, ["process.execute"]);
}

#[test]
fn root_actor_refs_are_rejected() {
    let temporary = tempfile::tempdir().unwrap();
    let manifest = temporary.path().join("CWR.toml");
    std::fs::write(
        &manifest,
        format!("type = {:?}\nroot_actor_refs = []\n", super::ROOT_TYPE),
    )
    .unwrap();
    assert!(super::resolve_graph(&manifest)
        .unwrap_err()
        .to_string()
        .contains("root_actor_refs"));
}

#[test]
fn retained_data_permissions_are_explicit_and_entry_manifest_owned() {
    let temporary = tempfile::tempdir().unwrap();
    let manifest = temporary.path().join("CWR.toml");
    let write = |tail: &str| {
        std::fs::write(&manifest, format!("type = {:?}\n{tail}", super::ROOT_TYPE)).unwrap()
    };
    write("");
    assert!(!super::resolve_graph(&manifest)
        .unwrap()
        .permissions
        .allows_all());
    write("[permissions]\nread_retained_data = 'all'\n");
    let graph = super::resolve_graph(&manifest).unwrap();
    assert!(graph.permissions.allows_all());
    assert_eq!(
        serde_json::to_value(&graph).unwrap()["permissions"]["read_retained_data"],
        "all"
    );
    write("[permissions]\nread_retained_data = 'participants'\n");
    assert!(!super::resolve_graph(&manifest)
        .unwrap()
        .permissions
        .allows_all());
    for invalid in ["'al'", "true", "42"] {
        write(&format!("[permissions]\nread_retained_data = {invalid}\n"));
        assert!(super::resolve_graph(&manifest).is_err());
    }
    write("[permissions]\nread_retained_date = 'all'\n");
    assert!(super::resolve_graph(&manifest).is_err());
    std::fs::write(
        temporary.path().join("imported.toml"),
        format!(
            "type = {:?}\n[permissions]\nread_retained_data = 'all'\n",
            super::RUNTIME_TYPE
        ),
    )
    .unwrap();
    write("[[imports]]\npath = './imported.toml'\n");
    assert!(!super::resolve_graph(&manifest)
        .unwrap()
        .permissions
        .allows_all());
}
