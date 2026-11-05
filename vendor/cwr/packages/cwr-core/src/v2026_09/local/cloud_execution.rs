impl LocalRuntime {
    /// Inspect current normal CWR authority for a trusted execution host. Public
    /// HTTP authentication is separate and cannot be replaced by input fields.
    pub fn cloud_execution_record(
        &self,
        invocation_ref: &str,
        active: bool,
    ) -> CwrResult<ActionInvocationRecord> {
        let state = self.lock()?;
        let record = state
            .admission
            .invocation_record(invocation_ref)
            .ok_or("Unknown cloud invocation")?;
        state
            .admission
            .check_context_execution(&state.graph, record)?;
        require_current_request_authority(&state, record)?;
        if active {
            require_no_stop_fence(&state, invocation_ref)?;
            if record.state != ActionInvocationState::Running || !record.attempt_active {
                return Err("Cloud invocation has no active normal CWR claim".into());
            }
        }
        Ok(record.clone())
    }

    /// Fresh delivery authority, including retained successful terminal output.
    /// Cleanup may inspect stopped work, but stopped work cannot publish frames.
    pub fn cloud_execution_delivery_record(
        &self,
        invocation_ref: &str,
    ) -> CwrResult<ActionInvocationRecord> {
        let state = self.lock()?;
        let record = state
            .admission
            .invocation_record(invocation_ref)
            .ok_or("Unknown cloud invocation")?;
        state
            .admission
            .check_context_execution(&state.graph, record)?;
        require_current_request_authority(&state, record)?;
        require_no_stop_fence(&state, invocation_ref)?;
        let active = record.state == ActionInvocationState::Running && record.attempt_active;
        let successful = record.state == ActionInvocationState::Completed
            && record
                .terminal_response()
                .is_some_and(|response| response.status.is_completed());
        if record.stop_requested_at_unix_ms.is_some() || !(active || successful) {
            return Err(
                "Cloud event delivery requires active or successfully completed normal CWR work"
                    .into(),
            );
        }
        Ok(record.clone())
    }

    /// Require the cloud proxy's request to match the actual admitted route and
    /// retained source context. The proxy cannot invent an action or authority.
    pub fn validate_cloud_dispatch(
        &self,
        dispatch: &crate::cloud_execution::CloudDispatch,
    ) -> CwrResult<ActionInvocationRecord> {
        self.cloud_execution_record(&dispatch.invocation.invocation_ref, true)?;
        let state = self.lock()?;
        let record = state
            .admission
            .invocation_record(&dispatch.invocation.invocation_ref)
            .ok_or("Unknown cloud invocation")?;
        let mut retained = state
            .admission
            .retained_cloud_execution_input(&record.invocation_ref)?;
        let route = selected_process_route(&state.graph, record, &mut retained)?;
        if route.adapter_ref() != dispatch.adapter_ref
            || route.fingerprint() != dispatch.route_fingerprint
            || record.conversation_ref.as_deref() != Some(dispatch.conversation_ref.as_str())
            || dispatch.request_id != record.invocation_ref
        {
            return Err("Cloud request differs from the selected CWR route".into());
        }
        retained.route_fingerprint = None;
        retained.provider_fingerprint = None;
        retained.retained_route = None;
        retained.local_inputs = None;
        // The ordinary host allocates a model run at claim time. This transient
        // correlation does not change action, actor, route, grants or context.
        retained.model_run_ref = dispatch.invocation.model_run_ref.clone();
        if retained != dispatch.invocation {
            return Err("Cloud input differs from retained CWR execution context".into());
        }
        Ok(record.clone())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CloudHostAuthorityPurpose {
    Work,
    Result,
}

impl LocalRuntime {
    /// Resolve source authority from retained native ancestry, not model input.
    /// Ongoing host triggers require a separate registered authority path.
    pub fn authorize_cloud_host_work(
        &self,
        invocation_ref: &str,
        expected_person: &str,
        expected_conversation: &str,
    ) -> CwrResult<Value> {
        self.authorize_cloud_host_authority(
            invocation_ref,
            expected_person,
            expected_conversation,
            CloudHostAuthorityPurpose::Work,
        )
    }

    /// Read-only publication/replay authority for an already successful native
    /// child. This cannot authorize new execution or provider credential use.
    pub fn authorize_cloud_host_result(
        &self,
        invocation_ref: &str,
        expected_person: &str,
        expected_conversation: &str,
    ) -> CwrResult<Value> {
        self.authorize_cloud_host_authority(
            invocation_ref,
            expected_person,
            expected_conversation,
            CloudHostAuthorityPurpose::Result,
        )
    }

    fn authorize_cloud_host_authority(
        &self,
        invocation_ref: &str,
        expected_person: &str,
        expected_conversation: &str,
        purpose: CloudHostAuthorityPurpose,
    ) -> CwrResult<Value> {
        let state = self.lock()?;
        let child = state
            .admission
            .invocation_record(invocation_ref)
            .ok_or("Unknown native host work")?;
        let successful = |record: &ActionInvocationRecord| {
            record.state == ActionInvocationState::Completed
                && record
                    .terminal_response()
                    .is_some_and(|response| response.status.as_u16() == 200)
        };
        if purpose == CloudHostAuthorityPurpose::Result && !successful(child) {
            return Err("Native host result requires a successful completed child".into());
        }
        let root_conversation = state
            .admission
            .conversation(expected_conversation)
            .ok_or("Unknown customer conversation")?;
        let expected_owner = root_conversation.owner_context_ref.as_deref();
        let mut current = child;
        let mut seen = BTreeSet::new();
        loop {
            if !seen.insert(current.invocation_ref.clone())
                || seen.len() > 64
                || match purpose {
                    CloudHostAuthorityPurpose::Work => current.state.is_terminal(),
                    CloudHostAuthorityPurpose::Result => {
                        !(matches!(
                            current.state,
                            ActionInvocationState::Admitted | ActionInvocationState::Running
                        ) || successful(current))
                    }
                }
                || current.state == ActionInvocationState::Stopping
            {
                return Err("Native host ancestry is stopped or invalid".into());
            }
            require_no_stop_fence(&state, &current.invocation_ref)?;
            state
                .admission
                .check_context_execution(&state.graph, current)?;
            require_current_request_authority(&state, current)?;
            let conversation = current
                .conversation_ref
                .as_deref()
                .and_then(|reference| state.admission.conversation(reference))
                .ok_or("Missing native work conversation")?;
            if conversation.owner_context_ref.as_deref() != expected_owner {
                return Err("Native work crossed customer owner context".into());
            }
            match current.parent_invocation_ref.as_deref() {
                Some(parent) => {
                    current = state
                        .admission
                        .invocation_record(parent)
                        .ok_or("Missing native ancestor")?
                }
                None => break,
            }
        }
        if current.requester_actor_ref != expected_person
            || current.conversation_ref.as_deref() != Some(expected_conversation)
            || !matches!(current.origin, InvocationOrigin::Ingress { .. })
        {
            return Err("Native work does not originate from the selected customer request".into());
        }
        let source = current
            .source_message_ref
            .clone()
            .or_else(|| {
                state
                    .admission
                    .admissions()
                    .find(|admission| {
                        admission.invocation_ref.as_deref() == Some(current.invocation_ref.as_str())
                    })
                    .map(|admission| admission.source_message_ref.clone())
            })
            .ok_or("Missing original customer message")?;
        let (origin_integration_ref, provider_event_ref) = match &current.origin {
            InvocationOrigin::Ingress {
                integration_ref,
                provider_event_ref,
            } => (integration_ref, provider_event_ref),
            _ => unreachable!("ingress origin checked above"),
        };
        Ok(
            json!({"authorized":true,"source_message_ref":source,"root_invocation_ref":current.invocation_ref,
            "root_requester_actor_ref":current.requester_actor_ref,"root_conversation_ref":current.conversation_ref,
            "origin_integration_ref":origin_integration_ref,"provider_event_ref":provider_event_ref,
            "target_actor_ref":child.target_actor_ref,"action_name":child.action_name,"actor_integration_ref":child.actor_integration_ref,
            "child_state":child.state,"child_status":child.terminal_response().map(|response|response.status.as_u16()),
            "root_state":current.state,"root_status":current.terminal_response().map(|response|response.status.as_u16())}),
        )
    }
}
