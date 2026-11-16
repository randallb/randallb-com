use super::*;
use serde_json::json;

fn database() -> Connection {
    let mut db = Connection::open_in_memory().unwrap();
    db.execute_batch("PRAGMA foreign_keys=ON; PRAGMA recursive_triggers=ON;")
        .unwrap();
    let tx = db.transaction().unwrap();
    initialize(&tx).unwrap();
    tx.commit().unwrap();
    db
}

#[test]
fn record_existence_requires_a_node_or_relationship() {
    let mut db = database();
    assert!(!record_exists(&db, "cwp://example/missing").unwrap());

    let tx = db.transaction().unwrap();
    bind(&tx, "cwp://example/binding-only").unwrap();
    put_node(&tx, "cwp://example/node", "message", &json!({})).unwrap();
    put_edge(
        &tx,
        "cwp://example/source",
        "member",
        "cwp://example/target",
        0,
        &json!({}),
    )
    .unwrap();
    tx.commit().unwrap();

    assert!(!record_exists(&db, "cwp://example/binding-only").unwrap());
    assert!(record_exists(&db, "cwp://example/node").unwrap());
    assert!(record_exists(
        &db,
        &derived_ref("member", &["cwp://example/source", "cwp://example/target"])
    )
    .unwrap());
}

#[test]
fn identity_claims_are_idempotent_and_reject_reinterpretation() {
    let mut db = database();
    let tx = db.transaction().unwrap();
    let reference = "cwp://example/publications/one";
    let owner = "cwp://example/conversations/one";

    let id = claim_identity(&tx, reference, "conversation_timeline_publication", owner).unwrap();
    assert_eq!(
        claim_identity(&tx, reference, "conversation_timeline_publication", owner).unwrap(),
        id
    );
    assert_eq!(
        identity_claim(&tx, reference).unwrap(),
        Some(IdentityClaim {
            kind: "conversation_timeline_publication".into(),
            owner_ref: Some(owner.into()),
        })
    );
    assert!(claim_identity(&tx, reference, "conversation_timeline_connection", owner).is_err());
    assert!(claim_identity(
        &tx,
        reference,
        "conversation_timeline_publication",
        "cwp://example/conversations/two"
    )
    .is_err());
}

#[test]
fn graph_records_cannot_reuse_an_identity_across_roles() {
    let mut db = database();
    let tx = db.transaction().unwrap();
    let source = "cwp://example/source";
    let target = "cwp://example/target";
    let edge_ref = derived_ref("member", &[source, target]);

    put_node(&tx, &edge_ref, "message", &json!({})).unwrap();
    assert!(put_edge(&tx, source, "member", target, 0, &json!({})).is_err());

    let publication = "cwp://example/publications/claimed";
    claim_identity(
        &tx,
        publication,
        "conversation_timeline_publication",
        "cwp://example/conversations/one",
    )
    .unwrap();
    assert!(put_node(&tx, publication, "message", &json!({})).is_err());

    let raw = "cwp://example/raw-message";
    let raw_id = bind(&tx, raw).unwrap();
    let schema_id = bind(&tx, &schema_ref("message")).unwrap();
    tx.execute(
        "INSERT INTO nodes(ref_id,kind,schema_id,props) VALUES(?1,'message',?2,jsonb('{}'))",
        [raw_id, schema_id],
    )
    .unwrap();
    assert!(claim_identity(
        &tx,
        raw,
        "conversation_timeline_publication",
        "cwp://example/conversations/one"
    )
    .is_err());
}

#[test]
fn bindings_survive_removal_and_cannot_be_reassigned() {
    let mut db = database();
    let tx = db.transaction().unwrap();
    let id = put_node(&tx, "cwp://example/node", "message", &json!({})).unwrap();
    remove(&tx, "cwp://example/node").unwrap();
    assert_eq!(bind(&tx, "cwp://example/node").unwrap(), id);
    assert!(tx
        .execute(
            "UPDATE ref_bindings SET canonical_ref='cwp://example/other' WHERE ref_id=?1",
            [id]
        )
        .is_err());
    assert!(tx
        .execute("DELETE FROM ref_bindings WHERE ref_id=?1", [id])
        .is_err());
    assert!(tx
        .execute(
            "UPDATE ref_bindings SET ref_id=?1 WHERE ref_id=?2",
            [id + 1, id]
        )
        .is_err());
    assert!(bind(&tx, "~@/unresolved").is_err());
    tx.commit().unwrap();
}

#[test]
fn disposable_graph_records_reclaim_only_disposable_identities() {
    let mut db = database();
    let tx = db.transaction().unwrap();
    let invocation = "cwp://example/invocations/one";
    let observation = "cwp://example/metric-observations/one";
    let unrelated = "cwp://example/metric-observations/unrelated";
    let durable = "cwp://example/application-state/one";
    put_node(&tx, durable, "application_state", &json!({})).unwrap();
    put_disposable_node(
        &tx,
        observation,
        "metric_observation",
        &json!({"record":{"recorded_at_unix_ms":1}}),
    )
    .unwrap();
    put_disposable_node(
        &tx,
        unrelated,
        "metric_observation",
        &json!({"record":{"recorded_at_unix_ms":2}}),
    )
    .unwrap();
    put_disposable_edge(
        &tx,
        invocation,
        "metric_observation",
        observation,
        1,
        &json!({}),
    )
    .unwrap();
    let edge_reference = derived_ref("metric_observation", &[invocation, observation]);
    assert_eq!(
        tx.query_row(
            "SELECT COUNT(*) FROM ref_bindings WHERE disposable=1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        4,
    );
    assert!(remove_disposable(&tx, durable).is_err());
    remove_disposable(&tx, observation).unwrap();
    assert_eq!(find(&tx, observation).unwrap(), None);
    assert_eq!(find(&tx, invocation).unwrap(), None);
    assert_eq!(find(&tx, &edge_reference).unwrap(), None);
    assert!(find(&tx, durable).unwrap().is_some());
    assert!(node_props(&tx, unrelated).unwrap().is_some());
    remove_disposable(&tx, unrelated).unwrap();
    assert_eq!(
        tx.query_row(
            "SELECT COUNT(*) FROM ref_bindings WHERE disposable=1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        0,
    );
    tx.commit().unwrap();
}

#[test]
fn batch_disposable_removal_reclaims_only_the_expired_subtrees() {
    let mut db = database();
    let tx = db.transaction().unwrap();
    let observations = (0..64)
        .map(|index| format!("cwp://example/metric-observations/{index}"))
        .collect::<Vec<_>>();
    for (index, observation) in observations.iter().enumerate() {
        let invocation = format!("cwp://example/invocations/{index}");
        put_disposable_node(
            &tx,
            observation,
            "metric_observation",
            &json!({"record":{"recorded_at_unix_ms":index}}),
        )
        .unwrap();
        put_disposable_edge(
            &tx,
            &invocation,
            "metric_observation",
            observation,
            index as i64,
            &json!({}),
        )
        .unwrap();
    }
    let retained = "cwp://example/metric-observations/retained";
    put_disposable_node(
        &tx,
        retained,
        "metric_observation",
        &json!({"record":{"recorded_at_unix_ms":999}}),
    )
    .unwrap();

    remove_disposables(&tx, observations.iter().map(String::as_str)).unwrap();

    for observation in &observations {
        assert_eq!(find(&tx, observation).unwrap(), None);
    }
    assert!(node_props(&tx, retained).unwrap().is_some());
    assert_eq!(
        tx.query_row(
            "SELECT COUNT(*) FROM ref_bindings WHERE disposable=1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        1,
    );
    let indexes = tx
        .prepare("SELECT name FROM sqlite_master WHERE type='index' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    for index in [
        "ref_bindings_disposable",
        "nodes_by_schema",
        "edges_by_schema",
    ] {
        assert!(indexes.iter().any(|name| name == index));
    }
    tx.commit().unwrap();
}

#[test]
fn opening_an_existing_graph_adds_disposable_binding_support() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("existing.sqlite3");
    let old = Connection::open(&path).unwrap();
    old.execute_batch(
        r#"
            CREATE TABLE ref_bindings (
                ref_id INTEGER PRIMARY KEY CHECK (ref_id > 0),
                canonical_ref TEXT NOT NULL UNIQUE CHECK (canonical_ref GLOB 'cwp://?*')
            );
            CREATE TRIGGER ref_bindings_no_update BEFORE UPDATE ON ref_bindings
            BEGIN SELECT RAISE(ABORT, 'reference bindings are immutable'); END;
            CREATE TRIGGER ref_bindings_no_delete BEFORE DELETE ON ref_bindings
            BEGIN SELECT RAISE(ABORT, 'reference bindings are immutable'); END;
            CREATE TABLE nodes (
                ref_id INTEGER PRIMARY KEY REFERENCES ref_bindings(ref_id),
                kind TEXT NOT NULL,
                schema_id INTEGER NOT NULL REFERENCES ref_bindings(ref_id),
                revision INTEGER NOT NULL DEFAULT 0,
                props BLOB NOT NULL
            );
            CREATE TABLE edges (
                ref_id INTEGER PRIMARY KEY REFERENCES ref_bindings(ref_id),
                kind TEXT NOT NULL,
                schema_id INTEGER NOT NULL REFERENCES ref_bindings(ref_id),
                source_id INTEGER NOT NULL REFERENCES ref_bindings(ref_id),
                target_id INTEGER NOT NULL REFERENCES ref_bindings(ref_id),
                position INTEGER NOT NULL DEFAULT 0,
                revision INTEGER NOT NULL DEFAULT 0,
                props BLOB NOT NULL
            );
            INSERT INTO ref_bindings(ref_id,canonical_ref) VALUES
                (1,'cwp://example/existing-message'),
                (2,'cwp://example/schemas/message');
            INSERT INTO nodes(ref_id,kind,schema_id,props)
                VALUES(1,'message',2,'{}');
        "#,
    )
    .unwrap();
    drop(old);

    let mut store = NativeStore::open(&path).unwrap();
    store
        .graph_transaction(|tx| {
            assert_eq!(
                identity_claim(tx, "cwp://example/existing-message")?,
                Some(IdentityClaim {
                    kind: "node:message".into(),
                    owner_ref: None,
                })
            );
            let reference = "cwp://example/ephemeral";
            bind_disposable(tx, reference)?;
            let disposable: i64 = tx.query_row(
                "SELECT disposable FROM ref_bindings WHERE canonical_ref=?1",
                [reference],
                |row| row.get(0),
            )?;
            assert_eq!(disposable, 1);
            assert!(tx
                .execute(
                    "INSERT INTO ref_bindings(canonical_ref,claim_owner_ref) VALUES(?1,?2)",
                    ["cwp://example/invalid-claim", "cwp://example/owner"],
                )
                .is_err());
            let unclaimed = "cwp://example/unclaimed";
            bind(tx, unclaimed)?;
            assert!(tx
                .execute(
                    "UPDATE ref_bindings SET claim_owner_ref=?2 WHERE canonical_ref=?1",
                    [unclaimed, "cwp://example/owner"],
                )
                .is_err());
            Ok(())
        })
        .unwrap();
}

#[test]
fn rebuilding_and_reordering_edges_reuses_bindings_and_extensions() {
    let mut db = database();
    let tx = db.transaction().unwrap();
    let source = "cwp://example/root";
    let target = "cwp://example/child";
    let id = put_edge(
        &tx,
        source,
        "member",
        target,
        1,
        &json!({"extension":{"future":true}}),
    )
    .unwrap();
    let count: i64 = tx
        .query_row("SELECT COUNT(*) FROM ref_bindings", [], |r| r.get(0))
        .unwrap();
    for position in 2..12 {
        assert_eq!(
            put_edge(&tx, source, "member", target, position, &json!({})).unwrap(),
            id
        );
        let props: String = tx
            .query_row("SELECT json(props) FROM edges WHERE ref_id=?1", [id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&props).unwrap()["extension"]["future"],
            true
        );
    }
    remove_edge(&tx, source, "member", target).unwrap();
    assert_eq!(
        put_edge(&tx, source, "member", target, 0, &json!({})).unwrap(),
        id
    );
    assert_eq!(
        tx.query_row("SELECT COUNT(*) FROM ref_bindings", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        count
    );
    assert_eq!(targets(&tx, source, "member").unwrap(), [target]);
    tx.commit().unwrap();
}

#[test]
fn rollback_removes_partial_records_bindings_and_index_edges() {
    let mut db = database();
    {
        let tx = db.transaction().unwrap();
        put_node(
            &tx,
            "cwp://example/record",
            "message",
            &json!({"record":{"text":"partial"}}),
        )
        .unwrap();
        put_edge(
            &tx,
            "cwp://example/root",
            "message",
            "cwp://example/record",
            0,
            &json!({}),
        )
        .unwrap();
        assert!(put_edge(
            &tx,
            "cwp://example/root",
            "message",
            "invalid",
            1,
            &json!({})
        )
        .is_err());
    }
    assert_eq!(find(&db, "cwp://example/record").unwrap(), None);
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM edges", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    let tx = db.transaction().unwrap();
    put_node(&tx, "cwp://example/retry", "message", &json!({})).unwrap();
    tx.commit().unwrap();
    assert!(node_props(&db, "cwp://example/retry").unwrap().is_some());
}

#[test]
fn record_replacement_preserves_extensions_and_rejects_schema_reinterpretation() {
    let mut db = database();
    let tx = db.transaction().unwrap();
    let reference = "cwp://example/message";
    put_node(
        &tx,
        reference,
        "message",
        &json!({"record":{"text":"old","reply":"old"},"future":{"flag":true}}),
    )
    .unwrap();
    put_node(&tx, reference, "message", &json!({"record":{"text":"new"}})).unwrap();
    put_node(&tx, reference, "message", &json!({"future":{"flag":true}})).unwrap();
    let props = node_props(&tx, reference).unwrap().unwrap();
    assert_eq!(
        props,
        json!({"record":{"text":"new"},"future":{"flag":true}})
    );
    assert!(put_node(&tx, reference, "invocation", &json!({"record":{}})).is_err());
    let schema = bind(&tx, "cwp://example/future-schema").unwrap();
    tx.execute("UPDATE nodes SET schema_id=?1 WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2)",params![schema,reference]).unwrap();
    assert!(put_node(&tx, reference, "message", &json!({"record":{}})).is_err());
}

#[test]
fn deleting_a_record_removes_edges_that_index_its_relationships() {
    let mut db = database();
    let tx = db.transaction().unwrap();
    let reference = "cwp://example/message";
    put_node(&tx, reference, "message", &json!({})).unwrap();
    put_edge(
        &tx,
        "cwp://example/root",
        "timeline",
        reference,
        0,
        &json!({}),
    )
    .unwrap();
    let edge = derived_ref("timeline", &["cwp://example/root", reference]);
    put_edge(&tx, "cwp://example/author", "lookup", &edge, 0, &json!({})).unwrap();
    remove(&tx, reference).unwrap();
    assert!(targets(&tx, "cwp://example/author", "lookup")
        .unwrap()
        .is_empty());
    assert!(targets(&tx, "cwp://example/root", "timeline")
        .unwrap()
        .is_empty());
    assert!(find(&tx, &edge).unwrap().is_some());
    assert_eq!(
        tx.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| r
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        0
    );
}

#[test]
fn edge_record_patch_retains_omitted_record_and_rejects_future_schema() {
    let mut db = database();
    let tx = db.transaction().unwrap();
    let source = "cwp://example/source";
    let target = "cwp://example/target";
    let id = put_edge(
        &tx,
        source,
        "member",
        target,
        0,
        &json!({"record":{"text":"old","reply":"old"}}),
    )
    .unwrap();
    put_edge(&tx, source, "member", target, 1, &json!({"extension":true})).unwrap();
    let read = || -> Value {
        let text: String = tx
            .query_row("SELECT json(props) FROM edges WHERE ref_id=?1", [id], |r| {
                r.get(0)
            })
            .unwrap();
        serde_json::from_str(&text).unwrap()
    };
    assert_eq!(
        read(),
        json!({"record":{"text":"old","reply":"old"},"extension":true})
    );
    put_edge(
        &tx,
        source,
        "member",
        target,
        1,
        &json!({"record":{"text":"new"}}),
    )
    .unwrap();
    assert_eq!(read(), json!({"record":{"text":"new"},"extension":true}));
    let schema = bind(&tx, "cwp://example/future-schema").unwrap();
    tx.execute(
        "UPDATE edges SET schema_id=?1 WHERE ref_id=?2",
        params![schema, id],
    )
    .unwrap();
    assert!(put_edge(&tx, source, "member", target, 1, &json!({})).is_err());
    assert_eq!(read(), json!({"record":{"text":"new"},"extension":true}));
}

#[test]
fn disposable_retention_refuses_durable_relationship_subtrees_before_mutation() {
    let mut db = database();
    let tx = db.transaction().unwrap();
    let first = "cwp://example/observations/first";
    let second = "cwp://example/observations/second";
    put_disposable_node(&tx, first, "observation", &json!({})).unwrap();
    put_disposable_node(&tx, second, "observation", &json!({})).unwrap();
    let transient_edge = "cwp://example/transient-edge";
    // A durable relationship may reference a disposable relationship, not
    // just the node itself. Cleanup must check recursively.
    let edge_id =
        put_disposable_edge(&tx, second, "sample", transient_edge, 0, &json!({})).unwrap();
    let edge_ref: String = tx
        .query_row(
            "SELECT canonical_ref FROM ref_bindings WHERE ref_id=?1",
            [edge_id],
            |row| row.get(0),
        )
        .unwrap();
    put_edge(
        &tx,
        "cwp://example/durable",
        "index",
        &edge_ref,
        0,
        &json!({}),
    )
    .unwrap();
    assert!(remove_disposables(&tx, [first, second]).is_err());
    assert!(node_props(&tx, first).unwrap().is_some());
    assert!(node_props(&tx, second).unwrap().is_some());
    assert_eq!(
        tx.query_row("SELECT COUNT(*) FROM edges", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        2
    );
    tx.commit().unwrap();
}
