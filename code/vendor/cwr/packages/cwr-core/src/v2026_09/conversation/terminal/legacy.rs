impl ConversationAdmissionStore {
    fn complete_legacy(
        &mut self,
        invocation_ref: &str,
        status: ActionResponseStatus,
        result: Value,
        activity: Option<ConversationActivityEvent>,
    ) -> CwrResult<ConversationMessage> {
        if let Some(reply) = existing_legacy_reply(
            &self.state,
            invocation_ref,
            ConversationAdmissionState::Completed,
        )? {
            return Ok(reply);
        }
        let parsed: RespondToMessageResult =
            serde_json::from_value(result.clone()).map_err(|error| {
                invalid_error(format!(
                    "{RESPOND_TO_MESSAGE_ACTION_NAME} result is invalid: {error}"
                ))
            })?;
        if parsed.text.trim().is_empty() {
            return invalid(format!(
                "{RESPOND_TO_MESSAGE_ACTION_NAME} result text must not be empty"
            ));
        }
        required_message(self.publish_legacy(
            invocation_ref,
            parsed.text,
            status,
            Some(result),
            None,
            RootReplyKind::Message,
            activity,
        )?)
    }

    fn complete_action_legacy(
        &mut self,
        invocation_ref: &str,
        status: ActionResponseStatus,
        result: Value,
        text: String,
        activity: Option<ConversationActivityEvent>,
    ) -> CwrResult<ConversationMessage> {
        if let Some(reply) = existing_legacy_reply(
            &self.state,
            invocation_ref,
            ConversationAdmissionState::Completed,
        )? {
            return Ok(reply);
        }
        if text.trim().is_empty() {
            return invalid("conversation action result text must not be empty");
        }
        required_message(self.publish_legacy(
            invocation_ref,
            text,
            status,
            Some(result.clone()),
            None,
            RootReplyKind::CompletedAction(result),
            activity,
        )?)
    }

    fn fail_legacy(
        &mut self,
        invocation_ref: &str,
        summary: String,
        activity: Option<ConversationActivityEvent>,
    ) -> CwrResult<Option<ConversationMessage>> {
        let admission_index = root_admission_index(&self.state, invocation_ref)?;
        match self.state.admissions[admission_index].state {
            ConversationAdmissionState::Failed => {
                return existing_root_reply(&self.state, invocation_ref);
            }
            ConversationAdmissionState::Admitted => {}
            _ => {
                return invalid(format!(
                    "invocation {invocation_ref} is not awaiting completion"
                ));
            }
        }
        if summary.trim().is_empty() {
            return invalid("adapter failure summary must not be empty");
        }
        let reply_kind = legacy_root_non_success_reply_kind(&self.state, invocation_ref)?;
        self.publish_legacy(
            invocation_ref,
            summary.clone(),
            ActionResponseStatus::FAILED,
            None,
            Some(ActionResponseDetail::new("adapter_failure").with_message(summary)),
            reply_kind,
            activity,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Legacy publication must preserve the complete historical response and reply shape."
    )]
    fn publish_legacy(
        &mut self,
        invocation_ref: &str,
        text: String,
        status: ActionResponseStatus,
        result: Option<Value>,
        error: Option<ActionResponseDetail>,
        reply_kind: RootReplyKind,
        activity: Option<ConversationActivityEvent>,
    ) -> CwrResult<Option<ConversationMessage>> {
        self.transact(|staged| {
            let response = ActionResponse {
                progress: None,
                invocation_ref: invocation_ref.into(),
                sequence: 1,
                status,
                summary: text.clone(),
                result,
                error,
                cause: None,
            };
            response
                .validate_fields()
                .map_err(|error| invalid_error(error.to_string()))?;
            let activity = activity.map(|event| (event, current_unix_ms()));
            let message = stage_root_projection(
                staged,
                invocation_ref,
                &response,
                text,
                reply_kind,
                activity,
            )?;

            Ok(message)
        })
    }
}
