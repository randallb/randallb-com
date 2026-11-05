use crate::NativeStore;
fn execute(store: &mut NativeStore, request: Request) -> Result<Response> {
    super::execute(
        store,
        &super::super::test_catalog().map_err(storage)?,
        request,
    )
}
use super::*;
fn uid() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn initial() -> Instance {
    Instance {
        id: "workspace-one".into(),
        name: "Fixture".into(),
        folders: Some(vec![]),
    }
}
fn version(id: &str, folders: &[&str]) -> Version {
    Version {
        id: id.into(),
        files: vec![File {
            path: "CWR.toml".into(),
            data: "Zml4dHVyZQ==".into(),
            mode: 420,
        }],
        folders: folders.iter().map(|value| (*value).into()).collect(),
    }
}
fn save(store: &mut NativeStore, root: &str) -> Result<()> {
    execute(
        store,
        Request::Save {
            root: root.into(),
            instance: initial(),
            expected_revision: None,
            request_id: uid(),
        },
    )?;
    Ok(())
}
fn publish(store: &mut NativeStore, root: &str, version: Version) -> Result<()> {
    execute(
        store,
        Request::Publish {
            root: root.into(),
            version,
            import: false,
            request_id: uid(),
        },
    )?;
    Ok(())
}
fn select(store: &mut NativeStore, root: &str, id: &str, expected: Option<&str>) -> Result<()> {
    execute(
        store,
        Request::Select {
            root: root.into(),
            version: id.into(),
            expected_active: expected.map(str::to_string),
            policy: SelectionPolicy::Compare,
            request_id: uid(),
        },
    )?;
    Ok(())
}
fn state(store: &mut NativeStore, root: &str) -> (Instance, String) {
    match execute(store, Request::ReadInstance { root: root.into() }).unwrap() {
        Response::Instance {
            instance: Some(instance),
            revision: Some(revision),
            ..
        } => (instance, revision),
        _ => panic!("expected instance"),
    }
}
#[test]
fn immutable_publication_selection_and_more_than_500_scoped_members() -> crate::CwpdbResult<()> {
    let mut store = NativeStore::open_in_memory()?;
    save(&mut store, "/scope")?;
    let mut first = version("one", &["/duplicate", "/duplicate", "/missing"]);
    first
        .folders
        .extend((0..501).map(|index| format!("/folder/{index}")));
    publish(&mut store, "/scope", first.clone())?;
    assert_eq!(state(&mut store, "/scope").0.folders, Some(vec![]));
    select(&mut store, "/scope", "one", None)?;
    assert_eq!(
        state(&mut store, "/scope").0.folders,
        Some(first.folders.clone())
    );
    store.graph_read(|db| {
        let catalog = Catalog::new(&super::super::test_catalog()?)?;
        assert!(catalog.ready(db, "/scope", &initial(), &first)?);
        assert_eq!(
            unordered::many_outgoing(db, MEMBER_EDGE, &version_ref("/scope", "one"), MAX_WORK)?
                .len(),
            503
        );
        Ok(())
    })?;
    let mut changed = first.clone();
    changed.folders.clear();
    assert_eq!(
        execute(
            &mut store,
            Request::Publish {
                root: "/scope".into(),
                version: changed,
                import: true,
                request_id: uid()
            }
        )
        .unwrap_err()
        .code,
        Code::EdgeConflict
    );
    assert_eq!(
        execute(
            &mut store,
            Request::Publish {
                root: "/scope".into(),
                version: first,
                import: false,
                request_id: uid()
            }
        )
        .unwrap_err()
        .code,
        Code::AlreadyExists
    );
    Ok(())
}
#[test]
fn stale_selection_and_attempted_mutable_grants_do_not_overwrite_authority(
) -> crate::CwpdbResult<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("versions.db");
    let mut first = NativeStore::open(&path)?;
    save(&mut first, "/scope")?;
    publish(&mut first, "/scope", version("one", &["/one"]))?;
    select(&mut first, "/scope", "one", None)?;
    publish(&mut first, "/scope", version("two", &["/two"]))?;
    let mut peer = NativeStore::open(&path)?;
    select(&mut peer, "/scope", "two", Some("one"))?;
    assert_eq!(
        select(&mut first, "/scope", "one", Some("one"))
            .unwrap_err()
            .code,
        Code::RevisionConflict
    );
    let (mut instance, revision) = state(&mut first, "/scope");
    assert_eq!(instance.folders, Some(vec!["/two".into()]));
    instance.folders = Some(vec!["/forged".into()]);
    assert_eq!(
        execute(
            &mut first,
            Request::Save {
                root: "/scope".into(),
                instance,
                expected_revision: Some(revision),
                request_id: uid()
            }
        )
        .unwrap_err()
        .code,
        Code::InvalidValue
    );
    assert_eq!(
        state(&mut first, "/scope").0.folders,
        Some(vec!["/two".into()])
    );
    save(&mut first, "/other")?;
    assert_eq!(
        select(&mut first, "/other", "two", None).unwrap_err().code,
        Code::NotFound
    );
    Ok(())
}
#[test]
fn failed_ready_publication_rolls_back_version_grants_edges_and_receipt() -> crate::CwpdbResult<()>
{
    let mut store = NativeStore::open_in_memory()?;
    save(&mut store, "/scope")?;
    store.graph_transaction(|tx|{tx.execute_batch("CREATE TRIGGER inject_ready BEFORE INSERT ON nodes WHEN NEW.kind='desktop_version_membership_v1' BEGIN SELECT RAISE(ABORT,'injected'); END")?;Ok(())})?;
    let request = Request::Publish {
        root: "/scope".into(),
        version: version("candidate", &["/new"]),
        import: false,
        request_id: uid(),
    };
    let clone_request = || serde_json::from_value(serde_json::to_value(&request).unwrap()).unwrap();
    assert_eq!(
        execute(&mut store, clone_request()).unwrap_err().code,
        Code::StorageFailure
    );
    store.graph_read(|db| {
        assert!(!cwpdb::record_exists(
            db,
            &version_ref("/scope", "candidate")
        )?);
        assert!(!unordered::retired(
            db,
            &version_ref("/scope", "candidate")
        )?);
        assert!(
            unordered::many_outgoing(db, GRANT_EDGE, &instance_ref("/scope"), MAX_WORK)?.is_empty()
        );
        Ok(())
    })?;
    store.graph_transaction(|tx| {
        tx.execute_batch("DROP TRIGGER inject_ready")?;
        Ok(())
    })?;
    let result = execute(&mut store, clone_request())?;
    assert_eq!(execute(&mut store, clone_request())?, result);
    Ok(())
}
#[test]
fn retained_arrays_remain_authoritative_until_recovery_and_reopen() -> crate::CwpdbResult<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("retained.db");
    let mut store = NativeStore::open(&path)?;
    save(&mut store, "/scope")?;
    let retained = version("retained", &["/missing/../alias", "/another-missing"]);
    store.graph_transaction(|tx| {
        cwpdb::put_node_with_schema(
            tx,
            &version_ref("/scope", "retained"),
            VERSION_KIND,
            VERSION_EDGE.schema,
            &json!({"record":retained}),
        )?;
        unordered::link(
            tx,
            VERSION_EDGE,
            &instance_ref("/scope"),
            &version_ref("/scope", "retained"),
        )?;
        unordered::link(
            tx,
            ACTIVE_EDGE,
            &instance_ref("/scope"),
            &version_ref("/scope", "retained"),
        )?;
        Ok(())
    })?;
    match execute(
        &mut store,
        Request::ReadInstance {
            root: "/scope".into(),
        },
    )? {
        Response::Instance {
            instance: Some(instance),
            membership_ready: false,
            ..
        } => assert_eq!(instance.folders, Some(retained.folders.clone())),
        _ => panic!("expected pending membership"),
    };
    execute(
        &mut store,
        Request::Recover {
            root: "/scope".into(),
            version: None,
        },
    )?;
    drop(store);
    let mut reopened = NativeStore::open_read_only(&path)?;
    match execute(
        &mut reopened,
        Request::ReadInstance {
            root: "/scope".into(),
        },
    )? {
        Response::Instance {
            instance: Some(instance),
            membership_ready: true,
            ..
        } => assert_eq!(instance.folders, Some(retained.folders)),
        _ => panic!("expected complete retained membership"),
    };
    Ok(())
}
#[test]
fn removal_erases_payloads_and_retires_versions_without_cascading_unknown_edges(
) -> crate::CwpdbResult<()> {
    let mut store = NativeStore::open_in_memory()?;
    save(&mut store, "/scope")?;
    publish(&mut store, "/scope", version("one", &["/private-folder"]))?;
    select(&mut store, "/scope", "one", None)?;
    store.graph_transaction(|tx| {
        cwpdb::put_edge(
            tx,
            &instance_ref("/scope"),
            "foreign",
            "cwp://fixture/foreign",
            0,
            &json!({}),
        )?;
        Ok(())
    })?;
    let (_, revision) = state(&mut store, "/scope");
    assert_eq!(
        execute(
            &mut store,
            Request::Remove {
                root: "/scope".into(),
                expected_revision: revision.clone(),
                request_id: uid()
            }
        )
        .unwrap_err()
        .code,
        Code::RelationshipsExist
    );
    assert_eq!(state(&mut store, "/scope").1, revision);
    store.graph_transaction(|tx| {
        cwpdb::remove_edge(
            tx,
            &instance_ref("/scope"),
            "foreign",
            "cwp://fixture/foreign",
        )?;
        Ok(())
    })?;
    execute(
        &mut store,
        Request::Remove {
            root: "/scope".into(),
            expected_revision: revision,
            request_id: uid(),
        },
    )?;
    assert!(matches!(
        execute(
            &mut store,
            Request::ReadInstance {
                root: "/scope".into()
            }
        )?,
        Response::Instance { instance: None, .. }
    ));
    store.graph_read(|db| {
        assert!(unordered::retired(db, &version_ref("/scope", "one"))?);
        let payload: String =
            db.query_row("SELECT group_concat(json(props)) FROM nodes", [], |r| {
                r.get(0)
            })?;
        assert!(!payload.contains("/private-folder"));
        Ok(())
    })?;
    Ok(())
}
