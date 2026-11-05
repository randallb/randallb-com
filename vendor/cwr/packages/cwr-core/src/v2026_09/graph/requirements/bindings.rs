use super::*;

pub(crate) fn forward_bindings(
    bindings: &Value,
    parent: &InputBindings,
) -> CwrResult<InputBindings> {
    if let Some(mappings) = bindings.as_array() {
        let mut result = InputBindings::new();
        for mapping in mappings {
            let alias = mapping
                .get("as")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid_error("input mapping requires as"))?;
            let reference = mapping
                .get("ref")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid_error("input mapping requires ref"))?;
            let name = reference.strip_prefix("~@/").ok_or_else(|| {
                invalid_error("input ref must select an alias in the containing declaration")
            })?;
            let binding = parent
                .get(name)
                .ok_or_else(|| invalid_error(format!("unknown input source {reference}")))?;
            if result.insert(alias.into(), binding.clone()).is_some() {
                return invalid(format!("duplicate input mapping {alias}"));
            }
        }
        return Ok(result);
    }
    bindings
        .as_object()
        .ok_or_else(|| invalid_error("input bindings must be an object or mappings"))?
        .iter()
        .map(|(name, value)| Ok((name.clone(), bind_template(value, parent)?)))
        .collect()
}

pub(crate) fn bind_template(value: &Value, inputs: &InputBindings) -> CwrResult<InputBinding> {
    if let Some(object) = value.as_object() {
        if object.len() == 1 && object.contains_key("input") {
            let name = object["input"]
                .as_str()
                .ok_or_else(|| invalid_error("input forwarding name must be a string"))?;
            return inputs
                .get(name)
                .cloned()
                .ok_or_else(|| invalid_error(format!("missing forwarded input {name}")));
        }
    }
    let mut sources = BTreeSet::new();
    let mut status = RequirementStatus::Supplied;
    let mut complete = true;
    let mut bind_child = |child: &Value| -> CwrResult<Option<(Option<Value>, BindingExpression)>> {
        let bound = bind_template(child, inputs)?;
        if bound.omitted() {
            return Ok(None);
        }
        let expression = bound.expression();
        sources.extend(bound.sources);
        if bound.value.is_none() {
            complete = false;
        }
        if bound.status != RequirementStatus::Supplied {
            status = bound.status;
        }
        Ok(Some((bound.value, expression)))
    };
    let (concrete, expression) = match value {
        Value::Object(object) => {
            let mut result = serde_json::Map::new();
            let mut expressions = BTreeMap::new();
            for (name, child) in object {
                if let Some((value, expression)) = bind_child(child)? {
                    if let Some(value) = value {
                        result.insert(name.clone(), value);
                    }
                    expressions.insert(name.clone(), expression);
                }
            }
            (
                Value::Object(result),
                BindingExpression::Object(expressions),
            )
        }
        Value::Array(array) => {
            let mut result = Vec::new();
            let mut expressions = Vec::new();
            for child in array {
                if let Some((value, expression)) = bind_child(child)? {
                    if let Some(value) = value {
                        result.push(value);
                    }
                    expressions.push(expression);
                }
            }
            (Value::Array(result), BindingExpression::Array(expressions))
        }
        _ => (value.clone(), BindingExpression::Value(value.clone())),
    };
    Ok(InputBinding {
        value: complete.then_some(concrete),
        sources: sources.into_iter().collect(),
        status,
        optional: false,
        environment: None,
        unresolved: Some(expression),
    })
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum InputEquivalence {
    Equal,
    Conflicting,
    Unresolved,
}

pub(crate) fn equivalent_inputs(left: &InputBindings, right: &InputBindings) -> InputEquivalence {
    let present = |inputs: &InputBindings| {
        inputs
            .iter()
            .filter(|(_, binding)| !binding.omitted())
            .map(|(name, _)| name.clone())
            .collect::<BTreeSet<_>>()
    };
    if present(left) != present(right) {
        return InputEquivalence::Conflicting;
    }
    let mut result = InputEquivalence::Equal;
    for name in present(left) {
        let (left, right) = (&left[&name], &right[&name]);
        // Type preservation has already been checked on each incoming edge.
        // Managed identity must survive even if two sources have equal values.
        let managed = |binding: &InputBinding| {
            binding.sources.iter().any(|s| {
                s.type_ref == SECRET_TYPE || s.storage == Some(PlaintextStorage::WorkspaceVariable)
            })
        };
        if (managed(left) || managed(right)) && left.expression() != right.expression() {
            return InputEquivalence::Conflicting;
        }
        match (&left.value, &right.value) {
            (Some(left), Some(right)) if left != right => return InputEquivalence::Conflicting,
            (Some(_), Some(_)) => {}
            _ if left.expression() == right.expression() => {}
            _ => result = InputEquivalence::Unresolved,
        }
    }
    result
}
