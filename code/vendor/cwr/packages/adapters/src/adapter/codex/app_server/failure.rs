/// Failures that can be reported through the adapter result protocol.
#[derive(Debug)]
pub(crate) struct CodexFailure {
    pub(crate) status: u16,
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) details: Option<Value>,
}

impl CodexFailure {
    pub(crate) fn result(&self, invocation_ref: String, sequence: u64) -> crate::adapter::AdapterEvent {
        crate::adapter::AdapterEvent::Result {
            invocation_ref,
            sequence,
            status: self.status,
            summary: self.message.clone(),
            result: None,
            error: Some(crate::adapter::AdapterError {
                code: self.code.into(),
                message: Some(self.message.clone()),
                details: self.details.clone(),
            }),
            continuation: None,
        }
    }

    fn frame_too_large() -> Self {
        Self {
            status: 500,
            code: "provider_frame_too_large",
            message: "Codex returned a response larger than Workloop's 64 MiB limit, so this turn could not finish.".into(),
            details: None,
        }
    }

    fn unresponsive(timeout: Duration) -> Self {
        Self {
            status: 504,
            code: "provider_timeout",
            message: "Codex stopped responding. Check your connection or VPN, then try again."
                .into(),
            details: Some(json!({
                "inactivity_timeout_ms": u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX)
            })),
        }
    }

    fn provider(context: &str, error: &Value) -> Self {
        // Use the provider's message, not its entire response or auxiliary data.
        let detail = error.get("message").and_then(Value::as_str)
            .or_else(|| error.as_str()).unwrap_or("").trim();
        let message = if detail.is_empty() {
            format!("{context}.")
        } else {
            let mut excerpt: String = detail.chars().take(2048).collect();
            if excerpt.len() < detail.len() {
                excerpt.push('…');
            }
            format!("{context}: {excerpt}")
        };
        Self {
            status: 500,
            code: "codex_execution_failed",
            message,
            details: None,
        }
    }

    pub(crate) fn from_error<'a>(error: &'a (dyn std::error::Error + 'static)) -> Option<&'a Self> {
        error.downcast_ref::<Self>().or_else(|| {
            error.downcast_ref::<io::Error>()?.get_ref()?.downcast_ref::<Self>()
        })
    }
}

impl std::fmt::Display for CodexFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CodexFailure {}
