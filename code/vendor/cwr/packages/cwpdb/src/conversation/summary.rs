//! One-conversation summary facts. CWR retains presentation and participant policy.
use super::*;
use serde_json::json;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SummaryCompletion {
    pub invocation_ref: String,
    pub sequence: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SummaryFacts {
    pub conversation_ref: String,
    pub creation_order: i64,
    pub title: Option<String>,
    pub archived: bool,
    pub owner_context_ref: Option<String>,
    pub parent_ref: Option<String>,
    pub sidebar_eligible: bool,
    pub root: bool,
    pub participant_actor_refs: Vec<String>,
    pub last_message_ref: Option<String>,
    pub last_message_preview: Option<String>,
    pub last_message_record: Option<Value>,
    pub completed_work: Vec<SummaryCompletion>,
    pub completed_work_refs: Vec<String>,
    pub revision: String,
}

pub(super) fn finish(mut facts: SummaryFacts) -> CwpdbResult<SummaryFacts> {
    use sha2::{Digest, Sha256};
    let message = facts
        .last_message_record
        .as_ref()
        .map(|record| json!([record.get("text"), record.get("visualizations")]));
    let material = json!([
        facts.conversation_ref,
        facts.creation_order,
        facts.title,
        facts.archived,
        facts.owner_context_ref,
        facts.parent_ref,
        facts.sidebar_eligible,
        facts.root,
        facts.participant_actor_refs,
        facts.last_message_ref,
        facts.last_message_preview,
        facts.completed_work,
        facts.completed_work_refs,
        message
    ]);
    fn sorted(value: Value) -> Value {
        match value {
            Value::Array(values) => Value::Array(values.into_iter().map(sorted).collect()),
            Value::Object(values) => Value::Object(
                values
                    .into_iter()
                    .collect::<std::collections::BTreeMap<_, _>>()
                    .into_iter()
                    .map(|(key, value)| (key, sorted(value)))
                    .collect(),
            ),
            value => value,
        }
    }
    facts.revision = format!(
        "summary:v1:{}",
        hex::encode(Sha256::digest(serde_json::to_vec(&sorted(material))?))
    );
    if serde_json::to_vec(&facts)?.len() > MAX_WIRE_BYTES {
        return Err("conversation summary exceeds24MiB wire budget".into());
    }
    Ok(facts)
}
