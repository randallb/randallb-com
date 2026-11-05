impl ConversationAdmissionStore {
    pub fn invocation_record(&self, invocation_ref: &str) -> Option<&ActionInvocationRecord> {
        self.state.invocation_ledger.invocation(invocation_ref)
    }

    pub fn invocation_records(&self) -> impl Iterator<Item = &ActionInvocationRecord> {
        self.state.invocation_ledger.invocations()
    }

    pub fn invocation_waits(&self) -> &[InvocationWait] {
        self.state.invocation_ledger.waits()
    }

    /// Resolves the exact retained invocation a host should consider for a stop.
    ///
    /// An explicit selector uses the same full-ref or exact final-segment ID
    /// semantics as invocation inspection and may select terminal work so the
    /// caller can report `too_late`. Without a selector, only nonterminal ingress
    /// roots in the supplied conversation are considered. A unique running or
    /// waiting root wins over queued admissions; otherwise ambiguity is returned
    /// rather than guessing from map order.
    pub fn select_invocation_for_stop(
        &self,
        conversation_ref: Option<&str>,
        selector: Option<&str>,
    ) -> CwrResult<InvocationStopSelection> {
        self.select_invocation_for_stop_excluding(conversation_ref, selector, None)
    }

    pub(crate) fn select_invocation_for_stop_excluding(
        &self,
        conversation_ref: Option<&str>,
        selector: Option<&str>,
        excluded_invocation_ref: Option<&str>,
    ) -> CwrResult<InvocationStopSelection> {
        if let Some(selector) = selector {
            let (selector, full_ref) = super::inspection::validate_invocation_selector(selector)?;
            let mut matches = self
                .state
                .invocation_ledger
                .invocations()
                .filter(|record| {
                    conversation_ref.is_none_or(|conversation_ref| {
                        record.conversation_ref.as_deref() == Some(conversation_ref)
                    }) && super::inspection::invocation_matches(
                        &record.invocation_ref,
                        selector,
                        full_ref,
                    )
                })
                .collect::<Vec<_>>();
            sort_stop_candidates(&mut matches);
            return Ok(stop_selection(matches));
        }

        let conversation_ref = conversation_ref
            .filter(|conversation_ref| !conversation_ref.trim().is_empty())
            .ok_or_else(|| {
                invalid_error("stopping without an invocation selector requires a conversation")
            })?;
        let roots = self
            .state
            .invocation_ledger
            .invocations()
            .filter(|record| {
                record.conversation_ref.as_deref() == Some(conversation_ref)
                    && excluded_invocation_ref != Some(record.invocation_ref.as_str())
                    && record.parent_invocation_ref.is_none()
                    && matches!(record.origin, InvocationOrigin::Ingress { .. })
                    && (!record.state.is_terminal()
                        || record.state == ActionInvocationState::Stopping)
            })
            .collect::<Vec<_>>();
        let mut active = roots
            .iter()
            .filter(|record| {
                matches!(
                    record.state,
                    ActionInvocationState::Running
                        | ActionInvocationState::Waiting
                        | ActionInvocationState::AwaitingPerson
                        | ActionInvocationState::Stopping
                )
            })
            .copied()
            .collect::<Vec<_>>();
        sort_stop_candidates(&mut active);
        if !active.is_empty() {
            return Ok(stop_selection(active));
        }
        let mut admitted = roots
            .into_iter()
            .filter(|record| record.state == ActionInvocationState::Admitted)
            .collect::<Vec<_>>();
        sort_stop_candidates(&mut admitted);
        Ok(stop_selection(admitted))
    }

    /// Applies the conservative shared stop authority rule used by interactive
    /// hosts: the target's requester or target may stop it, and the requester of
    /// its ingress root may stop descendants in the same invocation tree. The
    /// parent walk intentionally crosses detached edges; attachment controls
    /// propagation, not the initiating requester's authority over exact targets.
    pub fn is_actor_authorized_to_stop_invocation(
        &self,
        invocation_ref: &str,
        actor_ref: &str,
    ) -> CwrResult<bool> {
        CwrRef::parse(actor_ref).map_err(|error| {
            invalid_error(format!("invalid stop-requesting actor ref: {error}"))
        })?;
        let target = self.invocation_record(invocation_ref).ok_or_else(|| {
            invalid_error(format!("unknown stop target invocation {invocation_ref}"))
        })?;
        if target.requester_actor_ref == actor_ref || target.target_actor_ref == actor_ref {
            return Ok(true);
        }

        let mut current = target;
        let mut visited = BTreeSet::new();
        loop {
            if !visited.insert(current.invocation_ref.as_str()) {
                return invalid(format!(
                    "invocation parent cycle contains {}",
                    current.invocation_ref
                ));
            }
            match &current.origin {
                InvocationOrigin::Ingress { .. } => {
                    return Ok(current.requester_actor_ref == actor_ref)
                }
                InvocationOrigin::HostCall { .. } => {
                    let parent_ref = current.parent_invocation_ref.as_deref().ok_or_else(|| {
                        invalid_error(format!(
                            "child invocation {} has no parent",
                            current.invocation_ref
                        ))
                    })?;
                    current = self.invocation_record(parent_ref).ok_or_else(|| {
                        invalid_error(format!(
                            "invocation {} has unknown parent {parent_ref}",
                            current.invocation_ref
                        ))
                    })?;
                }
            }
        }
    }

    /// Resolves a delayed provider control against the work present when it was
    /// clicked. Keep terminal candidates that finished after the click: stopping
    /// them is too late, not permission to select a newer request instead.
    pub fn select_invocation_for_stop_at(
        &self,
        conversation_ref: &str,
        at_unix_ms: u64,
    ) -> CwrResult<InvocationStopSelection> {
        let mut candidates = self
            .invocation_records()
            .filter(|record| {
                record.conversation_ref.as_deref() == Some(conversation_ref)
                    && record.parent_invocation_ref.is_none()
                    && matches!(record.origin, InvocationOrigin::Ingress { .. })
                    && record.admitted_at_unix_ms <= at_unix_ms
                    && record
                        .finished_at_unix_ms
                        .is_none_or(|finished| finished >= at_unix_ms)
            })
            .collect::<Vec<_>>();
        let active = candidates
            .iter()
            .copied()
            .filter(|record| {
                record
                    .started_at_unix_ms
                    .is_some_and(|started| started <= at_unix_ms)
            })
            .collect::<Vec<_>>();
        if !active.is_empty() {
            candidates = active;
        }
        sort_stop_candidates(&mut candidates);
        Ok(stop_selection(candidates))
    }

    pub fn runnable_invocation_refs(&self) -> Vec<String> {
        self.state.invocation_ledger.runnable_invocation_refs()
    }

    pub fn admission_record_for_invocation(
        &self,
        invocation_ref: &str,
    ) -> Option<&ConversationAdmissionRecord> {
        self.state
            .admissions
            .iter()
            .find(|record| record.invocation_ref.as_deref() == Some(invocation_ref))
    }

    pub fn execution_conversation_ref(&self, invocation_ref: &str) -> Option<&str> {
        self.state
            .invocation_execution_conversation_refs
            .get(invocation_ref)
            .map(String::as_str)
            .or_else(|| {
                self.admission_record_for_invocation(invocation_ref)
                    .and_then(|record| record.execution_conversation_ref.as_deref())
            })
    }
}
fn sort_stop_candidates(candidates: &mut [&ActionInvocationRecord]) {
    candidates.sort_by(|left, right| {
        left.admitted_at_unix_ms
            .cmp(&right.admitted_at_unix_ms)
            .then_with(|| left.invocation_ref.cmp(&right.invocation_ref))
    });
}

fn stop_selection(mut candidates: Vec<&ActionInvocationRecord>) -> InvocationStopSelection {
    match candidates.len() {
        0 => InvocationStopSelection::None,
        1 => InvocationStopSelection::Selected(candidates.pop().unwrap().invocation_ref.clone()),
        _ => InvocationStopSelection::Ambiguous(
            candidates
                .into_iter()
                .map(|candidate| candidate.invocation_ref.clone())
                .collect(),
        ),
    }
}
