use std::collections::BTreeSet;

// Archive and cancellation share the transaction, including terminal history.
pub(super) fn stage_archive_stops(
    state: &mut ConversationRuntimeState,
    archive: &ConversationArchiveMutation,
) -> CwrResult<()> {
    let targets = archive_conversation_roots(state, &archive.conversation_ref);
    stage_archive_stop_targets(state, archive, targets)
}

impl ConversationAdmissionStore {
    pub(crate) fn archive_stop_targets(&self, conversation: &str) -> Vec<String> {
        let roots = archive_conversation_roots(&self.state, conversation);
        archive_live_stop_targets(&self.state, roots)
    }

    pub(crate) fn settle_archive_cancellation(
        &mut self,
        archive: &ConversationArchiveMutation,
        targets: Vec<String>,
    ) -> CwrResult<()> {
        self.transact(|state| stage_archive_stop_targets(state, archive, targets))
    }
}

fn archive_conversation_roots(state: &ConversationRuntimeState, conversation: &str) -> Vec<String> {
    // Own-scope work retains its requesting conversation on the invocation,
    // while its execution conversation is recorded separately. Both own work.
    let owners = state.invocation_execution_conversation_refs.iter()
        .filter(|(_, execution)| execution.as_str() == conversation)
        .map(|(reference, _)| reference.clone())
        .chain(state.admissions.iter()
            .filter(|record| record.execution_conversation_ref.as_deref() == Some(conversation))
            .filter_map(|record| record.invocation_ref.clone()))
        .collect::<BTreeSet<_>>();
    state.invocation_ledger.invocations()
        // Independent controls retain command order after the archive settles.
        // Delegated controls still belong to the work that is being stopped.
        .filter(|record| !(record.parent_invocation_ref.is_none()
            && record.target_actor_ref == crate::action_contract::INTRINSIC_CWR_ACTOR_REF
            ))
        .filter(|record| record.conversation_ref.as_deref() == Some(conversation)
            || owners.contains(&record.invocation_ref))
        .map(|record| record.invocation_ref.clone()).collect()
}

fn stage_archive_stop_targets(
    state: &mut ConversationRuntimeState,
    archive: &ConversationArchiveMutation,
    targets: Vec<String>,
) -> CwrResult<()> {
    let targets = archive_live_stop_targets(state, targets);
    let occurred_at = current_unix_ms();
    for target in targets {
        // An earlier selected ancestor may already have stopped this record.
        if state.invocation_ledger.invocation(&target).is_none_or(|record| record.state.is_terminal()) {
            continue;
        }
        let request = InvocationStopRequest {
            requested_at_unix_ms: occurred_at,
            requested_by_actor_ref: Some(archive.caller_actor_ref.clone()),
            request_id: Some(format!("archive-{}", Uuid::new_v4().simple())),
            reason: Some("Conversation archived".into()),
        };
        if let StopInvocationOutcome::Accepted {
            stopped_invocation_refs,
            ..
        } = state.invocation_ledger.stop(&target, request)?
        {
            for reference in stopped_invocation_refs {
                let response = retained_terminal_response(state, &reference)?;
                let record = state.invocation_ledger.invocation(&reference).unwrap();
                if matches!(record.origin, InvocationOrigin::Ingress { .. }) {
                    let reply_kind = root_non_success_reply_kind(state, &reference)?;
                    let activity = terminal_activity(&response)?;
                    stage_root_projection(
                        state,
                        &reference,
                        &response,
                        response.summary.clone(),
                        reply_kind,
                        Some((activity, occurred_at)),
                    )?;
                } else {
                    stage_child_projection(
                        state,
                        &reference,
                        &response,
                        response.summary.clone(),
                        occurred_at,
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn archive_live_stop_targets(state: &ConversationRuntimeState, targets: Vec<String>) -> Vec<String> {
    let roots = targets.into_iter().collect::<BTreeSet<_>>();
    // Terminal ancestors still connect outstanding descendants, but terminal
    // leaves require no stop traversal. Visit only live records as candidates.
    state.invocation_ledger.invocations()
        .filter(|record| !record.state.is_terminal() || record.state == ActionInvocationState::Stopping)
        .filter_map(|record| {
            let mut current = Some(record.invocation_ref.as_str());
            let mut seen = BTreeSet::new();
            while let Some(reference) = current {
                if roots.contains(reference) { return Some(record.invocation_ref.clone()); }
                if !seen.insert(reference) { break; }
                current = state.invocation_ledger.invocation(reference)
                    .and_then(|ancestor| ancestor.parent_invocation_ref.as_deref());
            }
            None
        }).collect::<Vec<_>>()
}
