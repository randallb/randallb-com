//! Pure protocol-to-storage conversion. No physical graph or SQL operations.
use super::*;
mod changes;
mod history_read;
mod open;
#[cfg(test)]
mod pages;
pub(crate) use history_read::CanonicalHistoryReader;
pub(super) mod projection;
mod recovery;
pub(super) mod resource;
mod snapshot;
#[cfg(test)]
mod summary;
mod timeline_read;
mod validation;
use changes::{activity_kind, changes, projected_publications};
use cwpdb_store::conversation::*;
use cwpdb_store::invocation::Authorizer;

type MetadataSources = BTreeMap<String, (Option<ConversationMessage>, Vec<ConversationActivity>)>;
pub(super) struct ConversationBackend {
    pub storage: Box<dyn ConversationStorage>,
    pub scope: Scope,
    pub authorizer: Box<dyn Authorizer + Send>,
    pub revision: String,
    pub metadata: Value,
    publications: BTreeMap<String, PublicationSuffix>,
    cache_initialized: bool,
    native: bool,
}
impl ConversationBackend {
    pub fn mark_native(&mut self) {
        self.native = true;
    }
    pub fn is_native(&self) -> bool {
        self.native
    }
    pub fn declared(
        storage: Box<dyn ConversationStorage>,
        scope: Scope,
        authorizer: Box<dyn Authorizer + Send>,
    ) -> CwrResult<(Self, ConversationRuntimeState, i64)> {
        let mut backend = Self {
            storage,
            scope,
            authorizer,
            revision: String::new(),
            metadata: json!({}),
            publications: BTreeMap::new(),
            cache_initialized: false,
            native: false,
        };
        let page = backend.pages(Selection::Bootstrap)?;
        let mut state = snapshot::decode_state(&page, &backend.scope.runtime_root_ref)?;
        backend.revision = page.revision;
        backend.metadata = page.metadata;
        let revision = backend.metadata["commit_revision"]
            .as_i64()
            .ok_or("missing retained commit revision")?;
        // Only ordinary unresolved work needs eager history. Closed histories
        // retain headers and load from canonical pages on authorized access.
        let mut critical = state
            .invocation_ledger
            .invocations()
            .filter(|record| !record.state.is_terminal() || record.attempt_active)
            .map(|record| record.invocation_ref.clone())
            .collect::<std::collections::BTreeSet<_>>();
        let mut pending = critical.iter().cloned().collect::<Vec<_>>();
        while let Some(reference) = pending.pop() {
            if let Some(parent) = state
                .invocation_ledger
                .invocation(&reference)
                .and_then(|record| record.parent_invocation_ref.as_ref())
            {
                if critical.insert(parent.clone()) {
                    pending.push(parent.clone());
                }
            }
        }
        let mut conversations = std::collections::BTreeSet::new();
        for reference in &critical {
            if let Some(root) = state
                .invocation_ledger
                .invocation(reference)
                .and_then(|record| record.conversation_ref.as_ref())
            {
                conversations.insert(root.clone());
            }
            if let Some(execution) = state.invocation_execution_conversation_refs.get(reference) {
                conversations.insert(execution.clone());
            }
        }
        for conversation in state.conversations.iter() {
            if conversation
                .parent_conversation_ref
                .as_ref()
                .is_some_and(|root| conversations.contains(root))
            {
                conversations.insert(conversation.conversation_ref.clone());
            }
        }
        let mut hydrated = Vec::new();
        for conversation in state.conversations.iter() {
            if conversations.contains(&conversation.conversation_ref) {
                hydrated.push(backend.conversation(&conversation.conversation_ref)?);
                state
                    .cold_conversation_refs
                    .remove(&conversation.conversation_ref);
            } else {
                hydrated.push(conversation.clone());
            }
        }
        state.conversations = hydrated.into();
        Ok((backend, state, revision))
    }
    pub fn identity_in_use(&mut self, reference: &str) -> CwrResult<bool> {
        self.storage
            .identity_in_use(&self.scope, reference, self.authorizer.as_ref())
    }
    pub fn retained_order(&mut self, reference: &str) -> CwrResult<Option<i64>> {
        self.storage
            .retained_order(&self.scope, reference, self.authorizer.as_ref())
    }
    pub fn progress_conversation(&mut self, reference: &str) -> CwrResult<Option<String>> {
        self.storage.published_progress_conversation(
            &self.scope,
            reference,
            self.authorizer.as_ref(),
        )
    }
    pub fn get(&mut self, reference: &str) -> CwrResult<Option<Record>> {
        let record = self
            .storage
            .get(&self.scope, reference, self.authorizer.as_ref())?;
        if record.as_ref().is_some_and(|record| {
            record.reference != reference
                || record.schema_ref != crate::cwpdb::schema_ref(record.kind.as_str())
        }) {
            return invalid("invalid retained conversation record identity or schema");
        }
        Ok(record)
    }
    pub fn metadata_sources(&mut self, reference: Option<&str>) -> CwrResult<MetadataSources> {
        let page = self.pages(Selection::ConversationMetadata {
            conversation_ref: reference.map(str::to_owned),
        })?;
        page.records
            .iter()
            .filter(|record| record.kind == RecordKind::Conversation)
            .map(|record| {
                let conversation = snapshot::decode_conversation(&page, &record.reference)?;
                Ok((
                    record.reference.clone(),
                    (
                        conversation.messages.into_iter().next(),
                        conversation.activity,
                    ),
                ))
            })
            .collect()
    }
    pub fn commit_changes(&mut self, changes: Changes) -> CwrResult<i64> {
        let request = Mutation {
            scope: self.scope.clone(),
            request_id: Uuid::new_v4().to_string(),
            expected_revision: self.revision.clone(),
            changes,
            journal: None,
            authority_guards: Vec::new(),
        };
        let result = self.storage.commit(&request, self.authorizer.as_ref())?;
        match result {
            CommitOutcome::Committed {
                revision,
                commit_revision,
                ..
            } => {
                self.revision = revision;
                Ok(commit_revision)
            }
            CommitOutcome::CommittedResultExpired => invalid("conversation commit result expired"),
        }
    }
    pub fn set_expected_revision(&mut self, revision: i64) {
        if !self.native {
            return;
        }
        let next = hex::encode(Sha256::digest(
            json!([format!("{}/cwpdb", self.scope.runtime_root_ref), revision])
                .to_string()
                .as_bytes(),
        ));
        if self.revision != next {
            self.cache_initialized = false;
        }
        self.revision = next;
    }
    pub fn prepare(&mut self, state: &ConversationRuntimeState) -> CwrResult<()> {
        if self.cache_initialized {
            return Ok(());
        }
        self.publications = projected_publications(state)?
            .into_iter()
            .map(|publication| (publication.conversation_ref.clone(), publication))
            .collect();
        self.cache_initialized = true;
        Ok(())
    }
    pub fn conversation(&mut self, reference: &str) -> CwrResult<RetainedConversation> {
        let page = self.pages(Selection::Conversation {
            conversation_ref: reference.into(),
        })?;
        snapshot::decode_conversation(&page, reference)
            .map_err(|error| invalid_error(format!("conversation record: {error}")))
    }
    fn changes(&self, state: &ConversationRuntimeState) -> CwrResult<Changes> {
        let mut changes = changes(state)?;
        changes.publications.retain_mut(|publication| {
            let previous = self.publications.get(&publication.conversation_ref);
            let old_items = previous.map(|old| old.items.as_slice()).unwrap_or_default();
            let first = old_items
                .iter()
                .zip(&publication.items)
                .take_while(|(old, new)| old == new)
                .count();
            let anchors_changed = first != old_items.len() || first != publication.items.len();
            publication.from_position = first as u64;
            publication.items.drain(..first);
            publication.remove_activities = previous
                .into_iter()
                .flat_map(|old| &old.activities)
                .filter(|old| {
                    !publication
                        .activities
                        .iter()
                        .any(|new| new.activity_ref == old.activity_ref)
                })
                .map(|old| old.activity_ref.clone())
                .collect();
            publication.activities.retain(|new| {
                previous.is_none_or(|old| !old.activities.iter().any(|old| old == new))
            });
            anchors_changed
                || !publication.activities.is_empty()
                || !publication.remove_activities.is_empty()
        });
        Ok(changes)
    }

    pub fn commit(
        &mut self,
        state: &ConversationRuntimeState,
        journal: Option<JournalCommit>,
        authority_guards: Vec<AuthorityGuard>,
    ) -> CwrResult<i64> {
        let request = Mutation {
            scope: self.scope.clone(),
            request_id: Uuid::new_v4().to_string(),
            expected_revision: self.revision.clone(),
            changes: self.changes(state)?,
            journal,
            authority_guards,
        };
        self.validate_changes(&request.changes)?;
        // Retry exactly the original operation after an uncertain response;
        // never regenerate a request id or relax its expected revision.
        let outcome = match self.storage.commit(&request, self.authorizer.as_ref()) {
            Ok(outcome) => outcome,
            Err(error) => match self.storage.reconcile(&request, self.authorizer.as_ref())? {
                Some(outcome) => outcome,
                None => {
                    if error.to_string().contains("conversation revision_conflict") {
                        return invalid("CWR runtime changed through another open writer; reopen it before retrying");
                    }
                    return Err(error);
                }
            },
        };
        match outcome {
            CommitOutcome::Committed {
                revision,
                commit_revision,
                ..
            } => {
                self.revision = revision;
                for publication in &request.changes.publications {
                    let previous = self
                        .publications
                        .entry(publication.conversation_ref.clone())
                        .or_insert_with(|| PublicationSuffix {
                            conversation_ref: publication.conversation_ref.clone(),
                            from_position: 0,
                            items: Vec::new(),
                            activities: Vec::new(),
                            remove_activities: Vec::new(),
                        });
                    previous.items.truncate(publication.from_position as usize);
                    previous.items.extend(publication.items.clone());
                    previous.activities.retain(|old| {
                        !publication.remove_activities.contains(&old.activity_ref)
                            && !publication
                                .activities
                                .iter()
                                .any(|new| new.activity_ref == old.activity_ref)
                    });
                    previous.activities.extend(publication.activities.clone());
                }
                Ok(commit_revision)
            }
            CommitOutcome::CommittedResultExpired => {
                invalid("conversation commit result expired; reload before continuing")
            }
        }
    }
    pub fn pages(&mut self, selection: Selection) -> CwrResult<SnapshotPage> {
        let mut request = ReadRequest {
            scope: self.scope.clone(),
            selection,
            continuation: None,
            page_size: 1000,
            max_bytes: 24 * 1024 * 1024,
            expected_revision: (!self.revision.is_empty()).then(|| self.revision.clone()),
        };
        let mut result = self.storage.read(&request, self.authorizer.as_ref())?;
        let revision = result.revision.clone();
        while !result.complete {
            request.continuation = Some(result.continuation.take().ok_or_else(|| {
                invalid_error("incomplete conversation page has no continuation")
            })?);
            request.expected_revision = Some(revision.clone());
            let page = self.storage.read(&request, self.authorizer.as_ref())?;
            if page.revision != revision || page.metadata != result.metadata {
                return invalid("conversation bootstrap changed between pages");
            }
            result.records.extend(page.records);
            result.relationships.extend(page.relationships);
            result.continuation = page.continuation;
            result.complete = page.complete;
        }
        self.validate_snapshot(&result)?;
        Ok(result)
    }
}

pub(super) struct ScopeAuthorizer(pub(super) cwpdb_store::invocation::Context);
impl Authorizer for ScopeAuthorizer {
    fn authorize(
        &self,
        context: &cwpdb_store::invocation::Context,
        _: &str,
        _: bool,
    ) -> cwpdb_store::CwpdbResult<()> {
        if context != &self.0 {
            return Err("conversation backend caller mismatch".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod fixtures;

#[cfg(test)]
pub(super) fn fixture_decode_tool_operations(
    page: &SnapshotPage,
) -> CwrResult<BTreeMap<String, tools::ToolOperation>> {
    snapshot::decode_tool_operations(page)
}

#[cfg(test)]
pub(super) fn fixture_conversation_headers(
    page: &SnapshotPage,
    runtime: &str,
) -> CwrResult<Vec<RetainedConversation>> {
    Ok(snapshot::decode_state(page, runtime)?
        .conversations
        .iter()
        .cloned()
        .collect())
}
