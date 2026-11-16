impl CloudHttpService {
    fn internal(
        &self,
        path: &str,
        authorization: &str,
        body: Value,
    ) -> CwrResult<CloudHttpResponse> {
        if matches!(
            path,
            "/internal/cloud/authorize-host-work" | "/internal/cloud/authorize-host-result"
        ) {
            if !equal_secret(
                authorization,
                &format!("Bearer {}", self.config.coordinator_token),
            ) {
                return Err("Native host credential required".into());
            }
            let reference = body["invocation_ref"]
                .as_str()
                .ok_or("Native invocation required")?;
            let person = body["expected_person_actor_ref"]
                .as_str()
                .ok_or("Expected customer required")?;
            let conversation = body["expected_conversation_ref"]
                .as_str()
                .ok_or("Expected customer conversation required")?;
            return Ok(CloudHttpResponse::ok(
                if path == "/internal/cloud/authorize-host-result" {
                    self.runtime
                        .authorize_cloud_host_result(reference, person, conversation)?
                } else {
                    self.runtime
                        .authorize_cloud_host_work(reference, person, conversation)?
                },
            ));
        }
        if matches!(
            path,
            "/internal/cloud/authorize-model" | "/internal/cloud/reserve-model-request"
        ) {
            return self.model_operation(path, authorization, body);
        }
        let id = body["assignment_id"]
            .as_str()
            .ok_or("Assignment identity required")?;
        let operation = body["operation"].as_str().unwrap_or("");
        let active = path.ends_with("reserve-executor-start")
            || !matches!(operation, "/destroy" | "/cancel");
        let (reference, _, assignment) = self.credentials(id, authorization, active)?;
        if path == "/internal/cloud/authorize-executor" {
            if !matches!(
                operation,
                "/start" | "/events" | "/input" | "/ack" | "/cancel" | "/destroy"
            ) {
                return Err("Unsupported executor operation".into());
            }
            return Ok(CloudHttpResponse::ok(json!({"authorized":true})));
        }
        if path == "/internal/cloud/reserve-executor-start" {
            let boot = body["worker_boot_id"]
                .as_str()
                .ok_or("Worker boot identity required")?;
            self.lock()?.begin_execution(
                self.context(),
                reference,
                &assignment.token,
                boot,
                now(),
            )?;
            return Ok(CloudHttpResponse::ok(json!({"reserved":true})));
        }
        Ok(CloudHttpResponse {
            status: 404,
            body: json!({"error":"not_found"}),
        })
    }
}
