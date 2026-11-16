impl LocalRuntime {
    fn finish_action_only_message(
        &self,
        graph: &ResolvedCwrGraph,
        record: &ActionInvocationRecord,
        source_message_ref: &str,
        queue: &Rc<RefCell<VecDeque<String>>>,
        observer: &mut dyn FnMut(LocalRuntimeEvent),
    ) -> CwrResult<Option<CwrResult<()>>> {
        let actions_only = graph.actor_integrations.iter().any(|binding| {
            binding.actor_ref == record.target_actor_ref
                && record.actor_integration_ref.as_deref()
                    == Some(binding.actor_integration_ref.as_str())
                && binding.message_mode
                    == crate::v2026_09::graph::ActorIntegrationMessageMode::ActionsOnly
        });
        if !actions_only || record.action_name != RESPOND_TO_MESSAGE_ACTION_NAME {
            return Ok(None);
        }
        let guidance: CwrResult<(String, BTreeMap<String, ResolvedCwrActionRoute>)> = (|| {
            let state = self.lock()?;
            let conversation_ref = record
                .conversation_ref
                .as_deref()
                .ok_or_else(|| invalid_error("message invocation has no conversation ref"))?;
            let source_text = state
                .admission
                .conversation(conversation_ref)
                .and_then(|conversation| {
                    conversation
                        .messages
                        .iter()
                        .find(|message| message.message_ref == source_message_ref)
                })
                .map(|message| message.text.clone())
                .ok_or_else(|| invalid_error("message invocation source is unavailable"))?;
            let requester_integration = record
                .parent_invocation_ref
                .as_deref()
                .and_then(|parent_ref| state.admission.invocation_record(parent_ref))
                .and_then(|parent| parent.actor_integration_ref.as_deref());
            let allowlist = action_actor_allowlist(
                &state.graph,
                &record.requester_actor_ref,
                requester_integration,
            )?;
            let target_integration = record
                .actor_integration_ref
                .as_deref()
                .ok_or_else(|| invalid_error("message invocation has no actor integration ref"))?;
            let routes = visible_action_routes_for_integration(
                &state,
                conversation_ref,
                &record.requester_actor_ref,
                allowlist.as_ref(),
                Some((&record.target_actor_ref, target_integration)),
            )?;
            Ok((source_text, routes))
        })();
        let (source_text, routes) = match guidance {
            Ok(guidance) => guidance,
            Err(error) => {
                return Ok(Some(self.publish_claim_failure(
                    record,
                    error.to_string(),
                    queue,
                    observer,
                )));
            }
        };
        let text = action_only_guidance(&routes, &source_text);
        self.lock()?.active.remove(&record.invocation_ref);
        let settlement = self.publish_terminal(
            record,
            AdapterTerminal {
                invocation_ref: record.invocation_ref.clone(),
                sequence: 1,
                status: 200,
                summary: text.clone(),
                result: Some(json!({"text": text})),
                error: None,
                continuation: None,
            },
            queue,
            observer,
        );
        Ok(Some(self.recover_claim_after_error(
            &record.invocation_ref,
            settlement,
        )))
    }
}

fn action_only_guidance(
    routes: &BTreeMap<String, ResolvedCwrActionRoute>,
    source_text: &str,
) -> String {
    if routes.is_empty() {
        return "I only respond through named actions, and no actions are currently available to you.".into();
    }
    let source_words = searchable_words(source_text);
    let scores = routes
        .iter()
        .map(|(activation, route)| {
            let searchable = format!(
                "{} {} {}",
                route.action_name, route.action_title, route.action_description
            );
            let score = searchable_words(&searchable)
                .iter()
                .filter(|word| source_words.contains(*word))
                .count();
            (score, activation)
        })
        .filter(|(score, _)| *score > 0)
        .collect::<Vec<_>>();
    let maximum = scores.iter().map(|(score, _)| *score).max();
    let likely = maximum.and_then(|maximum| {
        let mut matches = scores
            .iter()
            .filter(|(score, _)| *score == maximum)
            .map(|(_, activation)| *activation);
        let first = matches.next()?;
        matches.next().is_none().then_some(first)
    });

    let mut text = String::from("I only respond through named actions.\n");
    if let Some(activation) = likely {
        let route = &routes[activation];
        text.push_str(&format!("\nLikely action: `{activation}"));
        if let Some(example) = action_input_example(&route.input_schema) {
            text.push_str(&format!(" {example}"));
        }
        text.push_str(&format!(
            "` — {}: {}\n",
            route.action_title, route.action_description
        ));
    }
    text.push_str("\nAvailable actions:\n");
    for (activation, route) in routes {
        let required = required_input_names(&route.input_schema);
        let inputs = if required.is_empty() {
            "none".into()
        } else {
            required.join(", ")
        };
        text.push_str(&format!(
            "\n- `{activation}` — {}: {} Required inputs: {inputs}.\n",
            route.action_title, route.action_description,
        ));
        if let Some(example) = action_input_example(&route.input_schema) {
            text.push_str(&format!("  Example: `{activation} {example}`\n"));
        }
    }
    text
}

fn searchable_words(text: &str) -> BTreeSet<String> {
    text.to_lowercase()
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|word| word.len() > 2)
        .map(str::to_owned)
        .collect()
}

fn required_input_names(schema: &Value) -> Vec<&str> {
    schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

fn action_input_example(schema: &Value) -> Option<String> {
    schema_example_value(schema).map(|value| value.to_string())
}

fn schema_example_value(schema: &Value) -> Option<Value> {
    let value = schema_example_candidate(schema)?;
    jsonschema::is_valid(schema, &value).then_some(value)
}

fn schema_example_candidate(schema: &Value) -> Option<Value> {
    if let Some(value) = schema.get("const").cloned() {
        return Some(value);
    }
    if let Some(value) = schema.get("default").cloned() {
        return Some(value);
    }
    if let Some(value) = schema
        .get("examples")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
    {
        return Some(value.clone());
    }
    if let Some(value) = schema
        .get("enum")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
    {
        return Some(value.clone());
    }
    for alternatives in ["oneOf", "anyOf"] {
        if let Some(value) = schema
            .get(alternatives)
            .and_then(Value::as_array)
            .and_then(|schemas| schemas.iter().find_map(schema_example_value))
        {
            return Some(value);
        }
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("object") => object_example_value(schema),
        Some("array") => array_example_value(schema),
        Some("string") => string_example_value(schema),
        Some("integer") => number_example_value(schema, true),
        Some("number") => number_example_value(schema, false),
        Some("boolean") => Some(json!(false)),
        Some("null") => Some(Value::Null),
        _ => None,
    }
}

fn object_example_value(schema: &Value) -> Option<Value> {
    let properties = schema.get("properties").and_then(Value::as_object);
    required_input_names(schema)
        .into_iter()
        .map(|name| {
            let property = properties?.get(name)?;
            Some((name.to_owned(), schema_example_value(property)?))
        })
        .collect::<Option<Map<_, _>>>()
        .map(Value::Object)
}

fn array_example_value(schema: &Value) -> Option<Value> {
    let minimum = schema.get("minItems").and_then(Value::as_u64).unwrap_or(0);
    let item = (minimum > 0)
        .then(|| schema.get("items").and_then(schema_example_value))
        .flatten();
    if minimum > 0 && item.is_none() {
        return None;
    }
    Some(Value::Array(vec![item.unwrap_or(Value::Null); minimum as usize]))
}

fn string_example_value(schema: &Value) -> Option<Value> {
    let minimum = schema.get("minLength").and_then(Value::as_u64).unwrap_or(1);
    let maximum = schema.get("maxLength").and_then(Value::as_u64);
    if maximum.is_some_and(|maximum| minimum > maximum) {
        return None;
    }
    let target = minimum.max(7).min(maximum.unwrap_or(u64::MAX));
    let mut value = "example".to_owned();
    value.truncate(target as usize);
    value.extend(std::iter::repeat_n('x', target as usize - value.len()));
    Some(Value::String(value))
}

fn number_example_value(schema: &Value, integer: bool) -> Option<Value> {
    if schema.get("exclusiveMinimum").is_some() || schema.get("multipleOf").is_some() {
        return None;
    }
    let minimum = schema.get("minimum").and_then(Value::as_f64).unwrap_or(0.0);
    let maximum = schema.get("maximum").and_then(Value::as_f64);
    let value = if integer { minimum.ceil() } else { minimum };
    if maximum.is_some_and(|maximum| value > maximum) {
        return None;
    }
    if integer {
        Some(json!(value as i64))
    } else {
        Number::from_f64(value).map(Value::Number)
    }
}

#[cfg(test)]
mod action_only_tests {
    use super::*;

    #[test]
    fn example_values_fill_nested_required_constraints() {
        let schema = json!({
            "type": "object",
            "required": ["paths", "handoff"],
            "properties": {
                "paths": {
                    "type": "array",
                    "minItems": 1,
                    "items": {"type": "string", "minLength": 2}
                },
                "handoff": {
                    "type": "object",
                    "required": ["objective"],
                    "properties": {"objective": {"type": "string"}}
                }
            }
        });

        assert_eq!(
            schema_example_value(&schema),
            Some(json!({"paths": ["example"], "handoff": {"objective": "example"}}))
        );
    }

    #[test]
    fn example_values_are_omitted_when_the_candidate_violates_the_schema() {
        let schema = json!({
            "type": "object",
            "required": ["name"],
            "properties": {"name": {"type": "string", "pattern": "^z{10}$"}}
        });

        assert_eq!(action_input_example(&schema), None);
    }
}
