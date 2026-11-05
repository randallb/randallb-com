use crate::NativeStore;
fn execute(store: &mut NativeStore, request: Request) -> Result<Response> {
    super::execute(
        store,
        &super::super::test_catalog().map_err(storage)?,
        request,
    )
}
use super::*;
use crate::{BatchRequest, SqlStatement};

fn id(n: u64) -> String {
    format!("cwp://local.fixture/workspaces/00000000-0000-4000-8000-{n:012x}")
}
fn request(root: &str, id: &str) -> Request {
    Request::Register {
        root: root.into(),
        id: id.into(),
        request_id: uuid::Uuid::new_v4().to_string(),
    }
}
fn entry(response: Response) -> Entry {
    match response {
        Response::Committed {
            entry: Some(entry), ..
        }
        | Response::Read { entry: Some(entry) } => entry,
        _ => panic!("expected entry"),
    }
}
fn sql(store: &mut NativeStore, sql: &str) -> crate::CwpdbResult<()> {
    store.execute_batch(&BatchRequest {
        statements: vec![SqlStatement {
            sql: sql.into(),
            params: vec![],
        }],
    })?;
    Ok(())
}

#[test]
fn replay_conflict_expiration_and_retirement_are_atomic() -> crate::CwpdbResult<()> {
    let mut store = NativeStore::open_in_memory()?;
    let owner = id(1);
    let req = request("/first", &owner);
    let expected = execute(&mut store, request_from(&req))?;
    assert_eq!(execute(&mut store, request_from(&req))?, expected);
    let mut changed = serde_json::to_value(&req)?;
    changed["root"] = json!("/different");
    assert_eq!(
        execute(&mut store, serde_json::from_value(changed)?)
            .unwrap_err()
            .code,
        Code::RequestConflict
    );
    let first = entry(expected);
    let reb = Request::Rebind {
        root: "/moved".into(),
        id: owner.clone(),
        expected_revision: first.revision,
        request_id: uuid::Uuid::new_v4().to_string(),
    };
    let second = entry(execute(&mut store, request_from(&reb))?);
    assert_ne!(second.revision, "0");
    assert_eq!(
        execute(&mut store, request_from(&req))?,
        Response::CommittedResultExpired
    );
    assert_eq!(
        execute(&mut store, request_from(&reb))?,
        Response::Committed {
            entry: Some(second.clone()),
            removed: false
        }
    );
    assert!(matches!(
        execute(
            &mut store,
            Request::ReadRoot {
                root: "/first".into()
            }
        )?,
        Response::Read { entry: None }
    ));
    // The same path identity becomes vacant and may bind a different workspace.
    execute(&mut store, request("/first", &id(2)))?;
    let remove = Request::Remove {
        root: second.root,
        id: owner.clone(),
        expected_revision: second.revision,
        request_id: uuid::Uuid::new_v4().to_string(),
    };
    assert_eq!(
        execute(&mut store, request_from(&remove))?,
        Response::Committed {
            entry: None,
            removed: true
        }
    );
    assert_eq!(
        execute(&mut store, request_from(&remove))?,
        Response::CommittedResultExpired
    );
    assert_eq!(
        execute(&mut store, request_from(&reb))?,
        Response::CommittedResultExpired
    );
    assert_eq!(
        execute(&mut store, request("/moved", &owner))
            .unwrap_err()
            .code,
        Code::RefRetired
    );
    store.graph_read(|db|{let payload:String=db.query_row("SELECT group_concat(json(props)) FROM nodes WHERE kind='desktop_registry_receipt_v1' AND json_extract(props,'$.record.expired')=1",[],|r|r.get(0))?;assert!(!payload.contains("/first"));assert!(!payload.contains("/moved"));Ok(())})?;
    Ok(())
}
fn request_from(request: &Request) -> Request {
    serde_json::from_value(serde_json::to_value(request).unwrap()).unwrap()
}

#[test]
fn rollback_after_entity_change_preserves_no_partial_state_or_request_id() -> crate::CwpdbResult<()>
{
    let mut store = NativeStore::open_in_memory()?;
    let owner = id(3);
    let req = request("/rollback", &owner);
    store.graph_transaction(|tx|{tx.execute_batch("CREATE TRIGGER inject_registry_edge BEFORE INSERT ON edges WHEN NEW.kind='desktop_workspace_binding_v1' BEGIN SELECT RAISE(ABORT,'injected'); END")?;Ok(())})?;
    assert_eq!(
        execute(&mut store, request_from(&req)).unwrap_err().code,
        Code::StorageFailure
    );
    store.graph_read(|db| {
        assert!(!unordered::retired(db, &owner)?);
        assert!(!unordered::retired(db, &binding_ref("/rollback"))?);
        assert!(!cwpdb::record_exists(db, &owner)?);
        Ok(())
    })?;
    store.graph_transaction(|tx| {
        tx.execute_batch("DROP TRIGGER inject_registry_edge")?;
        Ok(())
    })?;
    execute(&mut store, request_from(&req))?;
    Ok(())
}

#[test]
fn stale_rebind_and_unknown_relationship_reject_without_erasing_live_receipt(
) -> crate::CwpdbResult<()> {
    let mut store = NativeStore::open_in_memory()?;
    let owner = id(4);
    let req = request("/a", &owner);
    let first = entry(execute(&mut store, request_from(&req))?);
    let next = entry(execute(
        &mut store,
        Request::Rebind {
            root: "/b".into(),
            id: owner.clone(),
            expected_revision: first.revision.clone(),
            request_id: uuid::Uuid::new_v4().to_string(),
        },
    )?);
    assert_eq!(
        execute(
            &mut store,
            Request::Rebind {
                root: "/c".into(),
                id: owner.clone(),
                expected_revision: first.revision,
                request_id: uuid::Uuid::new_v4().to_string()
            }
        )
        .unwrap_err()
        .code,
        Code::RevisionConflict
    );
    store.graph_transaction(|tx| {
        cwpdb::put_edge(tx, &owner, "foreign", "cwp://fixture/other", 0, &json!({}))?;
        Ok(())
    })?;
    let remove = Request::Remove {
        root: "/b".into(),
        id: owner.clone(),
        expected_revision: next.revision.clone(),
        request_id: uuid::Uuid::new_v4().to_string(),
    };
    assert_eq!(
        execute(&mut store, request_from(&remove)).unwrap_err().code,
        Code::RelationshipsExist
    );
    assert_eq!(
        entry(execute(&mut store, Request::ReadId { id: owner.clone() })?),
        next
    );
    store.graph_transaction(|tx| {
        cwpdb::remove_edge(tx, &owner, "foreign", "cwp://fixture/other")?;
        Ok(())
    })?;
    execute(&mut store, remove)?;
    Ok(())
}

#[test]
fn retained_records_reopen_without_rewrite_and_mismatched_envelopes_fail_closed(
) -> crate::CwpdbResult<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("registry.db");
    let owner = id(5);
    let mut store = NativeStore::open(&path)?;
    let catalog = Catalog::new(&super::super::test_catalog()?)?;
    // Produce old Desktop records with no identity claims, exactly as retained
    // SQL graph writers did, and preserve their revision and envelope.
    store.graph_transaction(|tx| {
        for (reference, kind, schema, value) in [
            (
                owner.clone(),
                WORKSPACE_KIND,
                "cwp://org.coworkerprotocol/cwpdb/schemas/desktop_workspace_v1/2026-09",
                json!({"record":{"id":owner,"root":"/retained"}}),
            ),
            (
                binding_ref("/retained"),
                BINDING_KIND,
                BINDING_EDGE.schema,
                json!({"record":{"id":owner,"root":"/retained"}}),
            ),
        ] {
            let node = cwpdb::bind(tx, &reference)?;
            let schema = cwpdb::bind(tx, schema)?;
            tx.execute(
                "INSERT INTO nodes(ref_id,kind,schema_id,props) VALUES(?1,?2,?3,jsonb(?4))",
                rusqlite::params![node, kind, schema, serde_json::to_string(&value)?],
            )?;
        }
        unordered::link(tx, BINDING_EDGE, &owner, &binding_ref("/retained"))?;
        unordered::link(tx, INSTANCE_EDGE, &owner, &instance_ref("/retained"))?;
        Ok(())
    })?;
    assert_eq!(
        entry(execute(&mut store, Request::ReadId { id: owner.clone() })?).revision,
        "0"
    );
    drop(store);
    let mut store = NativeStore::open(&path)?;
    let current = entry(execute(&mut store, request("/retained", &owner))?);
    assert_eq!(current.revision, "0");
    store.graph_read(|db| {
        assert_eq!(
            catalog
                .read(db, WORKSPACE, &owner)?
                .unwrap()
                .value
                .properties,
            json!({"record":{"id":owner,"root":"/retained"}})
        );
        Ok(())
    })?;
    sql(
        &mut store,
        "UPDATE nodes SET kind='wrong_kind' WHERE kind='desktop_workspace_binding_v1'",
    )?;
    assert_eq!(
        execute(&mut store, Request::ReadId { id: owner })
            .unwrap_err()
            .code,
        Code::UnsupportedDomainContract
    );
    Ok(())
}

#[test]
fn root_collision_and_revision_exhaustion_do_not_mutate() -> crate::CwpdbResult<()> {
    let mut store = NativeStore::open_in_memory()?;
    let owner = id(6);
    execute(&mut store, request("/scope", &owner))?;
    assert_eq!(
        execute(&mut store, request("/scope", &id(7)))
            .unwrap_err()
            .code,
        Code::AlreadyExists
    );
    assert_eq!(
        execute(&mut store, request("/other", &owner))
            .unwrap_err()
            .code,
        Code::EdgeConflict
    );
    sql(
        &mut store,
        "UPDATE nodes SET revision=9223372036854775807 WHERE kind='desktop_workspace_v1'",
    )?;
    assert_eq!(
        execute(
            &mut store,
            Request::Rebind {
                root: "/overflow".into(),
                id: owner.clone(),
                expected_revision: i64::MAX.to_string(),
                request_id: uuid::Uuid::new_v4().to_string()
            }
        )
        .unwrap_err()
        .code,
        Code::RevisionConflict
    );
    assert_eq!(
        entry(execute(&mut store, Request::ReadId { id: owner })?).root,
        "/scope"
    );
    Ok(())
}

#[test]
fn independent_connections_serialize_same_root_registration() -> crate::CwpdbResult<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("race.db");
    let first = NativeStore::open(&path)?;
    let second = NativeStore::open(&path)?;
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let handles = [first, second]
        .into_iter()
        .enumerate()
        .map(|(n, mut store)| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                execute(
                    &mut store,
                    Request::Ensure {
                        root: "/race".into(),
                        id: id(n as u64 + 10),
                        request_id: uuid::Uuid::new_v4().to_string(),
                    },
                )
            })
        })
        .collect::<Vec<_>>();
    let entries = handles
        .into_iter()
        .map(|handle| {
            entry(
                handle
                    .join()
                    .expect("registry worker")
                    .expect("registry result"),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(entries[0].id, entries[1].id);
    assert_eq!(entries[0].root, entries[1].root);
    Ok(())
}

#[test]
fn registry_reads_mutations_from_another_handle() -> crate::CwpdbResult<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("shared-registry.db");
    let mut first_store = NativeStore::open(&path)?;
    let mut second_store = NativeStore::open(&path)?;
    let owner = id(20);
    let registration = request("/shared", &owner);
    let first = entry(execute(&mut first_store, request_from(&registration))?);
    assert_eq!(
        entry(execute(&mut second_store, request_from(&registration))?),
        first
    );
    let moved = entry(execute(
        &mut second_store,
        Request::Rebind {
            root: "/moved".into(),
            id: owner.clone(),
            expected_revision: first.revision,
            request_id: uuid::Uuid::new_v4().to_string(),
        },
    )?);
    assert_eq!(
        execute(
            &mut first_store,
            Request::ReadRoot {
                root: "/shared".into(),
            },
        )?,
        Response::Read { entry: None }
    );
    assert_eq!(
        entry(execute(&mut first_store, Request::ReadId { id: owner })?),
        moved
    );
    Ok(())
}
