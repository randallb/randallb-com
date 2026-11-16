use super::*;
use crate::NativeStore;
fn scope() -> PinScope {
    PinScope {
        workspace: "/fixture".into(),
        viewer: "cwp://com.example/viewer".into(),
    }
}
fn key(source: &str) -> PinKey {
    PinKey {
        source_ref: format!("cwp://com.example/{source}"),
        r#ref: "cwp://com.example/conversation".into(),
    }
}
#[test]
fn pins_preserve_sources_scopes_noops_stale_order_and_reopen() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("db");
    let mut db = NativeStore::open(&path).unwrap();
    db.graph_transaction(|tx| {
        let first = replace_in_transaction(
            &crate::desktop::test_catalog().unwrap(),
            tx,
            &scope(),
            "0",
            &[key("a"), key("b")],
        )?;
        assert_eq!(first.revision, "1");
        assert_eq!(
            replace_in_transaction(
                &crate::desktop::test_catalog().unwrap(),
                tx,
                &scope(),
                "1",
                &first.members
            )?
            .revision,
            "1"
        );
        assert!(replace_in_transaction(
            &crate::desktop::test_catalog().unwrap(),
            tx,
            &scope(),
            "0",
            &first.members
        )
        .is_err());
        let second = replace_in_transaction(
            &crate::desktop::test_catalog().unwrap(),
            tx,
            &scope(),
            "1",
            &[key("b"), key("a")],
        )?;
        assert_eq!(second.revision, "2");
        assert!(read(
            &crate::desktop::test_catalog().unwrap(),
            tx,
            &PinScope {
                viewer: "cwp://com.example/other".into(),
                ..scope()
            }
        )?
        .members
        .is_empty());
        assert!(read(
            &crate::desktop::test_catalog().unwrap(),
            tx,
            &PinScope {
                workspace: "/other".into(),
                ..scope()
            }
        )?
        .members
        .is_empty());
        Ok(())
    })
    .unwrap();
    drop(db);
    let mut db = NativeStore::open(&path).unwrap();
    db.graph_transaction(|tx| {
        assert_eq!(
            read(&crate::desktop::test_catalog().unwrap(), tx, &scope())?.members,
            vec![key("b"), key("a")]
        );
        Ok(())
    })
    .unwrap();
}
#[test]
fn malformed_duplicates_and_midoperation_failures_leave_no_partial_order() {
    let root = tempfile::tempdir().unwrap();
    let mut db = NativeStore::open(root.path().join("db")).unwrap();
    db.graph_transaction(|tx| {
        assert!(replace_in_transaction(
            &crate::desktop::test_catalog().unwrap(),
            tx,
            &scope(),
            "0",
            &[key("a"), key("a")]
        )
        .is_err());
        assert!(replace_in_transaction(
            &crate::desktop::test_catalog().unwrap(),
            tx,
            &scope(),
            "0",
            &[PinKey {
                r#ref: "bad".into(),
                ..key("a")
            }]
        )
        .is_err());
        replace_in_transaction(
            &crate::desktop::test_catalog().unwrap(),
            tx,
            &scope(),
            "0",
            &[key("a")],
        )?;
        Ok(())
    })
    .unwrap();
    let result: CwpdbResult<()> = db.graph_transaction(|tx| {
        replace_in_transaction(
            &crate::desktop::test_catalog().unwrap(),
            tx,
            &scope(),
            "1",
            &[key("b"), key("a")],
        )?;
        Err("injected failure before commit".into())
    });
    assert!(result.is_err());
    db.graph_transaction(|tx| {
        let state = read(&crate::desktop::test_catalog().unwrap(), tx, &scope())?;
        assert_eq!(state.revision, "1");
        assert_eq!(state.members, vec![key("a")]);
        Ok(())
    })
    .unwrap();
}

#[test]
fn sqlite_failure_mid_reorder_and_competing_connection_preserve_committed_order() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("db");
    let mut first = NativeStore::open(&path).unwrap();
    let mut peer = NativeStore::open(&path).unwrap();
    first.graph_transaction(|tx|{replace_in_transaction(&crate::desktop::test_catalog().unwrap(), tx,&scope(),"0",&[key("a")])?;
        tx.execute_batch("CREATE TRIGGER fail_pin_order BEFORE INSERT ON edges WHEN NEW.kind='desktop_sidebar_pin_v1' AND NEW.position=1 BEGIN SELECT RAISE(ABORT,'injected mid-reorder failure'); END")?;Ok(())}).unwrap();
    assert!(peer
        .graph_transaction(|tx| replace_in_transaction(
            &crate::desktop::test_catalog().unwrap(),
            tx,
            &scope(),
            "1",
            &[key("b"), key("a")]
        ))
        .is_err());
    first
        .graph_transaction(|tx| {
            let current = read(&crate::desktop::test_catalog().unwrap(), tx, &scope())?;
            assert_eq!(current.revision, "1");
            assert_eq!(current.members, vec![key("a")]);
            tx.execute_batch("DROP TRIGGER fail_pin_order")?;
            Ok(())
        })
        .unwrap();
    peer.graph_transaction(|tx| {
        replace_in_transaction(
            &crate::desktop::test_catalog().unwrap(),
            tx,
            &scope(),
            "1",
            &[key("a"), key("b")],
        )
    })
    .unwrap();
    assert!(first
        .graph_transaction(|tx| replace_in_transaction(
            &crate::desktop::test_catalog().unwrap(),
            tx,
            &scope(),
            "1",
            &[key("b"), key("a")]
        ))
        .is_err());
}

#[test]
fn exact_pin_request_replay_precedes_stale_revision_and_changed_arguments_conflict() {
    let root = tempfile::tempdir().unwrap();
    let mut store = NativeStore::open(root.path().join("db")).unwrap();
    store
        .graph_transaction(|tx| {
            let first = replace_once_in_transaction(
                &crate::desktop::test_catalog().unwrap(),
                tx,
                &scope(),
                "0",
                &[key("a")],
                "pin_request",
            )?;
            assert!(matches!(first, PinReplacement::Committed { .. }));
            replace_in_transaction(
                &crate::desktop::test_catalog().unwrap(),
                tx,
                &scope(),
                "1",
                &[key("b")],
            )?;
            let replay = replace_once_in_transaction(
                &crate::desktop::test_catalog().unwrap(),
                tx,
                &scope(),
                "0",
                &[key("a")],
                "pin_request",
            )?;
            match replay {
                PinReplacement::Committed { snapshot } => {
                    assert_eq!(snapshot.revision, "1");
                    assert_eq!(snapshot.members, vec![key("a")]);
                }
                _ => panic!("missing original result"),
            }
            assert_eq!(
                read(&crate::desktop::test_catalog().unwrap(), tx, &scope())?.members,
                vec![key("b")]
            );
            assert!(replace_once_in_transaction(
                &crate::desktop::test_catalog().unwrap(),
                tx,
                &scope(),
                "0",
                &[key("b")],
                "pin_request"
            )
            .is_err());
            Ok(())
        })
        .unwrap();
}
