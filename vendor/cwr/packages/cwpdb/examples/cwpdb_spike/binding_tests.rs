use rusqlite::params;
use serde_json::json;

use super::{bindings, model::*, queries, store::*};

#[test]
fn bindings_survive_record_deletion_reopen_and_vacuum() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("graph.sqlite3");
    let mut graph = Store::open(&path).unwrap();
    let original = reference("actors", 50);
    graph.create_actor(&original).unwrap();
    let id = bindings::resolve(&graph.connection, &original).unwrap();
    for sql in [
        "UPDATE ref_bindings SET canonical_ref='cwp://changed/identity' WHERE ref_id=?1",
        "DELETE FROM ref_bindings WHERE ref_id=?1",
        "INSERT OR REPLACE INTO ref_bindings(ref_id,canonical_ref) VALUES (?1,'cwp://changed/identity')",
    ] {
        assert!(graph.connection.execute(sql, [id]).unwrap_err().to_string().contains("immutable"));
    }
    graph
        .connection
        .execute("DELETE FROM nodes WHERE ref_id=?1", [id])
        .unwrap();
    let newer = reference("actors", 51);
    graph.create_actor(&newer).unwrap();
    assert!(bindings::resolve(&graph.connection, &newer).unwrap() > id);
    graph.connection.execute_batch("VACUUM").unwrap();
    drop(graph);
    let mut graph = Store::open(&path).unwrap();
    assert_eq!(bindings::resolve(&graph.connection, &original).unwrap(), id);
    assert_eq!(bindings::expand(&graph.connection, id).unwrap(), original);
    graph.create_actor(&original).unwrap();
    assert_eq!(bindings::resolve(&graph.connection, &original).unwrap(), id);
}

#[test]
fn independent_databases_round_trip_full_refs_and_shared_schema_bindings() {
    let temp = tempfile::tempdir().unwrap();
    let first = "cwp://org.first/resources/same-name";
    let second = "cwp://org.second/resources/same-name";
    let mut ids = Vec::new();
    for (index, actors) in [[first, second], [second, first]].into_iter().enumerate() {
        let path = temp.path().join(format!("{index}.sqlite3"));
        let mut graph = Store::open(&path).unwrap();
        for actor in actors {
            graph.create_actor(actor).unwrap();
        }
        let source = "cwp://org.example/source";
        graph
            .write(|tx| {
                insert_node(tx, source, "test", &json!({"url": first}))?;
                insert_edge(tx, source, AUTHOR, first, 0, &json!({}))?;
                insert_edge(tx, source, AUTHOR, second, 0, &json!({}))?;
                Ok(())
            })
            .unwrap();
        let id = bindings::resolve(&graph.connection, first).unwrap();
        ids.push(id);
        assert_eq!(bindings::expand(&graph.connection, id).unwrap(), first);
        assert_ne!(id, bindings::resolve(&graph.connection, second).unwrap());
        let stored = node(&graph.connection, source).unwrap();
        assert_eq!(stored.schema_ref, schema("test"));
        assert_eq!(stored.props["url"], first); // Arbitrary JSON is not rewritten.
        let shared: i64 = graph
            .connection
            .query_row(
                "SELECT count(DISTINCT schema_id) FROM nodes WHERE kind='actor'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(shared, 1);
        let reverse = queries::walk(
            &graph.connection,
            first,
            AUTHOR,
            Direction::In,
            Window::first(10),
        )
        .unwrap();
        assert_eq!(reverse.items.len(), 1);
        assert_eq!(reverse.items[0].source_ref, source);
        assert_eq!(reverse.items[0].target_ref, first);
        assert_eq!(reverse.items[0].schema_ref, EDGE_SCHEMA);
        // Physical keys and all endpoint/schema references are integers.
        let valid: bool = graph.connection.query_row(
            "SELECT NOT EXISTS(SELECT 1 FROM edges WHERE typeof(ref_id)!='integer'
             OR typeof(source_id)!='integer' OR typeof(target_id)!='integer' OR typeof(schema_id)!='integer')",
            [], |r| r.get(0)
        ).unwrap();
        assert!(valid);
    }
    assert_ne!(ids[0], ids[1]);
}

#[test]
fn rolled_back_bindings_leave_no_cached_ids() {
    let temp = tempfile::tempdir().unwrap();
    let mut graph = Store::open(&temp.path().join("graph.sqlite3")).unwrap();
    let failed = "cwp://org.example/failed";
    let mut rolled_back_id = 0;
    let outcome: Result<()> = graph.write(|tx| {
        rolled_back_id = bindings::bind(tx, failed)?;
        Err("injected failure".into())
    });
    assert!(outcome.is_err());
    assert!(bindings::find(&graph.connection, failed).unwrap().is_none());
    let other = "cwp://org.example/committed";
    let committed = graph.write(|tx| bindings::bind(tx, other)).unwrap();
    assert_eq!(committed, rolled_back_id);
    let retry = graph.write(|tx| bindings::bind(tx, failed)).unwrap();
    assert_ne!(retry, committed);
    assert_eq!(bindings::expand(&graph.connection, retry).unwrap(), failed);
    assert_eq!(
        bindings::expand(&graph.connection, committed).unwrap(),
        other
    );
}

#[test]
fn concurrent_writers_share_one_binding() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("graph.sqlite3");
    let first = Store::open(&path).unwrap();
    let second = Store::open(&path).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let joins: Vec<_> = [first, second]
        .into_iter()
        .map(|mut graph| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                graph
                    .write(|tx| bindings::bind(tx, "cwp://org.example/shared"))
                    .unwrap()
            })
        })
        .collect();
    let ids: Vec<_> = joins.into_iter().map(|j| j.join().unwrap()).collect();
    assert_eq!(ids[0], ids[1]);
    let graph = Store::open(&path).unwrap();
    let count: i64 = graph
        .connection
        .query_row(
            "SELECT count(*) FROM ref_bindings WHERE canonical_ref=?1",
            ["cwp://org.example/shared"],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn unknown_reads_do_not_allocate_bindings() {
    let temp = tempfile::tempdir().unwrap();
    let graph = Store::open(&temp.path().join("graph.sqlite3")).unwrap();
    let unknown = "cwp://org.example/unknown";
    assert!(node(&graph.connection, unknown).is_err());
    assert!(
        queries::timeline(&graph.connection, unknown, Window::first(10))
            .unwrap()
            .items
            .is_empty()
    );
    assert!(queries::walk(
        &graph.connection,
        unknown,
        AUTHOR,
        Direction::In,
        Window::first(10)
    )
    .unwrap()
    .items
    .is_empty());
    let cursor = Cursor {
        scope: unknown.into(),
        kind: AUTHOR.into(),
        direction: Direction::In,
        position: 0,
        edge_ref: "cwp://org.example/unknown-edge".into(),
    };
    assert!(queries::walk(
        &graph.connection,
        unknown,
        AUTHOR,
        Direction::In,
        Window {
            cursor: Some(&cursor),
            limit: 10,
            order: Order::Forward
        }
    )
    .is_err());
    let count: i64 = graph
        .connection
        .query_row("SELECT count(*) FROM ref_bindings", params![], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2); // Store identity and its schema only.
}
