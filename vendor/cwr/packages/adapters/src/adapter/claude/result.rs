impl<O: Write> Session<'_, O> {
    fn receive_result(&mut self, event: Value) -> CwrResult<()> {
        let final_item_id = self
            .pending_commentary
            .as_ref()
            .filter(|(_, text)| event["result"].as_str() == Some(text.as_str()))
            .map(|(id, _)| id.clone())
            .unwrap_or_else(|| format!("cwr-final:{}", self.invocation.invocation_ref));
        if self
            .pending_commentary
            .as_ref()
            .is_some_and(|(_, text)| event["result"].as_str() == Some(text.as_str()))
        {
            self.pending_commentary = None;
        } else {
            self.flush_commentary()?;
        }
        if self.returned.is_some() {
            return Err(error("Claude returned more than one terminal result"));
        }
        if event["subtype"] != "success" || event["is_error"] != false {
            // Keep stdin open until outstanding host acknowledgements arrive,
            // then report a failed terminal instead of losing the error to EPIPE.
            self.provider_failed = true;
            self.returned = Some(failure::provider_result(&event));
            return Ok(());
        }
        if self.session_id.as_deref() != event["session_id"].as_str() || self.session_id.is_none() {
            return Err(error("Claude result has no matching initialized session"));
        }
        for field in ["permission_denials", "mcp_server_errors"] {
            if event.get(field).is_some_and(|value| {
                !value.is_null() && value.as_array().is_none_or(|items| !items.is_empty())
            }) {
                return Err(error(format!("Claude reported {field}")));
            }
        }
        let returned = if self.config.native_text {
            let text = event["result"]
                .as_str()
                .filter(|text| !text.trim().is_empty())
                .ok_or_else(|| error("Claude returned no assistant text"))?;
            json!({"status":200,"summary":"Returned a response.","result":{"text":text}})
        } else {
            event
                .get("structured_output")
                .cloned()
                .ok_or_else(|| error("Claude returned no structured action result"))?
        };
        if !jsonschema::is_valid(&self.config.return_schema, &returned) {
            return Err(error(
                "Claude result does not match the action's return schema",
            ));
        }
        self.finish_compaction()?;
        if self.config.native_text {
            self.present_text(
                &final_item_id,
                returned["result"]["text"].as_str().expect("validated text"),
                cwr::open_responses::v2026_04_24::AssistantPhase::FinalAnswer,
            )?;
        }
        self.returned = Some(returned);
        if let Some(working_ref) = &self.config.working_ref {
            let mut data = json!({"version":1,"provider":"claude","idle":true,
                "working_ref":working_ref,"thread_id":self.session_id,
                "invocation_ref":self.invocation.invocation_ref,
                "context_snapshot":self.config.working_snapshot});
            if let Some(store) = self.config.provider_store {
                data["provider_store"] = json!(store);
            }
            self.publish_activity(AdapterActivityEvent::ProviderEvent {
                provider: "claude".into(),
                method: "cwr.working_state".into(),
                data,
            })?;
        }
        Ok(())
    }
}
