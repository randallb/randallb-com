#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloudModelGrant {
    pub model: String,
    pub endpoint: String,
    pub request_budget: u32,
    pub max_output_tokens: u32,
}
struct ModelBudgetValidator;
impl Validator for ModelBudgetValidator {
    fn validate(
        &self,
        schema: &str,
        previous: Option<&serde_json::Map<String, Value>>,
        next: &serde_json::Map<String, Value>,
    ) -> CwpdbResult<()> {
        if schema != "cwp://org.coworkerprotocol/cwr/cloud/model-budget/v1" {
            return Err("Wrong model budget schema".into());
        }
        let requests = next
            .get("requests")
            .and_then(Value::as_object)
            .ok_or("Missing model reservations")?;
        let limit = next
            .get("limit")
            .and_then(Value::as_u64)
            .ok_or("Missing model request limit")?;
        if limit == 0 || limit > 64 || requests.len() as u64 > limit {
            return Err("Model request budget exceeded".into());
        }
        if let Some(previous) = previous {
            if ["model", "endpoint", "limit"]
                .iter()
                .any(|key| previous.get(*key) != next.get(*key))
                || previous
                    .get("requests")
                    .and_then(Value::as_object)
                    .ok_or("Missing previous requests")?
                    .iter()
                    .any(|(key, value)| requests.get(key) != Some(value))
            {
                return Err("Retained model reservations are immutable".into());
            }
        }
        Ok(())
    }
}
impl CloudHttpService {
    fn model_operation(
        &self,
        path: &str,
        authorization: &str,
        body: Value,
    ) -> CwrResult<CloudHttpResponse> {
        let id = body["assignment_id"]
            .as_str()
            .ok_or("Assignment identity required")?;
        let (reference, record, assignment) = self.credentials(id, authorization, true)?;
        let grant = self
            .config
            .model
            .as_ref()
            .ok_or("No model route or inference budget authorized")?;
        let binding = record.admitted["adapter_invocation"]["integration_ref"]
            .as_str()
            .ok_or("Missing retained model binding")?;
        let graph = self.runtime.graph()?;
        if !graph.actor_integrations.iter().any(|selected| {
            selected.integration_ref == binding
                && selected.adapter_ref == crate::adapter::OPEN_RESPONSES_ADAPTER_REF
        }) || record.admitted["adapter_invocation"]["integration_params"]["endpoint"]
            != grant.endpoint
        {
            return Err("Assignment does not select the authorized model route".into());
        }
        let retained_model = &record.admitted["adapter_invocation"]["adapter_params"]["model"];
        if retained_model != &grant.model {
            return Err("Assignment model differs from authorized route".into());
        }
        if path == "/internal/cloud/authorize-model" {
            return Ok(CloudHttpResponse::ok(
                json!({"workspaceRef":record.workspace_ref,"invocationRef":reference,
                "assignmentRef":format!("{}/assignments/{id}",self.runtime_ref),"personRef":record.admitted["cwr_invocation"]["requester_actor_ref"],"operations":["model.respond"]}),
            ));
        }
        let fingerprint = body["request_fingerprint"]
            .as_str()
            .filter(|value| value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or("Model request fingerprint required")?;
        let document_ref = format!("{}/model-budgets/{id}", self.runtime_ref);
        let authority = InputAuthority {
            runtime: &self.runtime,
            context: self.context(),
            invocation_ref: reference,
            reference: document_ref.clone(),
        };
        let mut coordinator = self.lock()?;
        let previous =
            coordinator
                .storage
                .get_document(&self.context(), &document_ref, &authority)?;
        let mut value=previous.as_ref().map(|record|record.value.clone()).unwrap_or_else(||json!({"model":grant.model,"endpoint":grant.endpoint,"limit":grant.request_budget,"requests":{}}).as_object().expect("budget object").clone());
        let requests = value
            .get_mut("requests")
            .and_then(Value::as_object_mut)
            .ok_or("Missing model request budget")?;
        if requests.contains_key(fingerprint) {
            return Ok(CloudHttpResponse::ok(json!({"reservation":"existing"})));
        }
        if requests.len() >= grant.request_budget as usize {
            return Ok(CloudHttpResponse::ok(json!({"reservation":"exhausted"})));
        }
        requests.insert(
            fingerprint.into(),
            json!({"state":"reserved","automatic_replay":false}),
        );
        let operation = if previous.is_some() {
            DocumentOperation::Replace { value }
        } else {
            DocumentOperation::Create {
                schema_ref: "cwp://org.coworkerprotocol/cwr/cloud/model-budget/v1".into(),
                value,
            }
        };
        coordinator.storage.mutate_document(
            &DocumentMutation {
                context: self.context(),
                reference: document_ref,
                request_id: format!("model-reserve:{}:{fingerprint}", assignment.assignment_id),
                expected_revision: previous.map(|record| record.revision),
                operation,
            },
            &authority,
            &ModelBudgetValidator,
        )?;
        Ok(CloudHttpResponse::ok(json!({"reservation":"reserved"})))
    }
}
