use crate::v2026_09::conversation::HistoryProjection;
#[derive(Default)]
pub(crate) struct BrowseDiagnosticContext<'a> {
    pub invocation_ref: Option<&'a str>,
    pub request_id: Option<&'a str>,
    pub operation_id: Option<&'a str>,
}

impl LocalRuntime {
    pub fn visualization_artifact(
        &self,
        artifact_ref: &str,
    ) -> CwrResult<ConversationVisualizationArtifact> {
        self.inner
            .lock()
            .map_err(|_| invalid_error("local runtime lock was poisoned"))?
            .admission
            .visualization_artifact(artifact_ref)
    }

    pub fn participant_conversation_timeline(
        &self,
        conversation_ref: &str,
        participant_actor_ref: &str,
        page: &ConversationRecordPageRequest,
        deadline: Instant,
    ) -> CwrResult<ConversationTimelinePage> {
        self.participant_conversation_timeline_cancellable(
            conversation_ref,
            participant_actor_ref,
            page,
            deadline,
            || false,
            |_| {},
        )
    }

    pub fn participant_conversation_timeline_cancellable(
        &self,
        conversation_ref: &str,
        participant_actor_ref: &str,
        page: &ConversationRecordPageRequest,
        deadline: Instant,
        is_cancelled: impl Fn() -> bool,
        register_cancellation: impl FnOnce(ConversationReadCancellation),
    ) -> CwrResult<ConversationTimelinePage> {
        let state = self.read_lock_until(deadline, &is_cancelled)?;
        let all = state.graph.permissions.allows_all();
        let permitted = state
            .admission
            .conversation_header(conversation_ref)
            .is_some_and(|conversation| {
                all || (conversation.parent_conversation_ref.is_none()
                    && conversation
                        .participant_actor_refs
                        .iter()
                        .any(|actor| actor == participant_actor_ref))
            });
        if !permitted {
            return invalid("conversation is unavailable to the requesting participant");
        }
        let reader = state.admission.canonical_timeline_reader();
        drop(state);
        let cancellation = ConversationReadCancellation::pending();
        register_cancellation(cancellation.clone());
        if is_cancelled() {
            cancellation.cancel();
        }
        cancellation.ensure_active(deadline)?;
        let page = reader.page(
            conversation_ref,
            page,
            (!all).then_some(participant_actor_ref),
            all,
            &cancellation.storage_control(deadline),
        )?;
        if is_cancelled() {
            cancellation.cancel();
        }
        cancellation.ensure_active(deadline)?;
        Ok(page)
    }

    pub fn participant_conversation_browse(
        &self,
        conversation_ref: &str,
        participant_actor_ref: &str,
        request: &crate::intrinsic_cwr::ConversationBrowseRequest,
        deadline: Instant,
    ) -> CwrResult<crate::intrinsic_cwr::ConversationBrowseResponse> {
        self.participant_conversation_browse_with_context(
            conversation_ref,
            participant_actor_ref,
            request,
            deadline,
            BrowseDiagnosticContext::default(),
        )
    }

    pub(crate) fn participant_conversation_browse_with_context(
        &self,
        conversation_ref: &str,
        participant_actor_ref: &str,
        request: &crate::intrinsic_cwr::ConversationBrowseRequest,
        deadline: Instant,
        context: BrowseDiagnosticContext<'_>,
    ) -> CwrResult<crate::intrinsic_cwr::ConversationBrowseResponse> {
        let cancellation = ConversationReadCancellation::pending();
        let state = loop {
            cancellation.ensure_active(deadline)?;
            match self.inner.try_lock() {
                Ok(state) => break state,
                Err(TryLockError::WouldBlock) => thread::sleep(Duration::from_millis(1)),
                Err(TryLockError::Poisoned(_)) => {
                    return invalid("local runtime lock was poisoned");
                }
            }
        };
        let all = state.graph.permissions.allows_all();
        let participates = state
            .admission
            .conversation_header(conversation_ref)
            .is_some_and(|conversation| {
                all || (conversation.parent_conversation_ref.is_none()
                    && conversation
                        .participant_actor_refs
                        .iter()
                        .any(|actor_ref| actor_ref == participant_actor_ref))
            });
        if !participates {
            return invalid("conversation is unavailable to the requesting participant");
        }
        let operation = state.admission.canonical_history_reader(
            conversation_ref,
            (!all).then(|| participant_actor_ref.to_owned()),
            all,
            cancellation.storage_control(deadline),
        );
        drop(state);
        cancellation.ensure_active(deadline)?;
        use crate::intrinsic_cwr::{
            ConversationBrowseOperation as Operation, ConversationBrowseRecords as Records,
        };
        let records: CwrResult<Records> = (|| {
            Ok(match &request.operation {
                Operation::Recent {
                    page,
                    excerpt_bytes,
                } => Records::Page {
                    page: operation.conversation_browse_page(
                        conversation_ref,
                        page.window(),
                        *excerpt_bytes,
                    )?,
                },
                Operation::Around {
                    message_ref,
                    before,
                    after,
                    excerpt_bytes,
                } => Records::Page {
                    page: operation.conversation_history_around(
                        conversation_ref,
                        message_ref,
                        *before,
                        *after,
                        *excerpt_bytes,
                    )?,
                },
                Operation::Search {
                    query,
                    after_position,
                    after_revision,
                    limit,
                    excerpt_bytes,
                } => Records::Search {
                    page: operation.conversation_history_search(
                        conversation_ref,
                        query,
                        *after_position,
                        after_revision.as_deref(),
                        *limit,
                        *excerpt_bytes,
                    )?,
                },
                Operation::Excerpt {
                    message_ref,
                    offset_bytes,
                    action_payload_offset_bytes,
                    max_bytes,
                } => Records::Excerpt {
                    entry: Box::new(operation.conversation_message_excerpt(
                        conversation_ref,
                        message_ref,
                        *offset_bytes,
                        *action_payload_offset_bytes,
                        *max_bytes,
                    )?),
                },
                Operation::Links {
                    message_ref,
                    after_reply_message_ref,
                    after_revision,
                    limit,
                } => Records::Links {
                    links: operation.conversation_message_links(
                        conversation_ref,
                        message_ref,
                        after_reply_message_ref.as_deref(),
                        after_revision.as_deref(),
                        *limit,
                    )?,
                },
            })
        })();
        let records = records.inspect_err(|error| {
            if let Some(overage) =
                crate::v2026_09::conversation::BrowseMessageOverage::in_error(error.as_ref())
            {
                self.diagnostics.record_browse_overage(
                    overage,
                    context.invocation_ref,
                    context.request_id,
                    context.operation_id,
                );
            }
        })?;
        let response = crate::intrinsic_cwr::ConversationBrowseResponse {
            conversation_ref: conversation_ref.to_owned(),
            records,
        };
        let response_bytes = serde_json::to_vec(&response)
            .map_err(|error| invalid_error(format!("could not encode browse result: {error}")))?
            .len();
        if response_bytes > crate::v2026_09::conversation::MAX_CONVERSATION_BROWSE_RESPONSE_BYTES {
            return invalid("conversation browse response exceeds the byte limit");
        }
        Ok(response)
    }

    pub fn conversation_records(
        &self,
        request: &ConversationRecordReadRequest,
        deadline: Instant,
        is_cancelled: impl Fn() -> bool,
        register_cancellation: impl FnOnce(ConversationReadCancellation),
    ) -> CwrResult<ConversationRecordPage> {
        request.validate()?;
        let cancellation = ConversationReadCancellation::pending();
        register_cancellation(cancellation.clone());
        let (location, runtime_root_ref) = loop {
            cancellation.ensure_active(deadline)?;
            if is_cancelled() {
                cancellation.cancel();
                cancellation.ensure_active(deadline)?;
            }
            match self.inner.try_lock() {
                Ok(state) => break (state.admission.read_store_location(), state.admission.runtime_root_ref().to_owned()),
                Err(TryLockError::WouldBlock) => {
                    thread::sleep(Duration::from_millis(1));
                }
                Err(TryLockError::Poisoned(_)) => {
                    return invalid("local runtime lock was poisoned");
                }
            }
        };
        cancellation.ensure_active(deadline)?;
        if is_cancelled() {
            cancellation.cancel();
            cancellation.ensure_active(deadline)?;
        }
        let mut store =
            location.open_until(deadline, &cancellation)?;
        let operation = store.begin_operation_until(deadline, cancellation)?;
        match request {
            ConversationRecordReadRequest::Summary {
                conversation_ref,
                workspace_owner_context_ref,
                participant_actor_ref,
            } => Ok(ConversationRecordPage::Summary {
                summary: operation.workspace_conversation_summary(
                    conversation_ref,
                    workspace_owner_context_ref.as_deref(),
                    participant_actor_ref,
                )?,
            }),
            ConversationRecordReadRequest::Resource { resource_ref } => {
                Ok(ConversationRecordPage::Resource {
                    resource: operation.conversation_resource(resource_ref)?.map(Box::new),
                })
            }
            ConversationRecordReadRequest::Completions {
                conversation_ref,
                after_sequence,
                through_sequence,
                timeline_revision,
                first,
            } => Ok(ConversationRecordPage::Completions {
                page: operation.conversation_completions(
                    conversation_ref,
                    &runtime_root_ref,
                    *after_sequence,
                    *through_sequence,
                    timeline_revision.as_deref(),
                    *first,
                )?,
            }),
            ConversationRecordReadRequest::Content {
                conversation_ref,
                page,
            } => Ok(ConversationRecordPage::Content {
                page: operation.conversation_content(conversation_ref, page.window())?,
            }),
            ConversationRecordReadRequest::Timeline {
                conversation_ref,
                page,
            } => Ok(ConversationRecordPage::Timeline {
                page: operation.conversation_timeline(conversation_ref, page.window())?,
            }),
        }
    }

    pub fn root_conversation_summaries(
        &self,
        request: &ConversationSummaryPageRequest,
        deadline: Instant,
        is_cancelled: impl Fn() -> bool,
        register_cancellation: impl FnOnce(ConversationReadCancellation),
    ) -> CwrResult<ConversationSummaryPage> {
        let cancellation = ConversationReadCancellation::pending();
        register_cancellation(cancellation.clone());
        let location = loop {
            cancellation.ensure_active(deadline)?;
            if is_cancelled() {
                cancellation.cancel();
                cancellation.ensure_active(deadline)?;
            }
            match self.inner.try_lock() {
                Ok(state) => break state.admission.read_store_location(),
                Err(TryLockError::WouldBlock) => {
                    thread::sleep(Duration::from_millis(1));
                }
                Err(TryLockError::Poisoned(_)) => {
                    return invalid("local runtime lock was poisoned");
                }
            }
        };
        cancellation.ensure_active(deadline)?;
        if is_cancelled() {
            cancellation.cancel();
            cancellation.ensure_active(deadline)?;
        }
        let mut store =
            location.open_until(deadline, &cancellation)?;
        let operation = store
            .begin_operation_until(deadline, cancellation)?
            .scope_to_owner(request);
        operation.root_conversations(request.window())
    }

    pub fn participant_root_conversation_summaries(
        &self,
        participant_actor_ref: &str,
        request: &ConversationSummaryPageRequest,
        archived: Option<bool>,
        deadline: Instant,
    ) -> CwrResult<ConversationSummaryPage> {
        self.participant_root_conversation_summaries_cancellable(
            participant_actor_ref,
            request,
            archived,
            deadline,
            || false,
            |_| {},
        )
    }

    pub fn participant_root_conversation_summaries_cancellable(
        &self,
        participant_actor_ref: &str,
        request: &ConversationSummaryPageRequest,
        archived: Option<bool>,
        deadline: Instant,
        is_cancelled: impl Fn() -> bool,
        register_cancellation: impl FnOnce(ConversationReadCancellation),
    ) -> CwrResult<ConversationSummaryPage> {
        request.validate()?;
        let cancellation = ConversationReadCancellation::pending();
        register_cancellation(cancellation.clone());
        let state = self.read_lock_until(deadline, &|| {
            is_cancelled() || cancellation.ensure_active(deadline).is_err()
        })?;
        let all = state.graph.permissions.allows_all();
        let location = state.admission.read_store_location();
        drop(state);
        let mut store =
            location.open_until(deadline, &cancellation)?;
        let operation = store
            .begin_operation_until(deadline, cancellation)?
            .scope_to_owner(request);
        operation.root_conversations_in_scope(
            request.window(),
            if all {
                None
            } else {
                Some(participant_actor_ref)
            },
            archived,
        )
    }
    fn read_lock_until(
        &self,
        deadline: Instant,
        is_cancelled: &impl Fn() -> bool,
    ) -> CwrResult<MutexGuard<'_, LocalRuntimeState>> {
        loop {
            if is_cancelled() {
                return invalid("runtime read was cancelled");
            }
            if Instant::now() >= deadline {
                return invalid("runtime read exceeded its deadline");
            }
            match self.inner.try_lock() {
                Ok(state) => return Ok(state),
                Err(TryLockError::WouldBlock) => thread::sleep(Duration::from_millis(1)),
                Err(TryLockError::Poisoned(_)) => {
                    return invalid("local runtime lock was poisoned")
                }
            }
        }
    }

    pub fn host_conversation_metadata_until(
        &self,
        conversation_ref: &str,
        deadline: Instant,
        is_cancelled: impl Fn() -> bool,
    ) -> CwrResult<Option<RetainedConversation>> {
        let state = self.read_lock_until(deadline, &is_cancelled)?;
        crate::v2026_09::host::project_host_conversation_metadata(
            &state.admission,
            conversation_ref,
        )
    }

    /// Message targets exclude software providers that only declare named actions.
    pub fn upstream_message_targets_as_until(
        &self,
        actor: &str,
        owner: Option<&str>,
        deadline: Instant,
        is_cancelled: impl Fn() -> bool,
    ) -> CwrResult<Vec<LocalActorIdentity>> {
        let state = self.read_lock_until(deadline, &is_cancelled)?;
        message_target_identities(&state, actor, owner)
    }

    pub fn upstream_catalog_as_until(
        &self,
        actor: &str,
        owner: Option<&str>,
        deadline: Instant,
        is_cancelled: impl Fn() -> bool,
    ) -> CwrResult<LocalCatalog> {
        let state = self.read_lock_until(deadline, &is_cancelled)?;
        let mut catalog = local_catalog(&upstream_graph(&state, owner)?, actor)?;
        catalog.retained_actors = state.catalog.retained_actors.clone();
        Ok(catalog)
    }
}

impl LocalRuntime {
    pub fn publication_snapshot_until(
        &self,
        actor: &str,
        owner: Option<&str>,
        deadline: Instant,
        is_cancelled: impl Fn() -> bool,
        register: impl FnOnce(ConversationReadCancellation),
    ) -> CwrResult<crate::v2026_09::conversation::ConversationPublicationSnapshot> {
        let state = self.read_lock_until(deadline, &is_cancelled)?;
        local_catalog(&upstream_graph(&state, owner)?, actor)?;
        let location = state.admission.read_store_location();
        drop(state);
        let cancellation = ConversationReadCancellation::pending();
        register(cancellation.clone());
        if is_cancelled() {
            cancellation.cancel();
        }
        crate::v2026_09::conversation::ConversationPublicationSnapshot::from_store(
            location.open_until(deadline, &cancellation)?,
            actor,
            deadline,
            cancellation,
        )
    }
}

impl LocalRuntime {
    pub fn record_streaming_output(
        &self,
        output: Option<&crate::adapter::AdapterLiveOutput>,
        closed: &[crate::adapter::AdapterClosedModelRun],
    ) -> CwrResult<bool> {
        self.lock()?
            .admission
            .record_streaming_output(output, closed)
    }
}

fn message_target_identities(
    state: &LocalRuntimeState,
    actor: &str,
    owner: Option<&str>,
) -> CwrResult<Vec<LocalActorIdentity>> {
    let catalog = local_catalog(&upstream_graph(state, owner)?, actor)?;
    let action_only = state
        .upstream
        .visible(owner)?
        .into_iter()
        .filter(|route| {
            !route
                .coworker
                .actions
                .iter()
                .any(|action| action.action_name == "respond_to--message")
        })
        .map(|route| route.coworker.actor_ref.clone())
        .collect::<std::collections::BTreeSet<_>>();
    Ok(catalog
        .known_actors
        .into_iter()
        .filter(|identity| {
            identity.actor_ref != actor && !action_only.contains(&identity.actor_ref)
        })
        .collect())
}
