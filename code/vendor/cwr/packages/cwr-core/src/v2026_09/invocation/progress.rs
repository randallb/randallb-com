use serde::{Deserialize, Serialize};

/// Action-authored measurements. The summary remains the human-readable contract.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(try_from = "ProgressWire")]
pub struct ActionProgress {
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProgressWire {
    key: String,
    #[serde(default, deserialize_with = "present")]
    phase: Option<String>,
    #[serde(default, deserialize_with = "present")]
    completed: Option<u64>,
    #[serde(default, deserialize_with = "present")]
    total: Option<u64>,
    #[serde(default, deserialize_with = "present")]
    unit: Option<String>,
}

pub(crate) fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

impl TryFrom<ProgressWire> for ActionProgress {
    type Error = String;
    fn try_from(value: ProgressWire) -> Result<Self, Self::Error> {
        let progress = Self {
            key: value.key,
            phase: value.phase,
            completed: value.completed,
            total: value.total,
            unit: value.unit,
        };
        progress.validate()?;
        Ok(progress)
    }
}

impl ActionProgress {
    pub fn validate(&self) -> Result<(), String> {
        for value in std::iter::once(&self.key)
            .chain(self.phase.iter())
            .chain(self.unit.iter())
        {
            if value.trim().is_empty() || value.contains(['\r', '\n']) {
                return Err("progress key, phase, and unit must be nonblank single lines".into());
            }
        }
        if self.completed.is_some() != self.total.is_some() {
            return Err("progress completed and total must be supplied together".into());
        }
        if self
            .completed
            .into_iter()
            .chain(self.total)
            .any(|count| count > 9_007_199_254_740_991)
        {
            return Err("progress counts must be safe integers".into());
        }
        Ok(())
    }
}

impl schemars::JsonSchema for ActionProgress {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ActionProgress".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "object",
            "additionalProperties": false,
            "required": ["key"],
            "properties": {
                "key": progress_string_schema(),
                "phase": progress_string_schema(),
                "completed": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 9_007_199_254_740_991_u64
                },
                "total": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 9_007_199_254_740_991_u64
                },
                "unit": progress_string_schema()
            },
            "dependentRequired": {"completed": ["total"], "total": ["completed"]}
        })
    }
}

fn progress_string_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "string",
        "pattern": "\\S",
        "not": {"pattern": "[\\r\\n]"}
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v2026_09::invocation::ActionResponse;
    use serde_json::json;

    #[test]
    fn progress_schema_and_model_agree_and_round_trip() {
        let schema: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../packages/cwr-core/contracts/cwp/schemas/2026-09/action_response/schema.json"
        ))
        .unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let base = json!({"invocation_ref":"cwp://example.test/invocations/test",
            "sequence":1,"status":102,"summary":"Tests: 2/3 complete."});
        let mut cases = vec![(base.clone(), true)];
        for (progress, valid) in [
            (json!({"key":"tests"}), true),
            (
                json!({"key":"tests","phase":"testing","completed":2,"total":3,"unit":"binaries"}),
                true,
            ),
            (json!({"key":"tests","completed":0,"total":0}), true),
            (json!({"key":"tests","completed":1}), false),
            (json!({"key":"tests","total":1}), false),
            (json!({"key":"tests","completed":-1,"total":2}), false),
            (json!({"key":"tests","completed":0.5,"total":2}), false),
            (
                json!({"key":"tests","completed":9007199254740992u64,"total":9007199254740992u64}),
                false,
            ),
            (json!({"key":" "}), false),
            (json!({"key":"a\nb"}), false),
            (json!({"key":"tests","phase":null}), false),
            (json!({"key":"tests","extra":true}), false),
            (json!(null), false),
        ] {
            let mut response = base.clone();
            response["progress"] = progress;
            cases.push((response, valid));
        }
        for (status, detail) in [
            (200, "result"),
            (422, "error"),
            (499, "cause"),
            (500, "error"),
        ] {
            let mut response = base.clone();
            response["status"] = json!(status);
            response[detail] = json!({"code":"fixture"});
            response["progress"] = json!({"key":"tests"});
            cases.push((response, false));
        }
        for (value, valid) in cases {
            assert_eq!(validator.is_valid(&value), valid, "schema: {value}");
            let parsed = serde_json::from_value::<ActionResponse>(value.clone());
            assert_eq!(parsed.is_ok(), valid, "model: {value}: {parsed:?}");
            if let Ok(parsed) = parsed {
                assert_eq!(serde_json::to_value(parsed).unwrap(), value);
            }
        }
        assert!(
            serde_json::from_value::<ActionProgress>(json!({"key":"tests","unit":""})).is_err()
        );
    }
}
