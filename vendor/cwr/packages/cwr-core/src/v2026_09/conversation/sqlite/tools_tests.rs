#[test]
fn batched_tool_files_follow_their_operation_and_edge_position() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("tool-files.sqlite");
    let page = cwpdb_store::test_support::tool_files::bootstrap(&path).unwrap();
    let loaded = super::super::backend::fixture_decode_tool_operations(&page).unwrap();
    assert_eq!(loaded.len(), 3);
    let paths = |reference: &str| {
        loaded[reference]
            .activity
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        paths("cwp://example.test/tool-operations/first"),
        ["a/before.rs", "a/after.rs"]
    );
    assert_eq!(
        paths("cwp://example.test/tool-operations/second"),
        ["b/only.rs"]
    );
    assert!(paths("cwp://example.test/tool-operations/empty").is_empty());
}
