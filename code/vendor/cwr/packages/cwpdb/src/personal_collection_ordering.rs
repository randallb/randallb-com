//! Anchored moves preserve unrelated collections and reject stale destinations.
use super::*;

pub(super) fn move_collection(
    declarations: &Catalog,
    tx: &Transaction<'_>,
    scope_ref: &str,
    collections: &mut Vec<Collection>,
    collection_ref: &str,
    target_ref: &str,
    before: bool,
) -> CwpdbResult<bool> {
    let source = collections
        .iter()
        .position(|item| item.r#ref == collection_ref)
        .ok_or("collection is unavailable")?;
    let target = collections
        .iter()
        .position(|item| item.r#ref == target_ref)
        .ok_or("collection destination is unavailable")?;
    if source == target {
        return Ok(false);
    }
    let mut destination = target + usize::from(!before);
    if source < destination {
        destination -= 1;
    }
    if source == destination {
        return Ok(false);
    }
    let item = collections.remove(source);
    collections.insert(destination, item);
    replace_list(declarations, tx, scope_ref, collections)?;
    Ok(true)
}

pub(super) fn move_member(
    declarations: &Catalog,
    tx: &Transaction<'_>,
    scope: &Scope,
    collection_ref: &str,
    key: &PinKey,
    target: &PinKey,
    before: bool,
) -> CwpdbResult<bool> {
    let mut members = read_members(declarations, tx, scope, collection_ref)?.members;
    let reference = sidebar_pins::target_ref(declarations, key)?;
    let target_ref = sidebar_pins::target_ref(declarations, target)?;
    let source = members
        .iter()
        .position(|item| {
            sidebar_pins::target_ref(declarations, item).ok().as_ref() == Some(&reference)
        })
        .ok_or("collection member is unavailable")?;
    let target = members
        .iter()
        .position(|item| {
            sidebar_pins::target_ref(declarations, item).ok().as_ref() == Some(&target_ref)
        })
        .ok_or("collection destination is unavailable")?;
    if source == target {
        return Ok(false);
    }
    let mut destination = target + usize::from(!before);
    if source < destination {
        destination -= 1;
    }
    if source == destination {
        return Ok(false);
    }
    let item = members.remove(source);
    members.insert(destination, item);
    replace_members(declarations, tx, collection_ref, &members)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NativeStore;

    #[test]
    fn moves_preserve_identity_members_and_replay_after_reopen() {
        let declarations = &crate::desktop::test_catalog().unwrap();
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("collections.sqlite3");
        let scope = Scope {
            workspace: "/fixture".into(),
            viewer: "cwp://example.test/person".into(),
            owner_context_ref: "".into(),
        };
        let mut store = NativeStore::open(&path).unwrap();
        let (a, b, c) = store
            .graph_transaction(|tx| {
                let create = |name: &str| {
                    mutate_once(
                        declarations,
                        tx,
                        &scope,
                        Change::Create { name: name.into() },
                        name,
                    )
                    .map(|r| r.unwrap())
                };
                Ok((create("A")?, create("B")?, create("C")?))
            })
            .unwrap();
        let movement = Change::MoveCollection {
            collection_ref: c.clone(),
            target_ref: a.clone(),
            before: true,
        };
        store
            .graph_transaction(|tx| {
                mutate_once(declarations, tx, &scope, movement.clone(), "move")?;
                let list = read_list(declarations, tx, &scope)?;
                assert_eq!(
                    list.collections
                        .iter()
                        .map(|c| &c.r#ref)
                        .collect::<Vec<_>>(),
                    vec![&c, &a, &b]
                );
                let revision = list.revision;
                mutate_once(declarations, tx, &scope, movement.clone(), "move")?;
                mutate_once(declarations, tx, &scope, movement.clone(), "noop")?;
                assert_eq!(read_list(declarations, tx, &scope)?.revision, revision);
                assert!(mutate_once(
                    declarations,
                    tx,
                    &Scope {
                        viewer: "cwp://example.test/other".into(),
                        ..scope.clone()
                    },
                    movement.clone(),
                    "foreign"
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
                    declarations,
                    tx,
                    &scope,
                    Change::MoveCollection {
                        collection_ref: c.clone(),
                        target_ref: b.clone(),
                        before: false,
                    },
                    "reverse",
                )?;
                mutate_once(declarations, tx, &scope, movement, "move")?;
                assert_eq!(
                    read_list(declarations, tx, &scope)?
                        .collections
                        .iter()
                        .map(|c| &c.r#ref)
                        .collect::<Vec<_>>(),
                    vec![&a, &b, &c]
                );
                assert!(mutate_once(
                    declarations,
                    tx,
                    &scope,
                    Change::MoveCollection {
                        collection_ref: a,
                        target_ref: "cwp://example.test/missing".into(),
                        before: true
                    },
                    "stale"
                )
                .is_err());
                Ok(())
            })
            .unwrap();
    }
}

#[cfg(test)]
#[path = "personal_collection_member_order_tests.rs"]
mod member_tests;
