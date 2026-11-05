use super::*;
use crate::{bind, derived_ref, initialize, put_edge, remove_edge};
use serde_json::json;

const SOURCE: &str = "cwp://example/resources/source";
const TARGET: &str = "cwp://example/resources/target";
const KINDS: &[RelationshipKind<'_>] = &[RelationshipKind::Unordered("member")];

fn database() -> Connection {
    let mut db = Connection::open_in_memory().unwrap();
    let tx = db.transaction().unwrap();
    initialize(&tx).unwrap();
    tx.commit().unwrap();
    db
}

fn link(db: &mut Connection, source: &str, kind: &str, target: &str) {
    let tx = db.transaction().unwrap();
    put_edge(&tx, source, kind, target, 0, &json!({"label":target})).unwrap();
    tx.commit().unwrap();
}

fn query<'a>(continuation: Option<&'a EdgeContinuation>) -> EdgeQuery<'a> {
    EdgeQuery {
        direction: Direction::Outgoing,
        kinds: KINDS,
        page_size: 2,
        continuation,
    }
}

#[test]
fn returns_full_edges_with_external_and_edge_endpoints_in_both_directions() {
    let mut db = database();
    link(&mut db, SOURCE, "member", TARGET);
    let edge_ref = derived_ref("member", &[SOURCE, TARGET]);
    link(&mut db, SOURCE, "member", &edge_ref);
    link(&mut db, SOURCE, "other", TARGET);
    let reader = EdgeReader::new(&db);
    let page = reader.query_edges(SOURCE, query(None)).unwrap();
    assert!(page.complete);
    assert!(page.continuation.is_none());
    assert_eq!(page.items.len(), 2);
    let edge = page.items.iter().find(|e| e.target_ref == TARGET).unwrap();
    assert_eq!(edge.reference, edge_ref);
    assert_eq!(edge.source_ref, SOURCE);
    assert_eq!(edge.kind, "member");
    assert_eq!(edge.schema_ref, crate::EDGE_SCHEMA_REF);
    assert_eq!(edge.properties, json!({"label":TARGET}));
    let incoming = reader
        .query_edges(
            TARGET,
            EdgeQuery {
                direction: Direction::Incoming,
                ..query(None)
            },
        )
        .unwrap();
    assert_eq!(incoming.items, vec![edge.clone()]);
}

#[test]
fn empty_queries_do_not_create_bindings() {
    let mut db = database();
    let tx = db.transaction().unwrap();
    bind(&tx, SOURCE).unwrap();
    tx.commit().unwrap();
    let reader = EdgeReader::new(&db);
    for reference in [SOURCE, TARGET] {
        let page = reader.query_edges(reference, query(None)).unwrap();
        assert!(page.items.is_empty());
        assert!(page.complete);
        assert!(page.continuation.is_none());
    }
    assert_eq!(
        db.query_row("SELECT count(*) FROM ref_bindings", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn pages_in_canonical_order_without_duplicates_and_deduplicates_kind_filters() {
    let mut db = database();
    for i in 0..7 {
        link(&mut db, SOURCE, "member", &format!("{TARGET}/{i}"));
    }
    let reader = EdgeReader::new(&db);
    let kinds = [
        RelationshipKind::Unordered("member"),
        RelationshipKind::Unordered("member"),
    ];
    let mut continuation = None;
    let mut refs = Vec::new();
    loop {
        let page = reader
            .query_edges(
                SOURCE,
                EdgeQuery {
                    kinds: &kinds,
                    ..query(continuation.as_ref())
                },
            )
            .unwrap();
        assert_eq!(page.complete, page.continuation.is_none());
        refs.extend(page.items.into_iter().map(|e| e.reference));
        continuation = page.continuation;
        if page.complete {
            break;
        }
    }
    assert_eq!(refs.len(), 7);
    assert!(refs.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn rejects_changed_queries_and_other_reader_or_store_continuations() {
    let mut db = database();
    for i in 0..3 {
        link(&mut db, SOURCE, "member", &format!("{TARGET}/{i}"));
    }
    let reader = EdgeReader::new(&db);
    let first = reader.query_edges(SOURCE, query(None)).unwrap();
    let token = first.continuation.as_ref().unwrap();
    let changed = [
        EdgeQuery {
            direction: Direction::Incoming,
            ..query(Some(token))
        },
        EdgeQuery {
            page_size: 1,
            ..query(Some(token))
        },
        EdgeQuery {
            kinds: &[RelationshipKind::Unordered("other")],
            ..query(Some(token))
        },
    ];
    for options in changed {
        assert!(matches!(
            reader.query_edges(SOURCE, options),
            Err(EdgeReadError::InvalidContinuation)
        ));
    }
    assert!(matches!(
        reader.query_edges(TARGET, query(Some(token))),
        Err(EdgeReadError::InvalidContinuation)
    ));
    assert!(matches!(
        EdgeReader::new(&db).query_edges(SOURCE, query(Some(token))),
        Err(EdgeReadError::InvalidContinuation)
    ));
    let other_db = database();
    assert!(matches!(
        EdgeReader::new(&other_db).query_edges(SOURCE, query(Some(token))),
        Err(EdgeReadError::InvalidContinuation)
    ));
    assert_eq!(
        reader
            .query_edges(SOURCE, query(Some(token)))
            .unwrap()
            .items
            .len(),
        1
    );
}

#[test]
fn rejects_invalid_limits_and_ordered_kinds_even_in_mixed_filters() {
    let db = database();
    let reader = EdgeReader::new(&db);
    for page_size in [0, usize::MAX] {
        assert!(matches!(
            reader.query_edges(
                SOURCE,
                EdgeQuery {
                    page_size,
                    ..query(None)
                }
            ),
            Err(EdgeReadError::InvalidInput)
        ));
    }
    for kinds in [&[][..], &[RelationshipKind::Unordered("")][..]] {
        assert!(matches!(
            reader.query_edges(
                SOURCE,
                EdgeQuery {
                    kinds,
                    ..query(None)
                }
            ),
            Err(EdgeReadError::InvalidInput)
        ));
    }
    let mixed = [
        RelationshipKind::Unordered("member"),
        RelationshipKind::Ordered("timeline"),
    ];
    assert!(matches!(
        reader.query_edges(
            SOURCE,
            EdgeQuery {
                kinds: &mixed,
                ..query(None)
            }
        ),
        Err(EdgeReadError::UnsupportedDomainContract)
    ));
}

#[test]
fn query_errors_are_storage_failures() {
    let db = Connection::open_in_memory().unwrap();
    assert!(matches!(
        EdgeReader::new(&db).query_edges(SOURCE, query(None)),
        Err(EdgeReadError::Storage(_))
    ));
}

#[test]
fn continuation_survives_removed_cursor_and_reads_new_pages_without_repeating() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("store.sqlite");
    let mut writer = Connection::open(&path).unwrap();
    let tx = writer.transaction().unwrap();
    initialize(&tx).unwrap();
    tx.commit().unwrap();
    for i in 0..5 {
        link(&mut writer, SOURCE, "member", &format!("{TARGET}/{i}"));
    }
    let db = Connection::open(&path).unwrap();
    let reader = EdgeReader::new(&db);
    let first = reader.query_edges(SOURCE, query(None)).unwrap();
    let cursor = first.items.last().unwrap();
    let tx = writer.transaction().unwrap();
    remove_edge(&tx, SOURCE, "member", &cursor.target_ref).unwrap();
    tx.commit().unwrap();
    link(&mut writer, SOURCE, "member", &format!("{TARGET}/new"));
    let second = reader
        .query_edges(SOURCE, query(first.continuation.as_ref()))
        .unwrap();
    assert!(second.items.iter().all(|e| e.reference > cursor.reference));
    let fresh = reader
        .query_edges(
            SOURCE,
            EdgeQuery {
                page_size: 10,
                ..query(None)
            },
        )
        .unwrap();
    assert!(fresh.complete);
    assert_eq!(fresh.items.len(), 5);
    assert!(fresh.items.iter().any(|e| e.target_ref.ends_with("/new")));
    let edge = fresh
        .items
        .iter()
        .find(|e| e.target_ref != cursor.target_ref)
        .unwrap();
    link(&mut writer, SOURCE, "member", &edge.target_ref);
    let identical = reader
        .query_edges(
            SOURCE,
            EdgeQuery {
                page_size: 10,
                ..query(None)
            },
        )
        .unwrap();
    assert_eq!(
        identical
            .items
            .iter()
            .find(|e| e.reference == edge.reference)
            .unwrap()
            .revision,
        edge.revision
    );
    let tx = writer.transaction().unwrap();
    put_edge(
        &tx,
        SOURCE,
        "member",
        &edge.target_ref,
        0,
        &json!({"label":"changed"}),
    )
    .unwrap();
    tx.commit().unwrap();
    let updated = reader
        .query_edges(
            SOURCE,
            EdgeQuery {
                page_size: 10,
                ..query(None)
            },
        )
        .unwrap();
    assert_ne!(
        updated
            .items
            .iter()
            .find(|e| e.reference == edge.reference)
            .unwrap()
            .revision,
        edge.revision
    );
}
