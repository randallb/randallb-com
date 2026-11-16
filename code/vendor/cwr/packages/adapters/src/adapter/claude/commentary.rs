impl<O: Write> Session<'_, O> {
    fn observe_commentary(&mut self, event: &Value) -> CwrResult<()> {
        if self.invocation.model_run_ref.is_none()
            || self.session_id.is_none()
            || event["session_id"].as_str() != self.session_id.as_deref()
        {
            return Ok(());
        }
        let Some(content) = event["message"]["content"].as_array() else {
            return Ok(());
        };
        let Some(id) = event["message"]["id"]
            .as_str()
            .filter(|id| !id.trim().is_empty())
        else {
            return Ok(());
        };
        let text = content
            .iter()
            .filter(|block| block["type"] == "text")
            .filter_map(|block| block["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        if self
            .pending_commentary
            .as_ref()
            .is_some_and(|(prior, _)| prior != id)
        {
            self.flush_commentary()?;
        }
        if !text.trim().is_empty() {
            match &mut self.pending_commentary {
                Some((_, previous)) if *previous != text => {
                    previous.push_str("\n\n");
                    previous.push_str(&text);
                }
                None => self.pending_commentary = Some((id.into(), text)),
                _ => {}
            }
        }
        // The CLI may emit text and tool blocks as separate assistant events.
        // Tool telemetry establishes that the earlier text is an interim update.
        if content.iter().any(|block| block["type"] == "tool_use") {
            self.flush_commentary()?;
        }
        Ok(())
    }

    fn flush_commentary(&mut self) -> CwrResult<()> {
        let Some((id, text)) = self.pending_commentary.take() else {
            return Ok(());
        };
        let Some(model_run_ref) = self.invocation.model_run_ref.clone() else {
            return Ok(());
        };
        self.present_text(
            &id,
            &text,
            cwr::open_responses::v2026_04_24::AssistantPhase::Commentary,
        )?;
        self.publish_activity(AdapterActivityEvent::CompletedCommentary {
            commentary: crate::adapter::AdapterCompletedCommentary {
                model_run_ref,
                provider: "claude".into(),
                thread_id: self.session_id.clone(),
                response_or_turn_id: id.clone(),
                item_id: id,
                text,
            },
        })
    }
}
