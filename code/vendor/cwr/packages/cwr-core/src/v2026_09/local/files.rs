// File contents are fetched on demand and never retained as host-call activity.
const READ_FILE_HOST_OPERATION: &str = "cwr.read_file";

fn read_file_operation() -> AdapterHostOperation {
    AdapterHostOperation {
        name: READ_FILE_HOST_OPERATION.into(),
        description: "Read an attached image using its file_ref from a conversation message. Use this to inspect images: an attachment reference alone does not contain image content. Access is checked for the current coworker.".into(),
        input_schema: json!({"type":"object","additionalProperties":false,
            "required":["file_ref"],"properties":{"file_ref":{"type":"string","minLength":1}}}),
        output_schema: json!({"type":"object"}),
    }
}

impl LocalRuntime {
    fn handle_read_file(
        &self,
        call: &AdapterCall,
        caller: &ActionInvocationRecord,
    ) -> CwrResult<AdapterHostCallOutcome> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            file_ref: String,
        }
        let input: Input = serde_json::from_value(call.input.clone())?;
        let state = self.lock()?;
        require_active_caller(&state.admission, call)?;
        let metadata = state
            .admission
            .file_metadata(&caller.target_actor_ref, &input.file_ref)?;
        if !matches!(
            metadata.media_type.as_str(),
            "image/png" | "image/jpeg" | "image/webp" | "image/gif"
        ) {
            return invalid("Image inspection supports PNG, JPEG, WebP, and GIF files");
        }
        let file = state
            .admission
            .read_file(&caller.target_actor_ref, &input.file_ref)?;
        Ok(AdapterHostCallOutcome::Returned(serde_json::to_value(
            file,
        )?))
    }
}


impl LocalRuntime {
    /// Retain a result image for the admitted request chain, without caller-selected grants.
    pub fn ingest_invocation_file_as(
        &self,
        actor_ref: &str,
        invocation_ref: &str,
        filename: &str,
        media_type: &str,
        bytes: &[u8],
    ) -> CwrResult<crate::v2026_09::conversation::RetainedFile> {
        let mut state = self.lock()?;
        if state.shutting_down {
            return invalid("local runtime is shutting down");
        }
        require_no_stop_fence(&state, invocation_ref)?;
        let invocation = state.admission.invocation_record(invocation_ref)
            .ok_or_else(|| invalid_error("Image invocation unavailable"))?;
        if invocation.requester_actor_ref != actor_ref
            || invocation.state != ActionInvocationState::Running
        {
            return invalid("Image invocation unavailable");
        }
        let mut readers = Vec::new();
        let mut visited = BTreeSet::new();
        let mut current = Some(invocation_ref.to_owned());
        while let Some(reference) = current {
            if !visited.insert(reference.clone()) {
                return invalid("Image invocation ancestry is invalid");
            }
            require_no_stop_fence(&state, &reference)?;
            let record = state.admission.invocation_record(&reference)
                .ok_or_else(|| invalid_error("Image invocation ancestry is unavailable"))?;
            readers.push(record.requester_actor_ref.clone());
            current = record.parent_invocation_ref.clone();
        }
        state.admission.ingest_file_for_readers(actor_ref, readers, filename, media_type, bytes)
    }
}
