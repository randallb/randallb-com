//! Fixed legacy identity-owner fixtures and finite claim observations.
use crate::{CwpdbResult, IdentityClaim};
use rusqlite::{Connection, OpenFlags};
use std::path::Path;

#[derive(Clone, Copy)]
pub enum ClaimRole {
    Conversation,
    TimelineConnection,
    TimelineItem,
    Publication,
}
impl ClaimRole {
    fn kind(self) -> &'static str {
        match self {
            Self::Conversation => "node:conversation",
            Self::TimelineConnection => "node:conversation_timeline_connection",
            Self::TimelineItem => "node:conversation_timeline_item",
            Self::Publication => "conversation_timeline_publication",
        }
    }
}
pub struct TimelineIdentityRefs<'a> {
    pub conversation: &'a str,
    pub connection: &'a str,
    pub item: &'a str,
    pub publication: &'a str,
}
fn open(path: &Path, runtime: &str, writable: bool) -> CwpdbResult<Connection> {
    let flags = if writable {
        OpenFlags::SQLITE_OPEN_READ_WRITE
    } else {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    };
    let db = Connection::open_with_flags(path, flags)?;
    let identity =
        crate::node_props(&db, &format!("{runtime}/cwpdb"))?.ok_or("fixture runtime missing")?;
    if identity["runtime_root_ref"] != runtime {
        return Err("fixture runtime identity mismatch".into());
    }
    Ok(db)
}
fn owned(db: &Connection, runtime: &str, conversation: &str) -> CwpdbResult<()> {
    let valid:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM edges e JOIN nodes n ON n.ref_id=e.target_id AND n.kind='conversation' WHERE e.source_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND e.kind='conversation' AND e.target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?2))",rusqlite::params![format!("{runtime}/cwpdb"),conversation],|r|r.get(0))?;
    if !valid {
        return Err("fixture conversation is not owned by runtime".into());
    }
    Ok(())
}
fn corrupt(
    path: &Path,
    runtime: &str,
    conversation: &str,
    references: &[(&str, ClaimRole)],
    new_owner: Option<&str>,
) -> CwpdbResult<()> {
    let mut db = open(path, runtime, true)?;
    let tx = db.transaction()?;
    owned(&tx, runtime, conversation)?;
    for (reference, role) in references {
        let actual =
            crate::identity_claim(&tx, reference)?.ok_or("fixture identity claim missing")?;
        if actual.kind != role.kind() || actual.owner_ref.as_deref() != Some(conversation) {
            return Err("fixture requires current owned identity claims".into());
        }
    }
    let trigger:String=tx.query_row("SELECT sql FROM sqlite_master WHERE type='trigger' AND name='ref_bindings_no_claim_update'",[],|r|r.get(0))?;
    tx.execute_batch("DROP TRIGGER ref_bindings_no_claim_update;")?;
    for (reference, _) in references {
        if tx.execute(
            "UPDATE ref_bindings SET claim_owner_ref=?2 WHERE canonical_ref=?1",
            rusqlite::params![reference, new_owner],
        )? != 1
        {
            return Err("fixture identity binding missing".into());
        }
    }
    tx.execute_batch(&trigger)?;
    tx.commit()?;
    Ok(())
}
/// Model an old store whose four known timeline claims omitted owner fields.
/// The exact protected trigger is restored in the same transaction.
pub fn ownerless_timeline(
    path: &Path,
    runtime: &str,
    refs: &TimelineIdentityRefs<'_>,
) -> CwpdbResult<()> {
    corrupt(
        path,
        runtime,
        refs.conversation,
        &[
            (refs.conversation, ClaimRole::Conversation),
            (refs.connection, ClaimRole::TimelineConnection),
            (refs.item, ClaimRole::TimelineItem),
            (refs.publication, ClaimRole::Publication),
        ],
        None,
    )
}
/// Model a single legacy publication-owner omission without touching siblings.
pub fn ownerless_publication(
    path: &Path,
    runtime: &str,
    conversation: &str,
    publication: &str,
) -> CwpdbResult<()> {
    corrupt(
        path,
        runtime,
        conversation,
        &[(publication, ClaimRole::Publication)],
        None,
    )
}
/// The one fixed foreign owner used to verify reopen rejects rebinding.
pub fn conflicting_conversation_owner(
    path: &Path,
    runtime: &str,
    conversation: &str,
) -> CwpdbResult<()> {
    corrupt(
        path,
        runtime,
        conversation,
        &[(conversation, ClaimRole::Conversation)],
        Some("cwp://example.test/conversations/other"),
    )
}
/// Observe the existing cwpdb-owned retained identity probe without repair.
pub fn needs_repair(path: &Path, runtime: &str) -> CwpdbResult<bool> {
    let db = open(path, runtime, false)?;
    Ok(db.query_row(
        include_str!("../conversation/native/identity_claim_probe.sql"),
        [],
        |r| r.get(0),
    )?)
}
/// Read one known role's claim without exposing its physical ID or connection.
pub fn observe(
    path: &Path,
    runtime: &str,
    conversation: &str,
    role: ClaimRole,
    reference: &str,
) -> CwpdbResult<Option<IdentityClaim>> {
    let db = open(path, runtime, false)?;
    owned(&db, runtime, conversation)?;
    let claim = crate::identity_claim(&db, reference)?;
    if claim.as_ref().is_some_and(|claim| {
        claim.kind != role.kind()
            || claim
                .owner_ref
                .as_deref()
                .is_some_and(|owner| owner != conversation)
    }) {
        return Err("fixture claim role or owner mismatch".into());
    }
    Ok(claim)
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn fixed_claim_faults_validate_all_inputs_restore_trigger_and_never_allow_rebind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("claims.sqlite");
        let runtime = "cwp://fixture/runtime";
        let conversation = "cwp://fixture/conversation";
        let refs = TimelineIdentityRefs {
            conversation,
            connection: "cwp://fixture/connection",
            item: "cwp://fixture/item",
            publication: "cwp://fixture/publication",
        };
        let mut store = crate::NativeStore::open(&path).unwrap();
        store
            .graph_transaction(|tx| {
                crate::put_node(
                    tx,
                    &format!("{runtime}/cwpdb"),
                    "runtime",
                    &json!({"runtime_root_ref":runtime,"commit_revision":4}),
                )?;
                crate::put_node(tx, conversation, "conversation", &json!({}))?;
                crate::put_edge(
                    tx,
                    &format!("{runtime}/cwpdb"),
                    "conversation",
                    conversation,
                    0,
                    &json!({}),
                )?;
                for (reference, role) in [
                    (refs.conversation, ClaimRole::Conversation),
                    (refs.connection, ClaimRole::TimelineConnection),
                    (refs.item, ClaimRole::TimelineItem),
                    (refs.publication, ClaimRole::Publication),
                ] {
                    crate::claim_identity(tx, reference, role.kind(), conversation)?;
                }
                Ok(())
            })
            .unwrap();
        let invalid = TimelineIdentityRefs {
            publication: "cwp://fixture/missing",
            ..refs
        };
        assert!(ownerless_timeline(&path, runtime, &invalid).is_err());
        assert!(ownerless_timeline(&path, "cwp://fixture/foreign", &refs).is_err());
        assert_eq!(
            observe(
                &path,
                runtime,
                conversation,
                ClaimRole::Conversation,
                conversation
            )
            .unwrap()
            .unwrap()
            .owner_ref
            .as_deref(),
            Some(conversation)
        );
        // These three physical no-rebind assertions moved from the CWR fixture.
        store
            .graph_transaction(|tx| {
                for (reference, role) in [
                    (refs.connection, ClaimRole::TimelineConnection),
                    (refs.item, ClaimRole::TimelineItem),
                    (refs.publication, ClaimRole::Publication),
                ] {
                    assert!(crate::claim_identity(
                        tx,
                        reference,
                        role.kind(),
                        "cwp://example.test/conversations/other"
                    )
                    .is_err());
                    assert_eq!(
                        crate::identity_claim(tx, reference)?
                            .unwrap()
                            .owner_ref
                            .as_deref(),
                        Some(conversation)
                    );
                }
                Ok(())
            })
            .unwrap();
        ownerless_publication(&path, runtime, conversation, refs.publication).unwrap();
        assert_eq!(
            observe(
                &path,
                runtime,
                conversation,
                ClaimRole::Publication,
                refs.publication
            )
            .unwrap()
            .unwrap()
            .owner_ref,
            None
        );
        store.graph_read(|db|{
            assert_eq!(crate::node_props(db,&format!("{runtime}/cwpdb"))?.unwrap()["commit_revision"],4);
            assert!(db.execute("UPDATE ref_bindings SET claim_owner_ref='cwp://fixture/forbidden' WHERE canonical_ref=?1",[refs.conversation]).is_err());Ok(())
        }).unwrap();
    }
}
