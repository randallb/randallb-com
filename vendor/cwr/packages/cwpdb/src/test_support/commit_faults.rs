//! One fixed commit-stage failure for downstream atomicity tests.
use crate::CwpdbResult;
use rusqlite::{Connection, OpenFlags};
use std::path::Path;
/// Reject only advancement of a retained runtime's physical commit clock.
/// Ordinary record/edge work is staged first and must roll back with the clock.
/// Current-state reads and already-retained replay need no clock write.
pub fn reject_runtime_clock_updates(path: &Path, reject: bool) -> CwpdbResult<()> {
    let mut db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    let tx = db.transaction()?;
    if reject {
        tx.execute_batch("CREATE TRIGGER cwr_fixture_reject_runtime_clock BEFORE UPDATE ON nodes WHEN OLD.kind='runtime' AND json_extract(NEW.props,'$.commit_revision') IS NOT json_extract(OLD.props,'$.commit_revision') BEGIN SELECT RAISE(ABORT,'fixture runtime commit failure'); END;")?;
    } else {
        tx.execute_batch("DROP TRIGGER cwr_fixture_reject_runtime_clock;")?;
    }
    tx.commit()?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        conversation::*,
        invocation::{Authorizer, Context},
    };
    use serde_json::json;
    struct Allow;
    impl Authorizer for Allow {
        fn authorize(&self, _: &Context, _: &str, _: bool) -> CwpdbResult<()> {
            Ok(())
        }
    }
    #[test]
    fn late_clock_failure_rolls_back_records_clock_and_receipt_then_exact_retry_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("commit.sqlite");
        let runtime = "cwp://fixture/runtime";
        let scope = Scope {
            runtime_root_ref: runtime.into(),
            context: Context {
                workspace_ref: runtime.into(),
                caller_ref: runtime.into(),
            },
        };
        let options = OpenOptions {
            runtime_state_version: "cwp://org.coworkerprotocol/cwpdb/schemas/runtime-state/2026-09"
                .into(),
            workspace_binding: None,
            timeline_rollout: None,
        };
        let mut native = NativeConversationStore::open(&path, scope.clone(), options).unwrap();
        let read = ReadRequest {
            scope: scope.clone(),
            selection: Selection::Bootstrap,
            continuation: None,
            page_size: 1000,
            max_bytes: MAX_WIRE_BYTES,
            expected_revision: None,
        };
        let before = native.read(&read, &Allow).unwrap();
        let conversation = "cwp://fixture/conversation";
        let mutation = Mutation {
            scope: scope.clone(),
            request_id: "clock-fault-retry".into(),
            expected_revision: before.revision.clone(),
            changes: Changes {
                records: vec![WriteRecord {
                    reference: conversation.into(),
                    kind: RecordKind::Conversation,
                    properties: json!({}),
                    owner_ref: None,
                }],
                memberships: vec![MembershipChange::Upsert {
                    source_ref: format!("{runtime}/cwpdb"),
                    kind: MembershipKind::Conversation,
                    member: Member {
                        target_ref: conversation.into(),
                        position: 0,
                        properties: json!({}),
                    },
                }],
                ..Changes::default()
            },
            journal: None,
            authority_guards: vec![],
        };
        reject_runtime_clock_updates(&path, true).unwrap();
        let error = native.commit(&mutation, &Allow).unwrap_err();
        assert!(
            error.to_string().contains("fixture runtime commit failure"),
            "{error}"
        );
        assert!(native.get(&scope, conversation, &Allow).unwrap().is_none());
        let failed = native.read(&read, &Allow).unwrap();
        assert_eq!(failed.revision, before.revision);
        assert_eq!(failed.metadata, before.metadata);
        assert!(native.reconcile(&mutation, &Allow).unwrap().is_none());
        reject_runtime_clock_updates(&path, false).unwrap();
        native.commit(&mutation, &Allow).unwrap();
        assert!(native.get(&scope, conversation, &Allow).unwrap().is_some());
        let committed = native.read(&read, &Allow).unwrap();
        assert_ne!(committed.revision, before.revision);
        native.commit(&mutation, &Allow).unwrap();
        assert_eq!(
            native.read(&read, &Allow).unwrap().revision,
            committed.revision
        );
    }
}
