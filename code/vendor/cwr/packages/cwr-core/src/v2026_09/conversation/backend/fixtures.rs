//! Pure test snapshot conversion through the declared storage port.
use super::*;
impl ConversationBackend {
    pub(in crate::v2026_09::conversation) fn commit_fixture_state(
        &mut self,
        state: &ConversationRuntimeState,
    ) -> CwrResult<i64> {
        let changes = self.changes(state)?;
        let records = changes
            .records
            .iter()
            .map(|record| Record {
                reference: record.reference.clone(),
                kind: record.kind,
                schema_ref: crate::cwpdb::schema_ref(record.kind.as_str()),
                properties: record.properties.clone(),
                revision: String::new(),
            })
            .collect();
        let mut relationships = Vec::new();
        for change in &changes.memberships {
            let (source, kind, members) = match change {
                MembershipChange::Upsert {
                    source_ref,
                    kind,
                    member,
                } => (source_ref, *kind, std::slice::from_ref(member)),
                MembershipChange::Replace { membership } => (
                    &membership.source_ref,
                    membership.kind,
                    membership.members.as_slice(),
                ),
                MembershipChange::Remove { .. } => continue,
            };
            for member in members {
                relationships.push(Relationship {
                    reference: crate::cwpdb::derived_ref(
                        kind.as_str(),
                        &[source, &member.target_ref],
                    ),
                    source_ref: source.clone(),
                    kind: kind.as_str().into(),
                    target_ref: member.target_ref.clone(),
                    position: member.position,
                    properties: member.properties.clone(),
                    revision: String::new(),
                });
            }
        }
        self.validate_snapshot(&SnapshotPage {
            revision: self.revision.clone(),
            metadata: json!({}),
            records,
            relationships,
            continuation: None,
            complete: true,
        })?;
        self.validate_changes(&changes)?;
        let result = self.commit_changes(changes)?;
        self.cache_initialized = false;
        Ok(result)
    }
}

#[test]
fn declared_pages_preserve_one_revision_or_refuse_drift_without_mixing_records() {
    struct Transport {
        replies: std::collections::VecDeque<SnapshotPage>,
    }
    impl cwpdb_store::remote::DeclaredTransport for Transport {
        fn call(&mut self, operation: Value) -> CwrResult<Value> {
            assert_eq!(operation["operation"], "read_conversation_graph");
            assert_eq!(operation["request"]["expected_revision"], "stable");
            Ok(serde_json::to_value(
                self.replies
                    .pop_front()
                    .ok_or("unexpected fixture operation")?,
            )?)
        }
    }
    for (revision, clock, valid) in [
        ("stable", 1, true),
        ("changed", 2, false),
        ("stable", 2, false),
    ] {
        let page = |revision: &str, clock: i64, complete: bool, marker: &str| SnapshotPage {
            revision: revision.into(),
            metadata: json!({"commit_revision":clock}),
            records: vec![Record {
                reference: format!("cwp://example.test/conversations/{marker}"),
                kind: RecordKind::Conversation,
                schema_ref: crate::cwpdb::schema_ref("conversation"),
                properties: json!({"title":marker}),
                revision: marker.into(),
            }],
            relationships: vec![],
            continuation: (!complete).then(|| "next".into()),
            complete,
        };
        let context = cwpdb_store::invocation::Context {
            workspace_ref: "cwp://example.test/runtime".into(),
            caller_ref: "cwp://example.test/runtime".into(),
        };
        let mut backend = ConversationBackend {
            storage: Box::new(cwpdb_store::remote::RemoteStore::new(Transport {
                replies: std::collections::VecDeque::from([
                    page("stable", 1, false, "first"),
                    page(revision, clock, true, "second"),
                ]),
            })),
            scope: Scope {
                runtime_root_ref: "cwp://example.test/runtime".into(),
                context: context.clone(),
            },
            authorizer: Box::new(ScopeAuthorizer(context)),
            revision: "stable".into(),
            metadata: json!({}),
            publications: BTreeMap::new(),
            cache_initialized: false,
            native: false,
        };
        let result = backend.pages(Selection::Records {
            kind: RecordKind::Conversation,
        });
        if valid {
            let snapshot = result.unwrap();
            assert_eq!(snapshot.revision, "stable");
            assert_eq!(snapshot.metadata["commit_revision"], 1);
            assert_eq!(
                snapshot
                    .records
                    .iter()
                    .map(|record| record.properties["title"].as_str().unwrap())
                    .collect::<Vec<_>>(),
                ["first", "second"]
            );
        } else {
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("changed between pages"));
        }
    }
}
