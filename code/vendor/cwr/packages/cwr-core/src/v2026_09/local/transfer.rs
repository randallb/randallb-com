impl LocalRuntime {
    pub fn fork_conversation_as(
        &self,
        conversation: Option<&str>,
        caller: &str,
        message_ref: &str,
        fork_id: &str,
    ) -> CwrResult<IntrinsicCwrResponse> {
        self.fork_conversation_at_as(conversation, caller, message_ref, fork_id, None)
    }
    pub fn fork_conversation_at_as(&self, conversation: Option<&str>, caller: &str, message_ref: &str, fork_id: &str, timeline_item_ref: Option<&str>) -> CwrResult<IntrinsicCwrResponse> {
        let conversation_ref = conversation
            .ok_or_else(|| invalid_error("Fork requires a conversation"))?;
        let mut state = self.lock()?;
        if state.shutting_down {
            return invalid("Runtime is shutting down");
        }
        let graph = state.graph.clone();
        let fork = state.admission.fork_conversation_at(
            &graph,
            caller,
            conversation_ref,
            message_ref,
            fork_id,
            timeline_item_ref,
        )?;
        Ok(IntrinsicCwrResponse::ConversationForked {
            conversation_ref: fork.conversation_ref,
            source_conversation_ref: conversation_ref.into(),
            source_message_ref: message_ref.into(),
        })
    }

    pub fn export_conversation_as(&self, conversation: Option<&str>, caller: &str) -> CwrResult<IntrinsicCwrResponse> {
        let conversation = conversation.ok_or_else(|| invalid_error("Export requires a conversation"))?;
        let export = self.lock()?.admission.export_conversation(conversation, caller)?;
        Ok(IntrinsicCwrResponse::ConversationExported { export: Box::new(export) })
    }

    pub fn import_conversation_as(&self, caller: &str, path: &str, import_id: Option<&str>) -> CwrResult<IntrinsicCwrResponse> {
        use std::io::Read;
        const LIMIT: u64 = 64 * 1024 * 1024;
        if let Some(import_id) = import_id {
            let state = self.lock()?;
            if state.shutting_down { return invalid("Runtime is shutting down"); }
            if let Some(conversation) = state.admission.imported_conversation(caller, import_id)? {
                return Ok(IntrinsicCwrResponse::ConversationImported { conversation_ref: conversation.conversation_ref });
            }
        }
        if !Path::new(path).is_absolute() {
            return invalid("Import requires an absolute JSON file path");
        }
        let mut bytes = Vec::new();
        std::fs::File::open(path)?.take(LIMIT + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > LIMIT { return invalid("Conversation export exceeds 64 MiB"); }
        let export = serde_json::from_slice(&bytes)?;
        let mut state = self.lock()?;
        if state.shutting_down { return invalid("Runtime is shutting down"); }
        let graph = state.graph.clone();
        let conversation = state.admission.import_conversation(&graph, caller, export, import_id)?;
        Ok(IntrinsicCwrResponse::ConversationImported { conversation_ref: conversation.conversation_ref })
    }
}
