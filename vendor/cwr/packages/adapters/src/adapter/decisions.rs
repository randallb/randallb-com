//! One-shot typed decisions through OpenRouter; no conversation or tool loop.
mod grading;
mod validation;
use crate::{adapter::*, CwrResult};
use reqwest::{blocking::Client, redirect::Policy};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    io::{BufRead, Read, Write},
    time::Duration,
};

const MAX_BYTES: u64 = 2 * 1024 * 1024;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Params {
    model: String,
    #[serde(default)]
    rubric_version: Option<String>,
    action_name: String,
    #[serde(default)]
    questions: Option<Value>,
    #[serde(default)]
    contract: Option<String>,
    #[serde(default = "timeout")]
    timeout_seconds: u64,
}
fn timeout() -> u64 {
    60
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Integration {
    endpoint: String,
}

pub fn run_decisions_adapter(mut input: impl BufRead, mut output: impl Write) -> CwrResult<()> {
    let message = read_adapter_message(&mut input)?.ok_or("Missing invocation")?;
    let AdapterHostMessage::Invoke {
        protocol,
        invocation,
    } = message
    else {
        return Err("Expected invocation".into());
    };
    if protocol != PROCESS_ADAPTER_PROTOCOL_REF {
        return Err("Unsupported protocol".into());
    }
    validate_adapter_invocation(&invocation)?;
    write_adapter_message(
        &mut output,
        &AdapterEvent::Progress {
            invocation_ref: invocation.invocation_ref.clone(),
            sequence: 1,
            status: 102,
            summary: "Evaluating grading questions.".into(),
            progress: None,
            published: false,
        },
    )?;
    let (status, result, error) = match evaluate(&invocation) {
        Ok(value) => (200, Some(value), None),
        Err((status, code)) => (
            status,
            None,
            Some(AdapterError {
                code: code.into(),
                message: Some("Decisions request did not produce a valid grade.".into()),
                details: None,
            }),
        ),
    };
    write_adapter_message(
        &mut output,
        &AdapterEvent::Result {
            invocation_ref: invocation.invocation_ref,
            sequence: 2,
            status,
            summary: if status == 200 {
                "Returned typed grading judgments."
            } else {
                "Grading unavailable."
            }
            .into(),
            result,
            error,
            continuation: invocation.continuation,
        },
    )
}

type Failure = (u16, &'static str);
fn evaluate(invocation: &AdapterInvocation) -> Result<Value, Failure> {
    let invalid = (422, "invalid_decisions_input");
    let params: Params =
        serde_json::from_value(super::activated::params(&invocation.adapter_params))
            .map_err(|_| invalid)?;
    let integration: Integration =
        serde_json::from_value(invocation.integration_params.clone()).map_err(|_| invalid)?;
    if params.model.trim().is_empty()
        || params.action_name != invocation.action_name
        || invocation.action_name == "respond_to--message"
        || !(1..=300).contains(&params.timeout_seconds)
    {
        return Err(invalid);
    }
    let grading = match params.contract.as_deref() {
        None => None,
        Some("grader-v1") if params.questions.is_none() && params.rubric_version.is_none() => {
            Some(grading::prepare(&invocation.input).map_err(|_| invalid)?)
        }
        _ => return Err(invalid),
    };
    let (questions, state, rubric_version) = if let Some(grade) = &grading {
        (
            grade.questions.clone(),
            grade.context.clone(),
            grade.rubric["version"].clone(),
        )
    } else {
        let version = params
            .rubric_version
            .filter(|v| !v.trim().is_empty())
            .ok_or(invalid)?;
        let questions = params.questions.ok_or(invalid)?;
        let state = match &invocation.input {
            Value::String(_) | Value::Object(_) | Value::Array(_) => invocation.input.clone(),
            scalar => Value::String(serde_json::to_string(scalar).map_err(|_| invalid)?),
        };
        (questions, state, Value::String(version))
    };
    validation::questions(&questions).map_err(|_| invalid)?;
    let endpoint = decisions_endpoint(&integration.endpoint).map_err(|_| invalid)?;
    let local = decisions_endpoint_is_loopback(&endpoint);
    let key = std::env::var("DECISIONS_API_KEY")
        .ok()
        .filter(|v| !v.trim().is_empty());
    if key.is_none() && !local {
        return Err((401, "decisions_credentials_missing"));
    }
    let request = json!({"model":params.model,"state":state,"questions":questions});
    if serde_json::to_vec(&request).map_err(|_| invalid)?.len() > MAX_BYTES as usize {
        return Err((413, "decisions_input_too_large"));
    }
    let client = Client::builder()
        .redirect(Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(params.timeout_seconds))
        .build()
        .map_err(|_| (502, "decisions_transport_failed"))?;
    let mut builder = client.post(endpoint).json(&request);
    if let Some(key) = key {
        builder = builder.bearer_auth(key);
    }
    let response = builder.send().map_err(|e| {
        if e.is_timeout() {
            (504, "decisions_timeout")
        } else {
            (502, "decisions_transport_failed")
        }
    })?;
    let status = response.status();
    if !status.is_success() {
        // Do not persist provider error bodies: they may echo credentials or customer input.
        return Err((
            if (status.is_client_error() || status.is_server_error()) && status.as_u16() != 499 {
                status.as_u16()
            } else {
                502
            },
            "decisions_provider_failed",
        ));
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| (502, "decisions_response_read_failed"))?;
    if bytes.len() > MAX_BYTES as usize {
        return Err((502, "decisions_response_too_large"));
    }
    let response: Value =
        serde_json::from_slice(&bytes).map_err(|_| (502, "invalid_decisions_response"))?;
    validation::answers(&questions, &response).map_err(|_| (502, "invalid_decisions_response"))?;
    let mut result = json!({
        "model":response["model"], "rubric_version":rubric_version,
        "answers":response["answers"], "usage":response["usage"],
        "provider_request_id":response.get("id").cloned().unwrap_or(Value::Null),
    });
    if let Some(grade) = grading {
        grade
            .apply(&mut result)
            .map_err(|_| (502, "invalid_decisions_response"))?;
    }
    let schema = jsonschema::validator_for(&invocation.output_schema).map_err(|_| invalid)?;
    if !schema.is_valid(&result) {
        return Err((502, "decisions_output_contract_mismatch"));
    }
    Ok(result)
}
