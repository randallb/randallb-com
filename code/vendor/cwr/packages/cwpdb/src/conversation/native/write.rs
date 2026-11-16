use super::*;
use rusqlite::{params, Transaction, TransactionBehavior};
pub(super) mod memberships;
mod publications;
pub(super) mod records;
impl ConversationStorage for NativeConversationStore {
    fn read_selection(
        &mut self,
        request: &ReadSelectionRequest,
        control: &ReadControl,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<ReadSelectionFacts> {
        self.check_scope(&request.scope)?;
        request.authorize(authorizer)?;
        let result = read_snapshot::owner_selection(&mut self.connection, request, control)?;
        self.check_scope(&request.scope)?;
        request.authorize(authorizer)?;
        request.authorize_result(&result, authorizer)?;
        control.check()?;
        Ok(result)
    }

    fn history_read(
        &mut self,
        request: &HistoryReadRequest,
        control: &ReadControl,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<Option<HistoryReadFacts>> {
        self.history_read_facts(request, control, authorizer)
    }

    fn timeline_page(
        &mut self,
        request: &TimelineReadRequest,
        control: &ReadControl,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<Option<TimelineReadFacts>> {
        self.timeline_page_facts(request, control, authorizer)
    }

    fn content_page(
        &mut self,
        request: &ContentPageRequest,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<Option<ContentPageFacts>> {
        self.content_page_facts(request, authorizer)
    }
    fn completion_page(
        &mut self,
        request: &CompletionPageRequest,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<Option<CompletionPageFacts>> {
        self.completion_page_facts(request, authorizer)
    }
    fn endpoint_kinds(
        &mut self,
        scope: &Scope,
        reference: &str,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<EndpointKinds> {
        use rusqlite::OptionalExtension;
        self.check_scope(scope)?;
        valid_ref(reference)?;
        authorizer.authorize(&scope.context, reference, false)?;
        let tx = self.connection.transaction()?;
        let node_kind = tx.query_row("SELECT kind FROM nodes WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)", [reference], |row| row.get(0)).optional()?;
        let edge_kind = tx.query_row("SELECT kind FROM edges WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)", [reference], |row| row.get(0)).optional()?;
        tx.commit()?;
        Ok(EndpointKinds {
            node_kind,
            edge_kind,
        })
    }
    fn resource(
        &mut self,
        scope: &Scope,
        reference: &str,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<Option<ResourceFacts>> {
        self.resource_facts(scope, reference, authorizer)
    }
    fn summary(
        &mut self,
        scope: &Scope,
        reference: &str,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<Option<SummaryFacts>> {
        self.summary_facts(scope, reference, authorizer)
    }

    fn identity_in_use(
        &mut self,
        scope: &Scope,
        reference: &str,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<bool> {
        self.check_scope(scope)?;
        valid_ref(reference)?;
        authorizer.authorize(&scope.context, reference, false)?;
        crate::record_exists(&self.connection, reference)
    }
    #[cfg(feature = "test-support")]
    fn audit_publication_writes(
        &mut self,
        action: PublicationAudit,
    ) -> CwpdbResult<PublicationWriteCounts> {
        self.publication_write_audit(action)
    }
    fn retained_order(
        &mut self,
        scope: &Scope,
        reference: &str,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<Option<i64>> {
        self.check_scope(scope)?;
        valid_ref(reference)?;
        authorizer.authorize(&scope.context, reference, false)?;
        crate::find(&self.connection, reference)
    }
    fn published_progress_conversation(
        &mut self,
        scope: &Scope,
        source_activity_ref: &str,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<Option<String>> {
        use rusqlite::OptionalExtension;
        self.check_scope(scope)?;
        valid_ref(source_activity_ref)?;
        let tx = self.connection.transaction()?;
        authorizer.authorize(&scope.context, source_activity_ref, false)?;
        let reference:Option<String>=tx.query_row("SELECT owner.canonical_ref FROM edges lookup INDEXED BY edges_in JOIN nodes message ON message.ref_id=lookup.source_id AND message.kind='message' JOIN edges membership INDEXED BY edges_in ON membership.target_id=message.ref_id AND membership.kind='message' JOIN ref_bindings owner ON owner.ref_id=membership.source_id WHERE lookup.target_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND lookup.kind='field:progress.source_activity_ref' ORDER BY membership.source_id,message.ref_id LIMIT 1",[source_activity_ref],|row|row.get(0)).optional()?;
        if let Some(reference) = &reference {
            authorizer.authorize(&scope.context, reference, false)?;
        }
        tx.commit()?;
        Ok(reference)
    }
    fn read(
        &mut self,
        request: &ReadRequest,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<SnapshotPage> {
        self.read_page(request, authorizer)
    }
    fn get(
        &mut self,
        scope: &Scope,
        reference: &str,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<Option<Record>> {
        self.check_scope(scope)?;
        valid_ref(reference)?;
        let tx = self.connection.transaction()?;
        authorizer.authorize(&scope.context, reference, false)?;
        let result = read::get(&tx, reference)?;
        if serde_json::to_vec(&result)?.len() > MAX_WIRE_BYTES {
            return Err("CWR record exceeds24MiB wire budget".into());
        }
        tx.commit()?;
        Ok(result)
    }
    fn history_status(
        &mut self,
        scope: &Scope,
        reference: &str,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<Option<HistoryStatus>> {
        self.history_status_facts(scope, reference, authorizer)
    }
    fn contains_member(
        &mut self,
        scope: &Scope,
        source: &str,
        kind: MembershipKind,
        target: &str,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<bool> {
        self.contains_member_facts(scope, source, kind, target, authorizer)
    }
    fn reconcile(
        &mut self,
        request: &Mutation,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<Option<CommitOutcome>> {
        self.check_scope(&request.scope)?;
        let tx = self.connection.transaction()?;
        authorizer.authorize(
            &request.scope.context,
            &request.scope.runtime_root_ref,
            true,
        )?;
        let result = receipts::reconcile(&tx, request)?;
        tx.commit()?;
        Ok(result)
    }
    fn commit(
        &mut self,
        request: &Mutation,
        authorizer: &dyn Authorizer,
    ) -> CwpdbResult<CommitOutcome> {
        if request.changes.records.len()
            + request.changes.remove_records.len()
            + request.changes.memberships.len()
            + request.changes.timeline_items.len()
            > 50_000
            || serde_json::to_vec(request)?.len() > MAX_WIRE_BYTES
        {
            return Err("CWR runtime mutation exceeds its work/byte budget".into());
        }
        self.check_scope(&request.scope)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        authorizer.authorize(
            &request.scope.context,
            &request.scope.runtime_root_ref,
            true,
        )?;
        if let Some(result) = receipts::reconcile(&tx, request)? {
            tx.commit()?;
            return Ok(result);
        }
        let (current, counter) = current_revision(&tx, &request.scope)?;
        if current != request.expected_revision {
            return Err("conversation revision_conflict".into());
        }
        for guard in &request.authority_guards {
            let protected = crate::ref_read::get_ref(&tx, &guard.reference)?;
            if protected.kind != "cloud_document_v1"
                || ref_revision(&guard.reference, protected.revision.legacy_counter())
                    != guard.expected_revision
                || crate::identity_claim(&tx, &guard.reference)?
                    .and_then(|claim| claim.owner_ref)
                    .as_deref()
                    != Some(request.scope.context.workspace_ref.as_str())
            {
                return Err(Box::new(SourceAuthorityConflict));
            }
        }
        let journal_receipt = if let Some(journal) = &request.journal {
            if journal.request.context.workspace_ref != request.scope.context.workspace_ref
                || journal.request.request_id == request.request_id
            {
                return Err("invalid nested execution event identity".into());
            }
            let scope = crate::derived_ref(
                "declared_mutation_scope",
                &[
                    &journal.request.context.workspace_ref,
                    &journal.request.context.caller_ref,
                ],
            );
            if !matches!(
                crate::sequence_receipt::check(
                    &tx,
                    &scope,
                    &journal.request.request_id,
                    &serde_json::to_value(&journal.request)?
                )?,
                crate::sequence_receipt::Replay::NotCommitted
            ) {
                return Err(
                    "execution event already committed; reconcile and reload CWR state".into(),
                );
            }
            match crate::invocation::mutate_in_transaction(
                &tx,
                &journal.request,
                authorizer,
                journal.now_unix_ms,
            )? {
                crate::invocation::Outcome::Committed(receipt) => Some(receipt),
                crate::invocation::Outcome::CommittedResultExpired => {
                    return Err("execution result expired".into())
                }
            }
        } else {
            None
        };
        let next = counter
            .checked_add(1)
            .ok_or("CWR commit revision exhausted")?;
        let root = root_ref(&request.scope);
        let mut changed = false;
        for record in &request.changes.records {
            changed |= records::put(&tx, &request.scope, record, next)?;
        }
        for record in &request.changes.remove_records {
            valid_ref(&record.reference)?;
            if record.kind == RecordKind::Runtime {
                return Err("runtime identity cannot be removed".into());
            }
            if let Some(existing) = read::get(&tx, &record.reference)? {
                if existing.kind != record.kind {
                    return Err("CWR record kind conflict".into());
                }
                crate::remove(&tx, &record.reference)?;
                changed = true;
            }
        }
        for change in &request.changes.memberships {
            changed |= memberships::apply(&tx, &request.scope, change)?;
        }
        for item in &request.changes.timeline_items {
            changed |= records::put(&tx, &request.scope, &item.record(), next)?;
        }
        for item in &request.changes.timeline_items {
            for membership in item.memberships() {
                changed |= memberships::apply(&tx, &request.scope, &membership)?;
            }
        }
        for publication in &request.changes.publications {
            changed |= publications::apply(&tx, publication, &request.scope, next)?;
        }
        if let Some(version) = &request.changes.state_version {
            if !["cwp://org.coworkerprotocol/cwpdb/schemas/runtime-state/2026-09","cwp://org.coworkerprotocol/cwpdb/schemas/runtime-state/2026-09/conversation-timeline-v1"].contains(&version.as_str()){return Err("unsupported CWR runtime state version".into());}
            changed|=tx.execute("UPDATE nodes SET props=jsonb_set(props,'$.runtime_state_version',?2) WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND json_extract(props,'$.runtime_state_version') IS NOT ?2",params![root,version])?>0;
        }
        if let Some(runs) = &request.changes.streaming_runs {
            if !runs.is_object() {
                return Err("invalid streaming run state".into());
            }
            changed|=tx.execute("UPDATE nodes SET props=jsonb_set(props,'$.streaming_runs',jsonb(?2)) WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND COALESCE(json_extract(props,'$.streaming_runs'),json('{}')) IS NOT json(?2)",params![root,serde_json::to_string(runs)?])?>0;
        }
        if let Some(stage) = &request.changes.timeline_rollout {
            changed |= initialization::configure_rollout(&tx, &request.scope, stage)?;
        }
        if let Some(policy) = &request.changes.context_policy {
            if !policy.is_object() {
                return Err("invalid retained context policy wrapper".into());
            }
            changed |= tx.execute("UPDATE nodes SET props=jsonb_set(props,'$.context_policy',jsonb(?2)) WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1) AND json_extract(props,'$.context_policy') IS NOT json(?2)",params![root,serde_json::to_string(policy)?])?>0;
        }
        if serde_json::to_vec(&runtime_metadata(&tx, &request.scope)?)?.len() > MAX_RECORD_BYTES {
            return Err("CWR metadata exceeds24MiB wire budget".into());
        }
        if request.changes.repair_legacy {
            changed |= maintenance::repair(&tx, request, next)?;
        }
        if changed {
            tx.execute("UPDATE nodes SET props=jsonb_set(props,'$.commit_revision',?2) WHERE ref_id=(SELECT ref_id FROM ref_bindings WHERE canonical_ref=?1)",params![root,next])?;
        }
        let result = CommitOutcome::Committed {
            revision: if changed {
                revision(&request.scope, next)
            } else {
                current
            },
            commit_revision: if changed { next } else { counter },
            journal_receipt,
        };
        receipts::save(&tx, request, &result)?;
        tx.commit()?;
        Ok(result)
    }
}
