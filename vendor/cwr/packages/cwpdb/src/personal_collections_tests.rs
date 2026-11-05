use super::*;
use crate::NativeStore;

fn scope(owner: &str) -> Scope {
    Scope {
        workspace: "/fixture".into(),
        viewer: "cwp://example.test/users/person".into(),
        owner_context_ref: owner.into(),
    }
}
fn key(name: &str) -> PinKey {
    PinKey {
        source_ref: "cwp://example.test/sources/root".into(),
        r#ref: format!("cwp://example.test/conversations/{name}"),
    }
}
fn change(tx: &Transaction<'_>, scope: &Scope, change: Change, request: &str) -> String {
    mutate_once(
        &crate::desktop::test_catalog().unwrap(),
        tx,
        scope,
        change,
        request,
    )
    .unwrap()
    .unwrap()
}

#[test]
fn named_collections_scope_members_and_rename_without_changing_pins() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("graph.sqlite3");
    let mut store = NativeStore::open(&path).unwrap();
    let owner = scope("urn:uuid:11111111-1111-4111-8111-111111111111");
    store
        .graph_transaction(|tx| {
            let pin_scope = sidebar_pins::PinScope {
                workspace: owner.workspace.clone(),
                viewer: "cwp://example.test/actors/current".into(),
            };
            sidebar_pins::replace_in_transaction(
                &crate::desktop::test_catalog().unwrap(),
                tx,
                &pin_scope,
                "0",
                &[key("old")],
            )?;
            let id = change(
                tx,
                &owner,
                Change::Create {
                    name: " Research ".into(),
                },
                "create",
            );
            assert_eq!(
                read_list(&crate::desktop::test_catalog().unwrap(), tx, &owner)?.collections[0]
                    .name,
                "Research"
            );
            assert_eq!(
                read_list(
                    &crate::desktop::test_catalog().unwrap(),
                    tx,
                    &scope("urn:uuid:22222222-2222-4222-8222-222222222222")
                )?
                .collections
                .len(),
                0
            );
            assert_eq!(
                read_list(
                    &crate::desktop::test_catalog().unwrap(),
                    tx,
                    &Scope {
                        viewer: "cwp://example.test/users/other".into(),
                        ..owner.clone()
                    }
                )?
                .collections
                .len(),
                0
            );
            change(
                tx,
                &owner,
                Change::SetMember {
                    collection_ref: id.clone(),
                    key: key("old"),
                    present: true,
                },
                "add-old",
            );
            change(
                tx,
                &owner,
                Change::SetMember {
                    collection_ref: id.clone(),
                    key: key("new"),
                    present: true,
                },
                "add-new",
            );
            change(
                tx,
                &owner,
                Change::Rename {
                    collection_ref: id.clone(),
                    name: "Ideas".into(),
                },
                "rename",
            );
            assert_eq!(
                read_list(&crate::desktop::test_catalog().unwrap(), tx, &owner)?.collections[0]
                    .r#ref,
                id
            );
            assert_eq!(
                read_members(&crate::desktop::test_catalog().unwrap(), tx, &owner, &id)?.members,
                vec![key("old"), key("new")]
            );
            assert_eq!(
                sidebar_pins::read(&crate::desktop::test_catalog().unwrap(), tx, &pin_scope)?
                    .members,
                vec![key("old")]
            );
            change(
                tx,
                &owner,
                Change::SetMember {
                    collection_ref: id.clone(),
                    key: key("old"),
                    present: false,
                },
                "remove-old",
            );
            assert_eq!(
                read_members(&crate::desktop::test_catalog().unwrap(), tx, &owner, &id)?.members,
                vec![key("new")]
            );
            change(
                tx,
                &owner,
                Change::Delete {
                    collection_ref: id.clone(),
                },
                "delete",
            );
            assert!(
                read_list(&crate::desktop::test_catalog().unwrap(), tx, &owner)?
                    .collections
                    .is_empty()
            );
            assert!(
                read_members(&crate::desktop::test_catalog().unwrap(), tx, &owner, &id).is_err()
            );
            assert_eq!(
                sidebar_pins::read(&crate::desktop::test_catalog().unwrap(), tx, &pin_scope)?
                    .members,
                vec![key("old")]
            );
            Ok(())
        })
        .unwrap();
    drop(store);
    let mut reopened = NativeStore::open(&path).unwrap();
    reopened
        .graph_read_transaction(|tx| {
            assert!(
                read_list(&crate::desktop::test_catalog().unwrap(), tx, &owner)?
                    .collections
                    .is_empty()
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn collection_scope_accepts_installed_pack_uuid_and_rejects_malformed_owner() {
    let installed = scope("urn:uuid:11111111-1111-4111-8111-111111111111");
    assert_ne!(
        scope_ref(&crate::desktop::test_catalog().unwrap(), &installed).unwrap(),
        scope_ref(&crate::desktop::test_catalog().unwrap(), &scope("")).unwrap()
    );
    assert!(scope_ref(
        &crate::desktop::test_catalog().unwrap(),
        &scope("cwp://example.test/contexts/legacy-pack")
    )
    .is_ok());
    for invalid in [
        "urn:uuid:fixture",
        "urn:uuid:11111111111141118111111111111111",
        "urn:uuid:11111111-1111-4111-8111-111111111111/../other",
    ] {
        assert!(scope_ref(&crate::desktop::test_catalog().unwrap(), &scope(invalid)).is_err());
    }
}

#[test]
fn collection_names_and_replay_are_stable() {
    let mut store = NativeStore::open_in_memory().unwrap();
    let scope = scope("");
    store
        .graph_transaction(|tx| {
            assert!(mutate_once(
                &crate::desktop::test_catalog().unwrap(),
                tx,
                &scope,
                Change::Create {
                    name: "Pinned".into()
                },
                "reserved"
            )
            .is_err());
            let first = change(
                tx,
                &scope,
                Change::Create {
                    name: "Planning".into(),
                },
                "create",
            );
            let revision =
                read_list(&crate::desktop::test_catalog().unwrap(), tx, &scope)?.revision;
            assert_eq!(
                change(
                    tx,
                    &scope,
                    Change::Create {
                        name: "Planning".into()
                    },
                    "create"
                ),
                first
            );
            assert_eq!(
                read_list(&crate::desktop::test_catalog().unwrap(), tx, &scope)?.revision,
                revision
            );
            assert!(mutate_once(
                &crate::desktop::test_catalog().unwrap(),
                tx,
                &scope,
                Change::Create {
                    name: "planning".into()
                },
                "duplicate"
            )
            .is_err());
            assert!(mutate_once(
                &crate::desktop::test_catalog().unwrap(),
                tx,
                &scope,
                Change::Rename {
                    collection_ref: first.clone(),
                    name: "recent".into()
                },
                "reserved-rename"
            )
            .is_err());
            change(
                tx,
                &scope,
                Change::SetMember {
                    collection_ref: first.clone(),
                    key: key("one"),
                    present: true,
                },
                "add",
            );
            let revision =
                read_list(&crate::desktop::test_catalog().unwrap(), tx, &scope)?.revision;
            change(
                tx,
                &scope,
                Change::SetMember {
                    collection_ref: first.clone(),
                    key: key("one"),
                    present: true,
                },
                "add-again",
            );
            assert_eq!(
                read_list(&crate::desktop::test_catalog().unwrap(), tx, &scope)?.revision,
                revision
            );
            assert!(mutate_once(
                &crate::desktop::test_catalog().unwrap(),
                tx,
                &scope,
                Change::Delete {
                    collection_ref: first.clone()
                },
                "add"
            )
            .is_err());
            assert_eq!(
                read_members(&crate::desktop::test_catalog().unwrap(), tx, &scope, &first)?.members,
                vec![key("one")]
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn adding_to_another_collection_moves_membership() {
    let root = tempfile::tempdir().unwrap();
    let mut store = NativeStore::open(root.path().join("graph.sqlite3")).unwrap();
    let owner = scope("urn:uuid:11111111-1111-4111-8111-111111111111");
    store
        .graph_transaction(|tx| {
            let first = change(
                tx,
                &owner,
                Change::Create {
                    name: "First".into(),
                },
                "first",
            );
            let second = change(
                tx,
                &owner,
                Change::Create {
                    name: "Second".into(),
                },
                "second",
            );
            change(
                tx,
                &owner,
                Change::SetMember {
                    collection_ref: first.clone(),
                    key: key("one"),
                    present: true,
                },
                "add",
            );
            change(
                tx,
                &owner,
                Change::SetMember {
                    collection_ref: second.clone(),
                    key: key("one"),
                    present: true,
                },
                "move",
            );
            assert!(
                read_members(&crate::desktop::test_catalog().unwrap(), tx, &owner, &first)?
                    .members
                    .is_empty()
            );
            assert_eq!(
                read_members(
                    &crate::desktop::test_catalog().unwrap(),
                    tx,
                    &owner,
                    &second
                )?
                .members,
                vec![key("one")]
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn assigning_an_already_included_legacy_target_normalizes_other_memberships() {
    let mut store = NativeStore::open_in_memory().unwrap();
    store
        .graph_transaction(|tx| {
            let owner = scope("");
            let a = change(tx, &owner, Change::Create { name: "A".into() }, "create-a");
            let b = change(tx, &owner, Change::Create { name: "B".into() }, "create-b");
            sidebar_pins::ensure_target(
                &crate::desktop::test_catalog().unwrap(),
                tx,
                &key("legacy"),
            )?;
            for selected in [&a, &b] {
                // Model pre-exclusivity storage without passing through normalization.
                replace_members(
                    &crate::desktop::test_catalog().unwrap(),
                    tx,
                    &a,
                    &[key("legacy")],
                )?;
                replace_members(
                    &crate::desktop::test_catalog().unwrap(),
                    tx,
                    &b,
                    &[key("legacy")],
                )?;
                change(
                    tx,
                    &owner,
                    Change::SetMember {
                        collection_ref: selected.clone(),
                        key: key("legacy"),
                        present: true,
                    },
                    if selected == &a {
                        "assign-a"
                    } else {
                        "assign-b"
                    },
                );
                assert_eq!(
                    read_members(
                        &crate::desktop::test_catalog().unwrap(),
                        tx,
                        &owner,
                        selected
                    )?
                    .members,
                    vec![key("legacy")]
                );
                let other = if selected == &a { &b } else { &a };
                assert!(
                    read_members(&crate::desktop::test_catalog().unwrap(), tx, &owner, other)?
                        .members
                        .is_empty()
                );
            }
            Ok(())
        })
        .unwrap();
}
