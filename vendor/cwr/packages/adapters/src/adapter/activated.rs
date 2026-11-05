//! Activated declaration contents travel with the admitted invocation.
use crate::CwrResult;
use serde_json::Value;

pub(crate) fn params(params: &Value) -> Value {
    let mut shared = params.clone();
    if let Some(object) = shared.as_object_mut() {
        for field in [
            "activated_sources",
            "activated_entrypoints",
            "role_contexts",
            "cwr_native_tools",
        ] {
            object.remove(field);
        }
    }
    shared
}

/// None supports direct adapter clients without an activated graph. An activated
/// graph must provide every entrypoint; never substitute current filesystem data.
pub(crate) fn instructions(params: &Value) -> CwrResult<Option<String>> {
    if params.get("role_contexts").is_none()
        && params.get("activated_sources").is_none()
        && params.get("activated_entrypoints").is_none()
    {
        return Ok(None);
    }
    let entries = params["activated_entrypoints"]
        .as_array()
        .ok_or("activated configuration has no retained entrypoint list")?;
    let sources = params["activated_sources"]
        .as_object()
        .ok_or("activated configuration has no retained source contents")?;
    let mut prompts = Vec::with_capacity(entries.len());
    for entry in entries {
        let path = entry
            .as_str()
            .ok_or("activated entrypoint must be a path")?;
        prompts.push(
            sources
                .get(path)
                .and_then(Value::as_str)
                .ok_or("activated entrypoint contents cannot be established; execution rejected")?,
        );
    }
    if entries.is_empty() {
        return Err("activated model configuration has no instruction entrypoints".into());
    }
    Ok(Some(prompts.join("\n\n")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn activated_instructions_preserve_order_and_require_every_source() {
        let mut params = json!({"entrypoint":"./PROMPT.md", "role_contexts":[],
            "activated_entrypoints":["/roles/two/PROMPT.md","/roles/one/PROMPT.md"],
            "activated_sources":{"/roles/one/PROMPT.md":"First", "/roles/two/PROMPT.md":"Second"}});
        assert_eq!(instructions(&params).unwrap().unwrap(), "Second\n\nFirst");
        assert_eq!(super::params(&params), json!({"entrypoint":"./PROMPT.md"}));
        params["activated_sources"]
            .as_object_mut()
            .unwrap()
            .remove("/roles/one/PROMPT.md");
        assert!(instructions(&params)
            .unwrap_err()
            .to_string()
            .contains("execution rejected"));
    }
}
