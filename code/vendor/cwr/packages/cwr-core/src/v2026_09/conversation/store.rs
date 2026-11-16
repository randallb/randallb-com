impl ConversationAdmissionStore {
    pub fn open(
        state_path: impl Into<PathBuf>,
        runtime_root_ref: impl Into<String>,
    ) -> CwrResult<Self> {
        Self::open_internal(state_path.into(), runtime_root_ref.into(), None, None, false)
    }

    /// Opens a standalone writer with the same publication maintenance and
    /// legacy backfill used by a local runtime. The caller must own the writer
    /// lease; use the read store to inspect a running runtime without mutation.
    pub fn open_managed(
        state_path: impl Into<PathBuf>,
        runtime_root_ref: impl Into<String>,
    ) -> CwrResult<Self> {
        let mut store = Self::open_with_timeline_rollout(
            state_path,
            runtime_root_ref,
            ConversationTimelineRollout::MigrateLegacy,
        )?;
        store.migrate_eligible_legacy_timelines()?;
        Ok(store)
    }

    pub fn open_bound(
        state_path: impl Into<PathBuf>,
        runtime_root_ref: impl Into<String>,
        workspace_id: &str,
        workspace_directory: &Path,
    ) -> CwrResult<Self> {
        Self::open_internal(
            state_path.into(),
            runtime_root_ref.into(),
            Some((workspace_id, workspace_directory)),
            None,
            false,
        )
    }

    pub(crate) fn open_with_timeline_rollout(
        state_path: impl Into<PathBuf>,
        runtime_root_ref: impl Into<String>,
        timeline_rollout: ConversationTimelineRollout,
    ) -> CwrResult<Self> {
        Self::open_internal(
            state_path.into(),
            runtime_root_ref.into(),
            None,
            Some(timeline_rollout),
            false,
        )
    }

    pub(crate) fn open_with_timeline_rollout_deferred(
        state_path: impl Into<PathBuf>,
        runtime_root_ref: impl Into<String>,
        timeline_rollout: ConversationTimelineRollout,
    ) -> CwrResult<Self> {
        Self::open_internal(
            state_path.into(),
            runtime_root_ref.into(),
            None,
            Some(timeline_rollout),
            true,
        )
    }

    pub(crate) fn open_bound_with_timeline_rollout_deferred(
        state_path: impl Into<PathBuf>,
        runtime_root_ref: impl Into<String>,
        workspace_id: &str,
        workspace_directory: &Path,
        timeline_rollout: ConversationTimelineRollout,
    ) -> CwrResult<Self> {
        Self::open_internal(
            state_path.into(),
            runtime_root_ref.into(),
            Some((workspace_id, workspace_directory)),
            Some(timeline_rollout),
            true,
        )
    }

    fn open_internal(
        state_path: PathBuf,
        runtime_root_ref: String,
        workspace_binding: Option<(&str, &Path)>,
        timeline_rollout: Option<ConversationTimelineRollout>,
        defer_history: bool,
    ) -> CwrResult<Self> {
        use cwpdb_store::conversation::{NativeConversationStore,OpenOptions,Scope,WorkspaceBinding};
        use cwpdb_store::invocation::Context;
        if let Some(store)=Self::open_remote_environment(&runtime_root_ref,timeline_rollout.unwrap_or_default())? {return Ok(store);}
        if std::env::var_os("CWR_REQUIRE_REMOTE_STATE").is_some() {return invalid("this native startup path cannot satisfy required remote canonical state");}
        validate_runtime_root(&runtime_root_ref)?;
        let file_name=state_path.file_name().ok_or("CWR runtime database path has no file name")?.to_owned();
        let parent=state_path.parent().ok_or("CWR runtime database path has no parent directory")?;
        fs::create_dir_all(parent)?;
        let state_path=fs::canonicalize(parent)?.join(file_name);
        let context=Context {workspace_ref:runtime_root_ref.clone(),caller_ref:runtime_root_ref.clone()};
        let scope=Scope {context:context.clone(),runtime_root_ref};
        let storage=NativeConversationStore::open(&state_path,scope.clone(),OpenOptions {
            runtime_state_version:STATE_VERSION.into(),
            workspace_binding:workspace_binding.map(|(workspace_id,workspace_directory)|WorkspaceBinding {workspace_id:workspace_id.into(),workspace_directory:workspace_directory.into()}),
            timeline_rollout:timeline_rollout.map(|rollout| match rollout {ConversationTimelineRollout::Disabled=>"disabled",ConversationTimelineRollout::NewConversations=>"new_conversations",ConversationTimelineRollout::MigrateLegacy=>"migrate_legacy"}.into()),
        }).map_err(|error|invalid_error(format!("{}: CWR runtime database error: {error}",state_path.display())))?;
        let mut store=Self::open_declared_mode(storage,scope,backend::ScopeAuthorizer(context),timeline_rollout.unwrap_or_default(),defer_history)?;
        store.state_path=state_path;
        store.timeline_rollout=timeline_rollout;
        store.backend.lock().map_err(|_|invalid_error("conversation backend lock poisoned"))?.mark_native();
        if !defer_history {store.hydrate_all_cold_conversations()?;}
        Ok(store)
    }

    fn rebuild_admission_indices(&mut self) {
        self.admission_indices_by_conversation = admission_indices_by_conversation(&self.state);
    }
}

fn admission_indices_by_conversation(
    state: &ConversationRuntimeState,
) -> BTreeMap<String, Vec<usize>> {
    let mut indices = BTreeMap::<String, Vec<usize>>::new();
    for (index, admission) in state.admissions.iter().enumerate() {
        indices
            .entry(admission.conversation_ref.clone())
            .or_default()
            .push(index);
    }
    indices
}
