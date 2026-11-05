use super::*;
use crate::NativeStore;

#[test]
fn member_moves_are_scoped_idempotent_and_leave_unrelated_members_in_order() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("members.sqlite3");
    let scope = Scope {
        workspace: "/fixture".into(),
        viewer: "cwp://example.test/person".into(),
        owner_context_ref: "".into(),
    };
    let key = |name: &str| PinKey {
        source_ref: "cwp://example.test/source".into(),
        r#ref: format!("cwp://example.test/{name}"),
    };
    let (a, hidden, b, c) = (key("a"), key("hidden"), key("b"), key("c"));
    let mut store = NativeStore::open(&path).unwrap();
    let collection = store
        .graph_transaction(|tx| {
            let collection = mutate_once(
                &crate::desktop::test_catalog().unwrap(),
                tx,
                &scope,
                Change::Create {
                    name: "Work".into(),
                },
                "create",
            )?
            .unwrap();
            for (index, member) in [&a, &hidden, &b, &c].into_iter().enumerate() {
                mutate_once(
                    &crate::desktop::test_catalog().unwrap(),
                    tx,
                    &scope,
                    Change::SetMember {
                        collection_ref: collection.clone(),
                        key: member.clone(),
                        present: true,
                    },
                    &format!("add-{index}"),
                )?;
            }
            Ok(collection)
        })
        .unwrap();
    let movement = Change::MoveMember {
        collection_ref: collection.clone(),
        key: c.clone(),
        target: a.clone(),
        before: true,
    };
    store
        .graph_transaction(|tx| {
            mutate_once(
                &crate::desktop::test_catalog().unwrap(),
                tx,
                &scope,
                movement.clone(),
                "move",
            )?;
            let snapshot = read_members(
                &crate::desktop::test_catalog().unwrap(),
                tx,
                &scope,
                &collection,
            )?;
            assert_eq!(
                serde_json::to_value(&snapshot.members)?,
                json!([c, a, hidden, b])
            );
            mutate_once(
                &crate::desktop::test_catalog().unwrap(),
                tx,
                &scope,
                movement.clone(),
                "move",
            )?;
            mutate_once(
                &crate::desktop::test_catalog().unwrap(),
                tx,
                &scope,
                movement.clone(),
                "same-place",
            )?;
            assert_eq!(
                read_members(
                    &crate::desktop::test_catalog().unwrap(),
                    tx,
                    &scope,
                    &collection
                )?
                .revision,
                snapshot.revision
            );
            assert!(mutate_once(
                &crate::desktop::test_catalog().unwrap(),
                tx,
                &Scope {
                    viewer: "cwp://example.test/other".into(),
                    ..scope.clone()
                },
                movement.clone(),
                "foreign"
            )
            .is_err());
            assert!(mutate_once(
                &crate::desktop::test_catalog().unwrap(),
                tx,
                &scope,
                Change::MoveMember {
                    collection_ref: collection.clone(),
                    key: c.clone(),
                    target: key("missing"),
                    before: false
                },
                "missing"
            )
            .is_err());
            Ok(())
        })
        .unwrap();
    drop(store);
    let mut store = NativeStore::open(&path).unwrap();
    store
        .graph_transaction(|tx| {
            mutate_once(
                &crate::desktop::test_catalog().unwrap(),
                tx,
                &scope,
                Change::MoveMember {
                    collection_ref: collection.clone(),
                    key: c.clone(),
                    target: b.clone(),
                    before: false,
                },
                "reverse",
            )?;
            mutate_once(
                &crate::desktop::test_catalog().unwrap(),
                tx,
                &scope,
                movement,
                "move",
            )?;
            assert_eq!(
                serde_json::to_value(
                    read_members(
                        &crate::desktop::test_catalog().unwrap(),
                        tx,
                        &scope,
                        &collection
                    )?
                    .members
                )?,
                json!([a, hidden, b, c])
            );
            Ok(())
        })
        .unwrap();
}
