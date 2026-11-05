use super::*;
use rusqlite::Transaction;
pub(super) fn scope(request: &Mutation) -> String {
    crate::derived_ref(
        "declared_mutation_scope",
        &[
            &request.scope.context.workspace_ref,
            &request.scope.context.caller_ref,
        ],
    )
}
pub(super) fn reconcile(db: &Connection, request: &Mutation) -> CwpdbResult<Option<CommitOutcome>> {
    match crate::sequence_receipt::check(
        db,
        &scope(request),
        &request.request_id,
        &serde_json::to_value(request)?,
    )? {
        crate::sequence_receipt::Replay::NotCommitted => Ok(None),
        crate::sequence_receipt::Replay::CommittedResultExpired => {
            Ok(Some(CommitOutcome::CommittedResultExpired))
        }
        crate::sequence_receipt::Replay::Retained(value) => {
            Ok(Some(serde_json::from_value(value)?))
        }
    }
}
pub(super) fn save(
    tx: &Transaction<'_>,
    request: &Mutation,
    result: &CommitOutcome,
) -> CwpdbResult<()> {
    let scope = scope(request);
    crate::claim_identity(
        tx,
        &scope,
        "mutation_scope",
        &request.scope.context.workspace_ref,
    )?;
    crate::sequence_receipt::commit(
        tx,
        &scope,
        &request.request_id,
        &serde_json::to_value(request)?,
        &serde_json::to_value(result)?,
        &[],
    )?;
    let payload = crate::sequence_receipt::tip(&scope, &request.request_id);
    let mut dependencies = request
        .changes
        .records
        .iter()
        .map(|record| record.reference.as_str())
        .chain(
            request
                .changes
                .publications
                .iter()
                .map(|publication| publication.conversation_ref.as_str()),
        )
        .collect::<std::collections::BTreeSet<_>>();
    for change in &request.changes.memberships {
        match change {
            MembershipChange::Upsert {
                source_ref, member, ..
            } => {
                dependencies.insert(source_ref.as_str());
                dependencies.insert(member.target_ref.as_str());
            }
            MembershipChange::Remove {
                source_ref,
                target_ref,
                ..
            } => {
                dependencies.insert(source_ref.as_str());
                dependencies.insert(target_ref.as_str());
            }
            MembershipChange::Replace { membership } => {
                dependencies.insert(membership.source_ref.as_str());
                dependencies.extend(
                    membership
                        .members
                        .iter()
                        .map(|member| member.target_ref.as_str()),
                );
            }
        }
    }
    for item in &request.changes.timeline_items {
        dependencies.extend([
            item.item_ref.as_str(),
            item.conversation_ref.as_str(),
            item.connection_ref.as_str(),
            item.publication_ref.as_str(),
        ]);
        dependencies.insert(match &item.content {
            TimelineContent::Message { message_ref } => message_ref,
            TimelineContent::Invocation { invocation_ref } => invocation_ref,
        });
        if let Some(child) = &item.child_connection_ref {
            dependencies.insert(child.as_str());
        }
    }
    for reference in dependencies {
        crate::put_disposable_edge(
            tx,
            reference,
            "cwpdb:runtime_receipt_dependency_v1",
            &payload,
            0,
            &json!({}),
        )?;
    }
    Ok(())
}
