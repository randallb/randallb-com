/// Live RPCs belong to this process/turn only; never put them in continuation.
#[derive(Default)]
struct PendingUserInputs {
    requests: Vec<(Value, String)>,
    sequence: u64,
    last_poll: Option<Instant>,
}
impl PendingUserInputs {
    fn is_waiting(&self) -> bool {
        !self.requests.is_empty()
    }

    fn host<H: BoundedActionHandler>(&mut self, handler: &mut H, key: &str, action: &str, questions: Value) -> CwrResult<Value> {
        self.sequence += 1;
        let result = handler.on_host_call(&format!("user-input-{}", self.sequence),
            cwr::adapter::user_input::USER_INPUT_OPERATION,
            json!({"key":key,"action":action,"questions":questions}))?;
        if !result.success { return invalid(format!("Could not handle Codex question: {}", result.content)); }
        Ok(result.content)
    }
    fn observe<W: Write, H: BoundedActionHandler>(&mut self, message: &Value, scope: Option<&CodexOpenResponsesMapper>, input: &mut W, handler: &mut H) -> CwrResult<bool> {
        if message["method"] == "item/tool/requestUserInput" {
            let id = message.get("id").filter(|id| id.is_string() || id.is_number()).ok_or("question has no request ID")?.clone();
            let params = &message["params"];
            let matches = scope.is_some_and(|s| params["threadId"] == s.thread_id() && params["turnId"] == s.turn_id());
            if !matches || params["itemId"].as_str().is_none_or(|s| s.is_empty()) {
                write_json_line(input, &json!({"id":id,"error":{"code":-32602,"message":"Question does not belong to the active Codex turn"}}))?;
                return Ok(true);
            }
            cwr::adapter::user_input::validate_questions(&params["questions"])?;
            if self.requests.iter().any(|(other, _)| other == &id) { return invalid("duplicate live Codex question request ID"); }
            let key = serde_json::to_string(&json!([params["threadId"],params["turnId"],id]))?;
            let result = self.host(handler, &key, "start", params["questions"].clone())?;
            if result["state"] != "pending" { return invalid("Codex question request ID was already resolved"); }
            self.requests.push((id, key));
            return Ok(true);
        }
        if message["method"] == "serverRequest/resolved" && scope.is_some_and(|s| message["params"]["threadId"] == s.thread_id()) {
            if let Some(index) = self.requests.iter().position(|(id, _)| id == &message["params"]["requestId"]) {
                let (_, key) = self.requests.remove(index);
                self.host(handler, &key, "cancel", Value::Null)?;
            }
        }
        Ok(false)
    }
    fn poll<W: Write, H: BoundedActionHandler>(&mut self, input: &mut W, handler: &mut H) -> CwrResult<()> {
        if self.requests.is_empty() || self.last_poll.is_some_and(|at| at.elapsed() < Duration::from_millis(200)) { return Ok(()); }
        self.last_poll = Some(Instant::now());
        for (id, key) in self.requests.clone() {
            let result = self.host(handler, &key, "poll", Value::Null)?;
            match result["state"].as_str() {
                Some("pending") => continue,
                Some("answered") => write_json_line(input, &json!({"id":id,"result":result["response"]}))?,
                Some("resolved") => write_json_line(input, &json!({"id":id,"error":{"code":-32000,"message":"Question was canceled"}}))?,
                _ => return invalid("invalid question state from host"),
            }
            self.requests.retain(|(other, _)| other != &id);
        }
        Ok(())
    }
}
