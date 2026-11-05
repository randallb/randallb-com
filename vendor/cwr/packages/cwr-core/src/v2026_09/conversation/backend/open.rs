use super::*;
use cwpdb_store::invocation::{Authorizer, Context};
impl ConversationAdmissionStore {
    /// Select a trusted declared backend for the ordinary CWR engine. No local
    /// SQLite mirror or alternative admission path is created.
    pub fn open_declared<S: ConversationStorage + 'static, A: Authorizer + Send + 'static>(
        storage: S,
        scope: Scope,
        authorizer: A,
        rollout: ConversationTimelineRollout,
    ) -> CwrResult<Self> {
        Self::open_declared_mode(storage, scope, authorizer, rollout, true)
    }
    pub(in crate::v2026_09::conversation) fn open_declared_mode<
        S: ConversationStorage + 'static,
        A: Authorizer + Send + 'static,
    >(
        storage: S,
        scope: Scope,
        authorizer: A,
        rollout: ConversationTimelineRollout,
        defer_history: bool,
    ) -> CwrResult<Self> {
        validate_runtime_root(&scope.runtime_root_ref)?;
        let (mut backend, mut state, mut revision) =
            ConversationBackend::declared(Box::new(storage), scope, Box::new(authorizer))?;
        let needs_history_recovery = backend.needs_history_recovery(&state)?;
        let stage = match rollout {
            ConversationTimelineRollout::Disabled => "disabled",
            ConversationTimelineRollout::NewConversations => "new_conversations",
            ConversationTimelineRollout::MigrateLegacy => "migrate_legacy",
        };
        if backend.metadata["conversation_timeline_rollout"]
            .as_str()
            .unwrap_or("disabled")
            != stage
        {
            revision = backend.commit_changes(Changes {
                timeline_rollout: Some(stage.into()),
                ..Changes::default()
            })?;
        }
        let maintenance = backend.metadata["storage_maintenance"]
            .as_array()
            .is_some_and(|flags| !flags.is_empty());
        // Legacy roots have no canonical timeline boundary. They require the
        // protocol recovery pass before any history may remain deferred.
        let legacy_history = state.conversations.iter().any(|conversation| {
            conversation.parent_conversation_ref.is_none()
                && conversation.timeline_connection_ref.is_none()
        });
        if maintenance || !defer_history || legacy_history || needs_history_recovery {
            let mut conversations = Vec::new();
            for conversation in state.conversations.iter() {
                conversations.push(backend.conversation(&conversation.conversation_ref)?);
            }
            state.conversations = conversations.into();
            state.cold_conversation_refs.clear();
        }
        state.begin();
        if state.cold_conversation_refs.is_empty() {
            streaming::withdraw_on_restart(&mut state)?;
            context::remove_legacy_resume_fields(&mut state);
            state.invocation_ledger.validate()?;
            invocation::validate_invocation_execution_links(&state)?;
            timeline::validate_retained_timelines(&state)?;
            visualization::validate_artifacts(&state)?;
            context::migrate_snapshots(&mut state)?;
            terminal::migrate_suspended_invocations(&mut state)?;
            terminal::reconcile_terminal_root_projections(&mut state)?;
        } else {
            // Closed histories are deliberately absent from Bootstrap. Validate
            // their protocol contents when selected; never reconcile an empty
            // cold header as though retained replies had disappeared.
            state.invocation_ledger.validate()?;
            invocation::validate_recovery_execution_links(&state)?;
        }
        timeline::stage_timeline_rollout_changes(&mut state, rollout)?;
        state.invocation_ledger.validate_changes()?;
        invocation::validate_changed_execution_links(&state)?;
        timeline::validate_changed_timelines(&state)?;
        if state.has_changes() || maintenance {
            let mut changes = changes(&state)?;
            if maintenance {
                changes.repair_legacy = true;
                changes::repair_conversation_headers(&mut changes, &state);
                changes.publications = projected_publications(&state)?;
            }
            revision = backend.commit_changes(changes)?;
        }
        state.commit();
        let policy = backend.metadata.get("context_policy");
        let context_policy_configured = policy
            .and_then(|value| value["configured"].as_bool())
            .unwrap_or(false);
        let context_policy = policy
            .and_then(|value| value.get("policy"))
            .filter(|value| !value.is_null())
            .map(|value| serde_json::from_value(value.clone()))
            .transpose()?;
        let indices = admission_indices_by_conversation(&state);
        Ok(Self {
            metrics: None,
            context_policy_configured,
            context_policy,
            response_graph: None,
            state_path: PathBuf::from("remote-cwpdb"),
            commit_revision: revision,
            pending_authority_guards: Vec::new(),
            backend: std::sync::Arc::new(std::sync::Mutex::new(backend)),
            admission_indices_by_conversation: indices,
            timeline_rollout: Some(rollout),
            state,
        })
    }
    pub(in crate::v2026_09::conversation) fn open_remote_environment(
        runtime_root_ref: &str,
        rollout: ConversationTimelineRollout,
    ) -> CwrResult<Option<Self>> {
        let Some(endpoint) = std::env::var_os("CWR_CONVERSATION_STORAGE_URL") else {
            return Ok(None);
        };
        let required = |name: &str| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    invalid_error(format!("Remote conversation storage requires {name}"))
                })
        };
        let context = Context {
            workspace_ref: required("CWR_CONVERSATION_WORKSPACE_REF")?,
            caller_ref: required("CWR_CONVERSATION_CALLER_REF")?,
        };
        CwrRef::parse(&context.workspace_ref)?;
        CwrRef::parse(&context.caller_ref)?;
        let scope = Scope {
            context: context.clone(),
            runtime_root_ref: runtime_root_ref.into(),
        };
        let transport = crate::cloud_execution::HttpDeclaredTransport::new(
            endpoint.to_str().ok_or("Invalid remote storage endpoint")?,
            required("CWR_CONVERSATION_STORAGE_TOKEN")?,
        )?;
        let storage = cwpdb_store::remote::RemoteStore::new(transport);
        Self::open_declared(storage, scope, ScopeAuthorizer(context), rollout).map(Some)
    }
}
