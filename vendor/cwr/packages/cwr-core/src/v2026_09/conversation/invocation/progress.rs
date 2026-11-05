impl ConversationAdmissionStore {
    /// Claims exactly one runnable attempt, persists that claim before it is
    /// returned, and reconstructs the adapter invocation from retained state.
    pub fn claim_invocation(
        &mut self,
        invocation_ref: &str,
        started_at_unix_ms: u64,
    ) -> CwrResult<ConversationInvocationClaimOutcome> {
        let invocation_ref = invocation_ref.to_owned();
        let model_run_ref = self.new_ref("model_runs");
        let current_browse_permission = self.response_graph.as_ref().map(|graph| {
            let Some(record) = self.state.invocation_ledger.invocation(&invocation_ref) else {
                return false;
            };
            graph.actor_integrations.iter().any(|binding| {
                binding.actor_ref == record.target_actor_ref
                    && Some(binding.actor_integration_ref.as_str())
                        == record.actor_integration_ref.as_deref()
                    && binding_allows_cwr_action(binding)
            })
        });
        let current_graph = self.response_graph.clone();
        let tool_activity_capture = self.timeline_rollout != Some(ConversationTimelineRollout::Disabled);
        self.transact_invocations(move |state| {
            let boundary = execution_conversation_in_state(state, &invocation_ref)?
                .activity.last().map(|activity| activity.sequence).unwrap_or(0);
            let outcome = state
                .invocation_ledger
                .claim_after_activity(&invocation_ref, started_at_unix_ms, Some(boundary))?;
            match outcome {
                ClaimInvocationOutcome::NotRunnable {
                    state,
                    attempt_active,
                } => Ok(ConversationInvocationClaimOutcome::NotRunnable {
                    state,
                    attempt_active,
                }),
                ClaimInvocationOutcome::Claimed(claim) => {
                    super::working::initialize(state, &invocation_ref, current_browse_permission, current_graph.as_ref())?;
                    if claim.invocation.attempt_count > 1 {
                        push_execution_activity_in_state(state, &invocation_ref,
                            ConversationActivityEvent::ExecutionActivity {
                                activity: crate::adapter::ExecutionActivity::started(crate::adapter::ExecutionPhase::Resuming),
                            }, started_at_unix_ms)?;
                    }
                    if state.timeline_store.iter().any(|(_, timeline)| timeline.connections.iter().flat_map(|connection| &connection.items).any(|item| matches!(&item.content, ConversationTimelineContent::Invocation { invocation_ref: reference } if reference == &invocation_ref))) {
                        state.streaming_runs.insert(invocation_ref.clone(), crate::v2026_09::conversation::streaming::StreamingRun { model_run_ref: model_run_ref.clone(), closed: false, sequences: BTreeMap::new() });
                    }
                    super::tools::begin_attempt(state, &invocation_ref, claim.invocation.attempt_count, tool_activity_capture);
                    let claim = *claim;
                    let claimed = reconstruct_claimed_invocation(state, claim, model_run_ref)?;
                    Ok(ConversationInvocationClaimOutcome::Claimed(Box::new(
                        claimed,
                    )))
                }
            }
        })
    }

    pub fn record_invocation_progress(
        &mut self,
        invocation_ref: &str,
        summary: impl Into<String>,
    ) -> CwrResult<ActionResponse> {
        let invocation_ref = invocation_ref.to_owned();
        let summary = summary.into();
        self.transact_invocations(move |state| {
            state.invocation_ledger.record_progress(
                &invocation_ref,
                ActionResponseStatus::PROGRESS,
                summary,
            )
        })
    }

    /// Persists the exact progress response and its diagnostic activity in one
    /// state replacement, so recovery can never observe only one projection.
    pub fn record_invocation_progress_with_activity(
        &mut self,
        invocation_ref: &str,
        status: u16,
        summary: impl Into<String>,
    ) -> CwrResult<ActionResponse> {
        self.record_invocation_progress_with_publication(invocation_ref, status, summary, false)
    }

    pub fn record_invocation_progress_with_publication(
        &mut self, invocation_ref: &str, status: u16, summary: impl Into<String>, published: bool,
    ) -> CwrResult<ActionResponse> {
        self.record_measured_invocation_progress(invocation_ref, status, summary, published, None)
    }

    pub fn record_measured_invocation_progress(
        &mut self, invocation_ref: &str, status: u16, summary: impl Into<String>, published: bool,
        progress: Option<crate::v2026_09::invocation::ActionProgress>,
    ) -> CwrResult<ActionResponse> {
        let status =
            ActionResponseStatus::new(status).map_err(|error| invalid_error(error.to_string()))?;
        if !status.is_progress() {
            return invalid(format!(
                "invocation progress activity status must be 1xx, not {}",
                status.as_u16()
            ));
        }
        let invocation_ref = invocation_ref.to_owned();
        let summary = summary.into();
        let occurred_at_unix_ms = current_unix_ms();
        self.transact_invocations(move |state| {
            let response =
                state
                    .invocation_ledger
                    .record_measured_progress(&invocation_ref, status, summary, progress)?;
            let activity = push_execution_activity_in_state(
                state,
                &invocation_ref,
                ConversationActivityEvent::Progress {
                    progress: response.progress.clone(),
                    status,
                    summary: response.summary.clone(),
                },
                occurred_at_unix_ms,
            )?;
            if published {
                super::working::publish_progress(state, &invocation_ref, &activity,
                    response.summary.clone(), Some(status))?;
            }
            Ok(response)
        })
    }

    /// Persists one completed model commentary item without creating an
    /// action response. Provider item identity makes replay idempotent, while
    /// the claimed attempt count fences output from yielded or recovered runs.
    pub fn record_commentary_progress(
        &mut self,
        invocation_ref: &str,
        attempt_count: u64,
        commentary: AdapterCompletedCommentary,
    ) -> CwrResult<RecordCommentaryProgressOutcome> {
        validate_completed_commentary(&commentary)?;
        if attempt_count == 0 {
            return invalid("commentary progress requires a claimed model run");
        }
        let provider_integration_ref =
            commentary_provider_integration(&self.state, invocation_ref)?;
        let page = self.backend.lock().map_err(|_| invalid_error("conversation backend lock poisoned"))?.pages(cwpdb_store::conversation::Selection::Commentary {
            provider_integration_ref: provider_integration_ref.clone(), response_or_turn_id: commentary.response_or_turn_id.clone(), item_id: commentary.item_id.clone(),
        })?;
        let retained = match page.records.as_slice() {
            [] => None,
            [record] if record.kind == cwpdb_store::conversation::RecordKind::Activity => Some(serde_json::from_value(record.properties["record"].clone())?),
            _ => return invalid("provider commentary item was retained more than once or invalid"),
        };
        self.transact(|staged| {
            let occurred_at_unix_ms = current_unix_ms();
            let outcome = stage_commentary_progress(
                staged,
                invocation_ref,
                attempt_count,
                commentary,
                occurred_at_unix_ms,
                provider_integration_ref,
                retained,
            )?;
            Ok(outcome)
        })
    }
}

fn binding_allows_cwr_action(
    binding: &crate::v2026_09::graph::ResolvedCwrActorIntegration,
) -> bool {
    let Some(allowed) = binding.actor_integration_params.get("cwr_action_actor_refs") else {
        return true;
    };
    let Some(refs) = allowed.as_array() else {
        return false;
    };
    refs.iter()
        .any(|reference| reference == crate::action_contract::INTRINSIC_CWR_ACTOR_REF)
}
