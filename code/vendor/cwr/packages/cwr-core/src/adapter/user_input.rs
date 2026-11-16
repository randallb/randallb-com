//! Questions held by a live provider request, not a resumable provider turn.
use crate::CwrResult;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub const USER_INPUT_OPERATION: &str = "cwr.provider_user_input";
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UserInputQuestion {
    pub id: String,
    pub header: String,
    pub question: String,
    #[serde(default)]
    pub is_other: bool,
    #[serde(default)]
    pub is_secret: bool,
    pub options: Option<Vec<UserInputOption>>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserInputOption {
    pub label: String,
    pub description: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInputAnswer {
    pub answers: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInputResponse {
    pub answers: BTreeMap<String, UserInputAnswer>,
}

pub fn validate_questions(value: &Value) -> CwrResult<Vec<UserInputQuestion>> {
    let questions: Vec<UserInputQuestion> = serde_json::from_value(value.clone())?;
    let mut ids = BTreeSet::new();
    if questions.is_empty() || questions.len() > 3 {
        return Err("expected one to three questions".into());
    }
    for q in &questions {
        if q.id.trim().is_empty() || q.question.trim().is_empty() || !ids.insert(&q.id) {
            return Err("questions require unique nonempty IDs and text".into());
        }
    }
    Ok(questions)
}
pub fn validate_answers(
    questions: &[UserInputQuestion],
    text: &str,
) -> CwrResult<UserInputResponse> {
    let response: UserInputResponse = serde_json::from_str(text)?;
    if response.answers.len() != questions.len() {
        return Err("answer every question".into());
    }
    for q in questions {
        let answer = response
            .answers
            .get(&q.id)
            .ok_or("missing question answer")?;
        if answer.answers.len() != 1 || answer.answers[0].trim().is_empty() {
            return Err("provide one answer per question".into());
        }
        if !q.is_other
            && q.options.as_ref().is_some_and(|options| {
                !options.is_empty() && !options.iter().any(|o| o.label == answer.answers[0])
            })
        {
            return Err("answer must be one of the question's choices".into());
        }
    }
    Ok(response)
}

pub fn host_operation() -> super::AdapterHostOperation {
    super::AdapterHostOperation {
        name: USER_INPUT_OPERATION.into(),
        description: "Bridge a live provider question to the requesting person.".into(),
        input_schema: serde_json::json!({"type":"object","required":["key","action"],"properties":{"key":{"type":"string","minLength":1},"action":{"enum":["start","poll","cancel"]},"questions":{}},"additionalProperties":false}),
        output_schema: serde_json::json!({"type":"object","required":["state"],"properties":{"state":{"enum":["pending","answered","resolved"]},"response":{}},"additionalProperties":false}),
    }
}
