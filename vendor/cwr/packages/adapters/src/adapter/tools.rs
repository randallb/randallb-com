//! One logical tool projection and exact dispatch decoder for every model adapter.
use crate::{adapter::AdapterHostOperation, CwrResult};
use cwr::tool_names::LogicalToolName;
use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Registration {
    pub namespace: String,
    pub name: String,
}

impl Registration {
    pub fn flat_name(&self) -> String {
        if self.namespace == "cwr" {
            format!("cwr_{}", self.name)
        } else {
            format!("{}__{}", self.namespace, self.name)
        }
    }
}

pub(crate) fn registration(operation: &AdapterHostOperation) -> CwrResult<Registration> {
    if let Some(name) = operation.input_schema["x-cwr-model-name"].as_str() {
        match operation.input_schema["x-cwr-decoder-version"].as_str() {
            Some("cwp/1") => {
                let name = LogicalToolName::parse(name)?;
                if name.local_name == "cwr--return--result" {
                    return Err("completion tool name is reserved".into());
                }
                return Ok(Registration {
                    namespace: name.namespace,
                    name: name.local_name,
                });
            }
            Some("cwr/1") => {
                let name = name
                    .strip_prefix("cwr.")
                    .filter(|name| {
                        !name.is_empty()
                            && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
                    })
                    .ok_or("unsupported historical tool name")?;
                return Ok(Registration {
                    namespace: "cwr".into(),
                    name: name.into(),
                });
            }
            _ => {
                return Err(
                    "unsupported historical tool contract: decoder version is unavailable".into(),
                )
            }
        }
    }
    let name = LogicalToolName::for_host_operation(&operation.name)?;
    if name.local_name == "cwr--return--result" {
        return Err("completion tool name is reserved".into());
    }
    Ok(Registration {
        namespace: name.namespace,
        name: name.local_name,
    })
}

pub(crate) fn provider_schema(schema: &Value) -> Value {
    let mut schema = schema.clone();
    if let Some(object) = schema.as_object_mut() {
        object.retain(|key, _| !key.starts_with("x-cwr-"));
    }
    schema
}

/// Validate the retained schema first, then decode only its declared contract.
/// Exact targets remain values retained in the registration, never alias lookups.
pub(crate) fn dispatch(
    operation: &AdapterHostOperation,
    input: Value,
) -> CwrResult<(String, Value)> {
    if !jsonschema::is_valid(&operation.input_schema, &input) {
        return Err("tool input does not match its retained schema".into());
    }
    if operation
        .input_schema
        .get("x-cwr-decoder-version")
        .is_some()
        && !matches!(
            operation.input_schema["x-cwr-decoder-version"].as_str(),
            Some("cwp/1" | "cwr/1")
        )
    {
        return Err("unsupported historical tool contract: decoder version is unavailable".into());
    }
    if let Some(target) = operation.input_schema.get("x-cwr-target") {
        return Ok((
            "cwr.invoke_action".into(),
            json!({"target": target, "input": input}),
        ));
    }
    if operation.input_schema["x-cwr-decoder-version"] == "cwr/1" {
        match operation.name.as_str() {
            "cwr.invoke_action" | "cwr.start_action" => {
                let activation = input["activation"]
                    .as_str()
                    .ok_or("unsupported historical action helper input")?;
                let target = operation.input_schema["x-cwr-action-targets"]
                    .get(activation)
                    .filter(|target| target.is_object())
                    .ok_or("unresolved historical target: no retained exact action mapping")?;
                return Ok((
                    operation.name.clone(),
                    json!({"target":target,"input":input["input"]}),
                ));
            }
            "cwr.stop_invocation" => {}
            "cwr.send_message" => {
                return Err(
                    "unresolved historical target: message helper has no retained exact binding"
                        .into(),
                )
            }
            _ => return Err("unsupported historical tool contract: decoder is unavailable".into()),
        }
    }
    Ok((operation.name.clone(), input))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn operation(schema: Value) -> AdapterHostOperation {
        AdapterHostOperation {
            name: "cwr.invoke_action".into(),
            description: "Invoke".into(),
            input_schema: schema,
            output_schema: json!({}),
        }
    }
    #[test]
    fn legacy_decoder_uses_only_retained_exact_target_and_schema() {
        let old = operation(json!({"type":"object","required":["activation","input"],
            "properties":{"activation":{"const":"@old--resize--photo"},"input":{}},
            "x-cwr-model-name":"cwr.invoke_action","x-cwr-decoder-version":"cwr/1",
            "x-cwr-action-targets":{"@old--resize--photo":{"binding_ref":"original","contract_id":"old"}}}));
        assert_eq!(registration(&old).unwrap().flat_name(), "cwr_invoke_action");
        let (name, decoded) = dispatch(
            &old,
            json!({"activation":"@old--resize--photo","input":{"width":5}}),
        )
        .unwrap();
        assert_eq!(name, "cwr.invoke_action");
        assert_eq!(decoded["target"]["binding_ref"], "original");
        assert_eq!(decoded["input"]["width"], 5);
        assert!(dispatch(&old, json!({"activation":"@new--resize--photo","input":{}})).is_err());
        let mut unmapped = old.clone();
        unmapped
            .input_schema
            .as_object_mut()
            .unwrap()
            .remove("x-cwr-action-targets");
        assert!(dispatch(
            &unmapped,
            json!({"activation":"@old--resize--photo","input":{}})
        )
        .unwrap_err()
        .to_string()
        .contains("unresolved historical target"));
        assert_eq!(old.input_schema["x-cwr-model-name"], "cwr.invoke_action");
    }
    #[test]
    fn native_tools_wrap_retained_target_without_reinterpreting_the_name() {
        let mut native = operation(
            json!({"type":"object","x-cwr-target":{"coworker_ref":"alice","binding_ref":"claude"}}),
        );
        native.name = "cwp.alice--resize--photo".into();
        assert_eq!(
            registration(&native).unwrap().flat_name(),
            "cwp__alice--resize--photo"
        );
        assert_eq!(
            dispatch(&native, json!({})).unwrap().1["target"]["binding_ref"],
            "claude"
        );
        native.input_schema["x-cwr-decoder-version"] = json!("future/99");
        assert!(dispatch(&native, json!({}))
            .unwrap_err()
            .to_string()
            .contains("unsupported historical tool contract"));
    }
}
