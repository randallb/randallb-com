use super::*;
#[test]
fn summary_window_uses_the_root_keyset_index() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runtime.db");
    let runtime = "cwp://fixture/runtime";
    let scope = Scope {
        runtime_root_ref: runtime.into(),
        context: Context {
            workspace_ref: "cwp://fixture/workspaces/family".into(),
            caller_ref: "cwp://fixture/actors/coordinator".into(),
        },
    };
    let options = OpenOptions {
        runtime_state_version: "cwp://org.coworkerprotocol/cwpdb/schemas/runtime-state/2026-09"
            .into(),
        workspace_binding: None,
        timeline_rollout: None,
    };
    let store = NativeConversationStore::open(&path, scope, options).unwrap();
    drop(store);
    let snapshot = NativeReadSnapshot::open_at_revision(
        &path,
        runtime,
        "0",
        ReadControl {
            cancellation: ReadCancellation::default(),
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(5),
            vm_steps: None,
        },
    )
    .unwrap();
    let plan = snapshot.connection.prepare(&format!("EXPLAIN QUERY PLAN {SUMMARY_SELECT} AND (creation_order, conversation_ref) < (?1, ?2) {ORDER_DESC} LIMIT ?3"))
        .unwrap().query_map(params![9950_i64, "boundary", 51_i64], |row| row.get::<_, String>(3)).unwrap()
        .collect::<Result<Vec<_>, _>>().unwrap().join("\n");
    assert!(plan.contains("edges_out_ordered"), "{plan}");
    assert!(!plan.contains("USE TEMP B-TREE"), "{plan}");
}

const WORK_FROM: &str = "FROM edges lookup JOIN edges a ON a.ref_id=lookup.target_id
WHERE lookup.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2)
AND a.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND a.kind='timeline_activity'";

#[test]
fn timeline_and_work_relationship_indexes_remain_ordered() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runtime.db");
    let runtime = "cwp://fixture/runtime";
    let scope = Scope {
        runtime_root_ref: runtime.into(),
        context: Context {
            workspace_ref: "cwp://fixture/workspaces/family".into(),
            caller_ref: "cwp://fixture/actors/coordinator".into(),
        },
    };
    let options = OpenOptions {
        runtime_state_version: "cwp://org.coworkerprotocol/cwpdb/schemas/runtime-state/2026-09"
            .into(),
        workspace_binding: None,
        timeline_rollout: None,
    };
    let store = NativeConversationStore::open(&path, scope, options).unwrap();
    drop(store);
    let snapshot = NativeReadSnapshot::open_at_revision(
        &path,
        runtime,
        "0",
        ReadControl {
            cancellation: ReadCancellation::default(),
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(5),
            vm_steps: None,
        },
    )
    .unwrap();

    let conversation = "cwp://fixture/conversations/one";
    let plan = snapshot.connection.prepare("EXPLAIN QUERY PLAN SELECT position FROM timeline_items WHERE conversation_ref=?1 AND position>?2 ORDER BY position LIMIT 10").unwrap().query_map(params![conversation,0], |row|row.get::<_,String>(3)).unwrap().collect::<Result<Vec<_>,_>>().unwrap().join("\n");
    assert!(plan.contains("edges_out_ordered"), "{plan}");
    assert!(!plan.contains("USE TEMP B-TREE"), "{plan}");
    for kind in ["timeline_invocation", "timeline_source", "timeline_parent"] {
        let sql = format!("EXPLAIN QUERY PLAN SELECT a.position {WORK_FROM} AND lookup.kind='{kind}' ORDER BY lookup.position");
        let plan = snapshot
            .connection
            .prepare(&sql)
            .unwrap()
            .query_map(
                params![conversation, "cwp://fixture/invocations/one"],
                |row| row.get::<_, String>(3),
            )
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .join("\n");
        assert!(plan.contains("edges_out_ordered"), "{plan}");
    }
}
