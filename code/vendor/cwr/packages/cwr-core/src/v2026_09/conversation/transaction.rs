impl ConversationAdmissionStore {
    fn transact<T>(
        &mut self,
        mutate: impl FnOnce(&mut ConversationRuntimeState) -> CwrResult<T>,
    ) -> CwrResult<T> {
        self.hydrate_all_cold_conversations()?;
        let mut backend = self.backend.lock().map_err(|_| invalid_error("conversation backend lock poisoned"))?;
        backend.set_expected_revision(self.commit_revision);
        backend.prepare(&self.state)?;
        drop(backend);
        self.state.begin();
        let transaction = StateMutation(&mut self.state);
        let result = mutate(transaction.0)?;
        if let Some(graph) = &self.response_graph {
            stage_person_response_routes(transaction.0, graph)?;
            availability::validate_new_context_work(transaction.0, graph, self.context_policy.as_ref(), self.context_policy_configured)?;
        }
        streaming::withdraw_inactive(transaction.0)?;
        if let Some(rollout) = self.timeline_rollout {
            timeline::stage_timeline_rollout_changes(transaction.0, rollout)?;
        }
        transaction.0.invocation_ledger.validate_changes()?;
        invocation::validate_changed_execution_links(transaction.0)?;
        timeline::validate_changed_timelines(transaction.0)?;
        if transaction.0.has_changes() {
            self.commit_revision = self.backend.lock().map_err(|_| invalid_error("conversation backend lock poisoned"))?.commit(transaction.0, None, self.pending_authority_guards.clone())?;
        }
        // Observe only successfully persisted changes, before discarding the journal.
        // This never blocks execution on the metrics writer or scans retained history.
        if let Some(metrics) = &self.metrics {
            for (_, old, new) in transaction.0.invocation_ledger.changes() {
                if let Some(record) = new.filter(|new| old != Some(*new)) {
                    let source =
                        transaction
                            .0
                            .admissions
                            .changes()
                            .find_map(|(_, _, admission)| {
                                (admission.invocation_ref.as_deref()
                                    == Some(&record.invocation_ref))
                                .then_some(admission.source_message_ref.as_str())
                            });
                    metrics.observe_invocation(
                        &transaction.0.runtime_root_ref,
                        record,
                        source,
                        false,
                    );
                }
            }
        }
        let admissions_changed = transaction.0.admissions.changes().next().is_some();
        transaction.0.commit();
        drop(transaction);
        // Artifact HTML is durable in SQLite after commit and should not grow
        // the resident runtime heap. Explicit reads fetch it from the database.
        self.state.visualization_artifacts = Default::default();
        self.state.files = Default::default();
        if admissions_changed {
            self.rebuild_admission_indices();
        }
        Ok(result)
    }
}

/// Roll back on any validation, serialization, or SQLite error, and on unwind.
/// SQLite commits before these undo records are discarded.
struct StateMutation<'a>(&'a mut ConversationRuntimeState);
impl Drop for StateMutation<'_> {
    fn drop(&mut self) {
        self.0.conversations.rollback();
        self.0.visualization_artifacts.rollback();
        self.0.files.rollback();
        self.0.admissions.rollback();
        self.0.invocation_ledger.rollback();
        self.0.timeline_store.rollback();
        self.0.streaming_messages.rollback();
        self.0.streaming_runs.rollback();
        self.0.tool_attempts.rollback();
        self.0.tool_operations.rollback();
        self.0.invocation_execution_conversation_refs.rollback();
        self.0.upstream_participant_versions.rollback();
        self.0.protected_result_children.rollback();
    }
}

impl ConversationRuntimeState {
    #[cfg(test)]
    fn fixture_diff(&self, replacement: &Self) -> CwrResult<Self> {
        if self.runtime_root_ref != replacement.runtime_root_ref || self.version != replacement.version {
            return invalid("fixture replacement cannot rebind runtime identity or state version");
        }
        let mut staged=self.clone();
        staged.begin();
        staged.conversations.fixture_replace(&replacement.conversations)?;
        staged.admissions.fixture_replace(&replacement.admissions)?;
        staged.invocation_ledger.fixture_replace(&replacement.invocation_ledger)?;
        staged.timeline_store.fixture_replace(&replacement.timeline_store);
        staged.files.fixture_replace(&replacement.files);
        staged.visualization_artifacts.fixture_replace(&replacement.visualization_artifacts);
        staged.streaming_messages.fixture_replace(&replacement.streaming_messages);
        staged.streaming_runs.fixture_replace(&replacement.streaming_runs);
        staged.tool_attempts.fixture_replace(&replacement.tool_attempts);
        staged.tool_operations.fixture_replace(&replacement.tool_operations);
        staged.invocation_execution_conversation_refs.fixture_replace(&replacement.invocation_execution_conversation_refs);
        staged.upstream_participant_versions.fixture_replace(&replacement.upstream_participant_versions);
        staged.protected_result_children.fixture_replace(&replacement.protected_result_children);
        Ok(staged)
    }

    fn has_changes(&self) -> bool {
        self.protected_result_children.changes().any(|(_,old,new)|old!=new)
            || self.files.changes().any(|(_, old, new)| old != new)
            || self.tool_attempts.changes().any(|(_, old, new)| old != new)
            || self
                .tool_operations
                .changes()
                .any(|(_, old, new)| old != new)
            || self
                .streaming_messages
                .changes()
                .any(|(_, old, new)| old != new)
            || self
                .streaming_runs
                .changes()
                .any(|(_, old, new)| old != new)
            || self
                .conversations
                .changes()
                .any(|(_, old, new)| old != Some(new))
            || self
                .visualization_artifacts
                .changes()
                .any(|(_, old, new)| old != new)
            || self
                .admissions
                .changes()
                .any(|(_, old, new)| old != Some(new))
            || self
                .invocation_ledger
                .changes()
                .any(|(_, old, new)| old != new)
            || self
                .invocation_ledger
                .changed_waits()
                .any(|(_, old, new)| old != Some(new))
            || self
                .timeline_store
                .changes()
                .any(|(_, old, new)| old != new)
            || self
                .invocation_execution_conversation_refs
                .changes()
                .any(|(_, old, new)| old != new)
            || self
                .upstream_participant_versions
                .changes()
                .any(|(_, old, new)| old != new)
    }
    fn begin(&mut self) {
        self.conversations.begin();
        self.visualization_artifacts.begin();
        self.files.begin();
        self.admissions.begin();
        self.invocation_ledger.begin();
        self.timeline_store.begin();
        self.streaming_messages.begin();
        self.streaming_runs.begin();
        self.tool_attempts.begin();
        self.tool_operations.begin();
        self.invocation_execution_conversation_refs.begin();
        self.upstream_participant_versions.begin();
        self.protected_result_children.begin();
    }
    fn commit(&mut self) {
        self.conversations.commit();
        self.visualization_artifacts.commit();
        self.files.commit();
        self.admissions.commit();
        self.invocation_ledger.commit();
        self.timeline_store.commit();
        self.streaming_messages.commit();
        self.streaming_runs.commit();
        self.tool_attempts.commit();
        self.tool_operations.commit();
        self.invocation_execution_conversation_refs.commit();
        self.upstream_participant_versions.commit();
        self.protected_result_children.commit();
    }
}

impl ConversationAdmissionStore {
    pub(crate) fn with_publication_authority<T>(&mut self, guards:Vec<cwpdb_store::conversation::AuthorityGuard>, publish:impl FnOnce(&mut Self)->CwrResult<T>)->CwrResult<T> {
        if !self.pending_authority_guards.is_empty() {return invalid("nested protected publication is invalid");}
        self.pending_authority_guards=guards;
        let result=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||publish(self)));
        self.pending_authority_guards.clear();
        match result {Ok(result)=>result,Err(unwind)=>std::panic::resume_unwind(unwind)}
    }
    pub(crate) fn retain_protected_result(&mut self,root:&str,child:&str)->CwrResult<()> {
        self.transact(|state| {let mut children=state.protected_result_children.get(root).cloned().unwrap_or_default();children.insert(child.into());state.protected_result_children.insert(root.into(),children);Ok(())})
    }
    pub(crate) fn protected_results(&self,root:&str)->Vec<String> {
        self.state.protected_result_children.get(root).map(|children|children.iter().cloned().collect()).unwrap_or_default()
    }
}

impl ConversationAdmissionStore {
    pub(crate) fn stored_context_policy(&self)->CwrResult<Option<(bool,Option<crate::v2026_09::upstream::ConversationContextPolicy>)>> {
        let backend=self.backend.lock().map_err(|_|invalid_error("conversation backend lock poisoned"))?;
        let Some(value)=backend.metadata.get("context_policy") else {return Ok(None)};
        let configured=value["configured"].as_bool().ok_or("invalid retained context policy configuration")?;
        let policy=value.get("policy").filter(|value|!value.is_null()).map(|value|serde_json::from_value(value.clone())).transpose()?;
        Ok(Some((configured,policy)))
    }
    pub(crate) fn persist_context_policy(&mut self,policy:&Option<crate::v2026_09::upstream::ConversationContextPolicy>)->CwrResult<()> {
        let value=json!({"configured":true,"policy":policy});
        let mut backend=self.backend.lock().map_err(|_|invalid_error("conversation backend lock poisoned"))?;
        backend.set_expected_revision(self.commit_revision);
        if backend.metadata.get("context_policy")!=Some(&value) {
            self.commit_revision=backend.commit_changes(cwpdb_store::conversation::Changes{context_policy:Some(value.clone()),..Default::default()})?;
            backend.metadata["context_policy"]=value;
        }
        self.context_policy=policy.clone();self.context_policy_configured=true;
        Ok(())
    }
}
