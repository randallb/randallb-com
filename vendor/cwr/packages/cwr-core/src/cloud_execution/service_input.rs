use cwpdb_store::documents::{
    DocumentStorage, Mutation as DocumentMutation, Operation as DocumentOperation, Validator,
};
use sha2::{Digest, Sha256};
struct InputAuthority<'a> {
    runtime: &'a LocalRuntime,
    context: Context,
    invocation_ref: String,
    reference: String,
}
impl Authorizer for InputAuthority<'_> {
    fn authorize(&self, context: &Context, reference: &str, _write: bool) -> CwpdbResult<()> {
        if context != &self.context || reference != self.reference {
            return Err("Transport input authority mismatch".into());
        }
        self.runtime
            .cloud_execution_record(&self.invocation_ref, true)?;
        Ok(())
    }
}
struct ImmutableInput;
impl Validator for ImmutableInput {
    fn validate(
        &self,
        schema: &str,
        previous: Option<&serde_json::Map<String, Value>>,
        next: &serde_json::Map<String, Value>,
    ) -> CwpdbResult<()> {
        if schema != "cwp://org.coworkerprotocol/cwr/cloud/transport-input/v1"
            || previous.is_some_and(|old| old != next)
        {
            return Err("Cloud input is immutable".into());
        }
        Ok(())
    }
}
impl CloudHttpService {
    fn input(
        &self,
        reference: &str,
        assignment: &CloudAssignment,
        body: Value,
    ) -> CwrResult<CloudHttpResponse> {
        let request_id = body["request_id"]
            .as_str()
            .filter(|id| !id.trim().is_empty())
            .ok_or("Transport input request identity required")?;
        let frame: crate::adapter::AdapterHostMessage =
            serde_json::from_value(body["frame"].clone())?;
        match &frame {
            crate::adapter::AdapterHostMessage::CallResult { invocation_ref, .. }
            | crate::adapter::AdapterHostMessage::ActivityResult { invocation_ref, .. }
                if invocation_ref == reference => {}
            _ => return Err("Transport input differs from the assigned invocation".into()),
        }
        let hash = hex::encode(Sha256::digest(serde_json::to_vec(&json!([
            self.config.workspace_ref,
            reference,
            request_id
        ]))?));
        let document_ref = format!("{}/cloud-inputs/{hash}", self.runtime_ref);
        let authority = InputAuthority {
            runtime: &self.runtime,
            context: self.context(),
            invocation_ref: reference.into(),
            reference: document_ref.clone(),
        };
        let value=json!({"invocation_ref":reference,"assignment_ref":assignment.assignment_id,"request_id":request_id,"frame":frame}).as_object().ok_or("Invalid cloud input")?.clone();
        {
            let mut coordinator = self.lock()?;
            if let Some(previous) =
                coordinator
                    .storage
                    .get_document(&self.context(), &document_ref, &authority)?
            {
                if previous.value != value {
                    return Err("Cloud input request identity conflict".into());
                }
            } else {
                coordinator.storage.mutate_document(
                    &DocumentMutation {
                        context: self.context(),
                        reference: document_ref.clone(),
                        request_id: format!("cloud-input:{hash}"),
                        expected_revision: None,
                        operation: DocumentOperation::Create {
                            schema_ref: "cwp://org.coworkerprotocol/cwr/cloud/transport-input/v1"
                                .into(),
                            value,
                        },
                    },
                    &authority,
                    &ImmutableInput,
                )?;
            }
        }
        // The same live executor deduplicates the same transport input. Lost
        // executors are interrupted; this path never starts a replacement.
        let accepted = self.executor(assignment, "input", Some(body), None)?;
        Ok(CloudHttpResponse::ok(accepted))
    }
}
