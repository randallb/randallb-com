use super::*;
use crate::{
    sidebar_pins::{read, replace_in_transaction},
    NativeStore,
};
fn scope() -> PinScope {
    PinScope {
        workspace: "/fixture".into(),
        viewer: "cwp://example.test/viewer".into(),
    }
}
fn key(value: &str) -> PinKey {
    PinKey {
        source_ref: "cwp://example.test/source".into(),
        r#ref: format!("cwp://example.test/{value}"),
    }
}
fn move_fixture(
    tx: &rusqlite::Transaction<'_>,
    scope: &PinScope,
    owner_key: &str,
    key: &PinKey,
    direction: Direction,
    eligible: &[PinKey],
    request_id: &str,
) -> CwpdbResult<PinReplacement> {
    super::move_once_in_transaction(
        &crate::desktop::test_catalog().unwrap(),
        tx,
        scope,
        RelativeMove {
            anchor: None,
            owner_key,
            key,
            direction,
            include_archived: false,
        },
        eligible,
        request_id,
    )
}
#[test]
fn durable_move_replays_after_reopen_and_eligibility_changes_without_reapplying() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("pins.db");
    let mut store = NativeStore::open(&path).unwrap();
    let original = vec![key("a"), key("foreign"), key("b"), key("c")];
    let result = store
        .graph_transaction(|tx| {
            replace_in_transaction(
                &crate::desktop::test_catalog().unwrap(),
                tx,
                &scope(),
                "0",
                &original,
            )?;
            let result = move_fixture(
                tx,
                &scope(),
                "",
                &key("c"),
                Direction::Up,
                &[key("a"), key("b"), key("c")],
                "move",
            )?;
            assert_eq!(
                read(&crate::desktop::test_catalog().unwrap(), tx, &scope())?.members,
                vec![key("a"), key("foreign"), key("c"), key("b")]
            );
            Ok(serde_json::to_value(result)?)
        })
        .unwrap();
    drop(store);
    let mut store = NativeStore::open(&path).unwrap();
    store
        .graph_transaction(|tx| {
            let before = read(&crate::desktop::test_catalog().unwrap(), tx, &scope())?;
            let mut members = before.members;
            members.push(key("new"));
            replace_in_transaction(
                &crate::desktop::test_catalog().unwrap(),
                tx,
                &scope(),
                &before.revision,
                &members,
            )?;
            let replay = move_fixture(
                tx,
                &scope(),
                "",
                &key("c"),
                Direction::Up,
                &[key("new"), key("c")],
                "move",
            )?;
            assert_eq!(serde_json::to_value(replay)?, result);
            assert_eq!(
                read(&crate::desktop::test_catalog().unwrap(), tx, &scope())?.members,
                members
            );
            for (owner, target, direction) in [
                ("cwp://example.test/pack", key("c"), Direction::Up),
                ("", key("b"), Direction::Up),
                ("", key("c"), Direction::Down),
            ] {
                let error = move_fixture(tx, &scope(), owner, &target, direction, &members, "move")
                    .unwrap_err();
                assert!(error.is::<sequence_receipt::RequestConflict>());
            }
            let error = super::move_once_in_transaction(
                &crate::desktop::test_catalog().unwrap(),
                tx,
                &scope(),
                RelativeMove {
                    anchor: None,
                    owner_key: "",
                    key: &key("c"),
                    direction: Direction::Up,
                    include_archived: true,
                },
                &members,
                "move",
            )
            .unwrap_err();
            assert!(error.is::<sequence_receipt::RequestConflict>());
            let other_source = PinKey {
                source_ref: "cwp://example.test/other-source".into(),
                ..key("c")
            };
            let error = move_fixture(
                tx,
                &scope(),
                "",
                &other_source,
                Direction::Up,
                &members,
                "move",
            )
            .unwrap_err();
            assert!(error.is::<sequence_receipt::RequestConflict>());
            sidebar_pins::replace_once_in_transaction(
                &crate::desktop::test_catalog().unwrap(),
                tx,
                &scope(),
                "3",
                &members,
                "replace-first",
            )?;
            let error = move_fixture(
                tx,
                &scope(),
                "",
                &key("c"),
                Direction::Up,
                &members,
                "replace-first",
            )
            .unwrap_err();
            assert!(error.is::<sequence_receipt::RequestConflict>());
            let error = sidebar_pins::replace_once_in_transaction(
                &crate::desktop::test_catalog().unwrap(),
                tx,
                &scope(),
                "3",
                &members,
                "move",
            )
            .unwrap_err();
            assert!(error.is::<sequence_receipt::RequestConflict>());
            assert_eq!(
                read(&crate::desktop::test_catalog().unwrap(), tx, &scope())?.members,
                members
            );
            tx.execute(
                "DELETE FROM edges WHERE kind='sequence_mutation_receipt_v1'",
                [],
            )?;
            assert!(matches!(
                move_fixture(tx, &scope(), "", &key("c"), Direction::Up, &members, "move")?,
                PinReplacement::CommittedResultExpired
            ));
            assert_eq!(
                read(&crate::desktop::test_catalog().unwrap(), tx, &scope())?.members,
                members
            );
            Ok(())
        })
        .unwrap();
}
#[test]
fn failed_move_rolls_back_effect_and_receipt_and_hidden_pins_are_not_neighbors() {
    let mut store = NativeStore::open_in_memory().unwrap();
    store.graph_transaction(|tx| {
        replace_in_transaction(&crate::desktop::test_catalog().unwrap(), tx,&scope(),"0",&[key("hidden"),key("a"),key("foreign"),key("b")])?;
        tx.execute_batch("CREATE TRIGGER fail_pin_move BEFORE INSERT ON edges WHEN NEW.kind='sequence_mutation_receipt_v1' BEGIN SELECT RAISE(ABORT,'injected receipt failure'); END")?;
        Ok(())
    }).unwrap();
    assert!(store
        .graph_transaction(|tx| move_fixture(
            tx,
            &scope(),
            "",
            &key("b"),
            Direction::Up,
            &[key("a"), key("b")],
            "retry"
        ))
        .is_err());
    store
        .graph_transaction(|tx| {
            assert_eq!(
                read(&crate::desktop::test_catalog().unwrap(), tx, &scope())?.members,
                vec![key("hidden"), key("a"), key("foreign"), key("b")]
            );
            tx.execute_batch("DROP TRIGGER fail_pin_move")?;
            move_fixture(
                tx,
                &scope(),
                "",
                &key("b"),
                Direction::Up,
                &[key("a"), key("b")],
                "retry",
            )?;
            assert_eq!(
                read(&crate::desktop::test_catalog().unwrap(), tx, &scope())?.members,
                vec![key("hidden"), key("b"), key("foreign"), key("a")]
            );
            assert!(move_fixture(
                tx,
                &scope(),
                "",
                &key("b"),
                Direction::Up,
                &[key("b"), key("a")],
                "boundary"
            )
            .is_err());
            assert!(move_fixture(
                tx,
                &scope(),
                "",
                &key("hidden"),
                Direction::Down,
                &[key("b"), key("a")],
                "foreign"
            )
            .is_err());
            Ok(())
        })
        .unwrap();
}

#[test]
fn anchored_moves_preserve_hidden_slots_and_recover_without_reapplying() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("anchor.db");
    let mut store = NativeStore::open(&path).unwrap();
    let allowed = vec![key("a"), key("b"), key("c")];
    let perform =
        |tx: &rusqlite::Transaction<'_>, source: &PinKey, target: &PinKey, before, id: &str| {
            super::move_once_in_transaction(
                &crate::desktop::test_catalog().unwrap(),
                tx,
                &scope(),
                RelativeMove {
                    owner_key: "",
                    key: source,
                    direction: Direction::Down,
                    include_archived: false,
                    anchor: Some((target, before)),
                },
                &allowed,
                id,
            )
        };
    store
        .graph_transaction(|tx| {
            replace_in_transaction(
                &crate::desktop::test_catalog().unwrap(),
                tx,
                &scope(),
                "0",
                &[key("a"), key("hidden"), key("b"), key("foreign"), key("c")],
            )?;
            perform(tx, &key("c"), &key("a"), true, "first")?;
            assert_eq!(
                read(&crate::desktop::test_catalog().unwrap(), tx, &scope())?.members,
                vec![key("c"), key("hidden"), key("a"), key("foreign"), key("b")]
            );
            perform(tx, &key("c"), &key("b"), false, "last")?;
            assert_eq!(
                read(&crate::desktop::test_catalog().unwrap(), tx, &scope())?.members,
                vec![key("a"), key("hidden"), key("b"), key("foreign"), key("c")]
            );
            assert!(perform(tx, &key("c"), &key("hidden"), true, "denied").is_err());
            assert!(perform(tx, &key("c"), &key("b"), true, "first").is_err());
            Ok(())
        })
        .unwrap();
    drop(store);
    let mut store = NativeStore::open(&path).unwrap();
    store
        .graph_transaction(|tx| {
            let original = read(&crate::desktop::test_catalog().unwrap(), tx, &scope())?.members;
            perform(tx, &key("c"), &key("a"), true, "first")?;
            assert_eq!(
                read(&crate::desktop::test_catalog().unwrap(), tx, &scope())?.members,
                original
            );
            Ok(())
        })
        .unwrap();
}
