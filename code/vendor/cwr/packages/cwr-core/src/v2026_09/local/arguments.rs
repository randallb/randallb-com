pub fn parse_schema_arguments<S: AsRef<str>>(schema: &Value, arguments: &[S]) -> CwrResult<Value> {
    if arguments.is_empty() {
        return Ok(json!({}));
    }
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            invalid_error("action input schema has no top-level object properties; use JSON")
        })?;
    let projected = properties
        .iter()
        .filter(|(name, _)| is_lower_snake_case(name))
        .map(|(name, schema)| (name.replace('_', "-"), (name, schema)))
        .collect::<BTreeMap<_, _>>();
    let mut input = Map::new();
    for argument in arguments {
        let assignment = argument
            .as_ref()
            .strip_prefix("--")
            .ok_or_else(|| invalid_error("action arguments must use --key=value"))?;
        let (flag, raw) = assignment
            .split_once('=')
            .ok_or_else(|| invalid_error("action arguments must use --key=value"))?;
        let (property, property_schema) = projected.get(flag).ok_or_else(|| {
            invalid_error(format!(
                "unknown or non-scalar action input property --{flag}"
            ))
        })?;
        if input.contains_key(property.as_str()) {
            return invalid(format!("duplicate action input property --{flag}"));
        }
        input.insert(
            (*property).clone(),
            parse_scalar_value(flag, raw, property_schema)?,
        );
    }
    Ok(Value::Object(input))
}

pub fn parse_schema_argument_text(schema: &Value, raw: &str) -> CwrResult<Value> {
    let trimmed = raw.trim();
    if trimmed.starts_with('{') {
        let input: Value = serde_json::from_str(trimmed).map_err(|error| {
            invalid_error(format!("inline action input is not valid JSON: {error}"))
        })?;
        if !input.is_object() {
            return invalid("inline action input must be one JSON object");
        }
        return Ok(input);
    }
    if trimmed.is_empty() {
        return Ok(json!({}));
    }
    let tokens = shlex::split(raw)
        .ok_or_else(|| invalid_error("action arguments contain an unterminated quote"))?;
    let mut arguments: Vec<String> = Vec::new();
    let mut awaiting_value = false;
    for token in tokens {
        if awaiting_value {
            let argument = arguments
                .last_mut()
                .expect("a pending action value has an argument");
            argument.push_str(&token);
            awaiting_value = false;
        } else if token.starts_with("--") {
            awaiting_value = !token.contains('=');
            arguments.push(if token.contains('=') {
                token
            } else {
                format!("{token}=")
            });
        } else {
            let Some(argument) = arguments.last_mut() else {
                return invalid("action arguments must start with --key=value or --key value");
            };
            if !argument.ends_with('=') {
                argument.push(' ');
            }
            argument.push_str(&token);
        }
    }
    if awaiting_value {
        return invalid("separated action flags must be followed by a value");
    }
    parse_schema_arguments(schema, &arguments)
}

fn parse_scalar_value(flag: &str, raw: &str, schema: &Value) -> CwrResult<Value> {
    match schema.get("type").and_then(Value::as_str) {
        Some("string") => Ok(Value::String(raw.into())),
        Some("boolean") => match raw {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => invalid(format!("--{flag} must be true or false")),
        },
        Some("null") if raw == "null" => Ok(Value::Null),
        Some("null") => invalid(format!("--{flag} must be null")),
        Some("integer") => {
            let value: Value = serde_json::from_str(raw)
                .map_err(|_| invalid_error(format!("--{flag} must be a JSON integer")))?;
            if value.as_i64().is_none() && value.as_u64().is_none() {
                return invalid(format!("--{flag} must be a JSON integer"));
            }
            Ok(value)
        }
        Some("number") => raw
            .parse::<Number>()
            .map(Value::Number)
            .map_err(|_| invalid_error(format!("--{flag} must be a JSON number"))),
        _ => invalid(format!(
            "--{flag} is not a supported scalar property; use JSON"
        )),
    }
}

fn is_lower_snake_case(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('_')
        && !value.ends_with('_')
        && !value.contains("__")
        && value.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        })
}
