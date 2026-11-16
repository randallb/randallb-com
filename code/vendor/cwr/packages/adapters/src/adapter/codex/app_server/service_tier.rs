// Host preference is read at the new-turn boundary, never while steering a turn.
const MODEL_LIST_ID: &str = "cwr-service-tier-models";
#[derive(Default)]
struct TurnServiceTier {
    model: Option<String>,
    tier: Option<&'static str>,
    cursors: std::collections::BTreeSet<String>,
}
impl TurnServiceTier {
    fn prepare(
        &mut self,
        writer: &mut impl Write,
        config: &BoundedActionConfig,
        message: &Value,
        context_ready: bool,
    ) -> CwrResult<bool> {
        if context_ready {
            let Some(path) = &config.service_tier_file else { return Ok(true); };
            // Missing or damaged preferences cannot silently opt into extra usage.
            self.tier = Some("default");
            let fast = std::fs::read(path).ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .is_some_and(|value| value["service_tier"] == "fast");
            if !fast || self.model.is_none() { return Ok(true); }
            self.request_models(writer, None)?;
            return Ok(false);
        }
        if message["id"].as_str() != Some(MODEL_LIST_ID) { return Ok(false); }
        // A catalog error or an unknown model keeps this turn at standard speed.
        if let Some(models) = message.pointer("/result/data").and_then(Value::as_array) {
            if let Some(model) = models.iter().find(|model|
                model["model"].as_str() == self.model.as_deref()) {
                if model["serviceTiers"].as_array().is_some_and(|tiers|
                    // Codex catalogs also expose Fast mode under its API name, priority.
                    tiers.iter().any(|tier| matches!(tier["id"].as_str(), Some("fast" | "priority")))) {
                    self.tier = Some("fast");
                }
                return Ok(true);
            }
            if let Some(cursor) = message.pointer("/result/nextCursor").and_then(Value::as_str) {
                if self.cursors.len() < 16 && self.cursors.insert(cursor.to_owned()) {
                    self.request_models(writer, Some(cursor))?;
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
    fn request_models(&self, writer: &mut impl Write, cursor: Option<&str>) -> CwrResult<()> {
        write_json_line(writer, &json!({"id":MODEL_LIST_ID, "method":"model/list",
            "params":{"includeHidden":true, "cursor":cursor}}))
    }
    fn start_turn(&self, writer: &mut impl Write, thread_id: &Option<String>,
        input: Value) -> CwrResult<()> {
        let mut params = json!({"threadId":thread_id, "input":input});
        if let Some(tier) = self.tier {
            params["serviceTierForTurn"] = json!(tier);
        }
        write_json_line(writer, &json!({"id":TURN_START_ID, "method":"turn/start", "params":params}))
    }
}
