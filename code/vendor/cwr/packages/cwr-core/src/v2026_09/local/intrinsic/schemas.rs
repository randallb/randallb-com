fn intrinsic_action_input_schema(action_name: &str) -> Value {
    match action_name {
        "get--schedule" | "pause--schedule" | "resume--schedule" | "list--schedule_runs" => json!({
            "type":"object", "additionalProperties":false, "required":["schedule_ref"],
            "properties":{"schedule_ref":{"type":"string","minLength":1}}
        }),
        "set--experimental_features" => json!({
            "type": "object", "additionalProperties": false, "required": ["enabled", "revision"],
            "properties": {"enabled": {"type": "boolean"}, "revision": {"type": "string", "minLength": 1}}
        }),
        "fork--conversation" => json!({"type":"object","additionalProperties":false,"required":["message_ref"],"properties":{"message_ref":{"type":"string","minLength":1},"timeline_item_ref":{"type":"string","minLength":1}}}),
        "rename--conversation" => json!({
            "type": "object", "additionalProperties": false, "required": ["title"],
            "properties": {"title": {"type": "string", "minLength": 1}}
        }),
        "list--conversations" => json!({
            "type": "object", "additionalProperties": false,
            "properties": {
                "archived": {"type": "boolean"},
                "page": conversation_page_input_schema(200, true)
            }
        }),
        "read--conversation" => json!({
            "type": "object", "additionalProperties": false,
            "properties": {
                "conversation_ref": {"type": "string", "minLength": 1},
                "page": conversation_page_input_schema(500, false)
            }
        }),
        "browse--conversation" => {
            generated_input_schema::<crate::intrinsic_cwr::ConversationBrowseRequest>()
        }
        "debug--invocation" | "stop--invocation" => json!({
            "type": "object", "additionalProperties": false,
            "properties": {"invocation_ref": {"type": "string", "minLength": 1}}
        }),
        _ => json!({"type": "object", "additionalProperties": false}),
    }
}

fn conversation_page_input_schema(maximum: usize, summary: bool) -> Value {
    let cursor = if summary {
        json!({
            "type": "object", "additionalProperties": false,
            "required": ["creation_order", "conversation_ref"],
            "properties": {
                "creation_order": {"type": "integer"},
                "conversation_ref": {"type": "string", "minLength": 1}
            }
        })
    } else {
        json!({
            "type": "object", "additionalProperties": false, "required": ["position"],
            "properties": {
                "position": {"type": "integer"},
                "revision": {"type": "string", "minLength": 1}
            }
        })
    };
    json!({
        "type": "object", "additionalProperties": false,
        "anyOf": [{"required": ["first"]}, {"required": ["last"]}],
        "properties": {
            "after": cursor.clone(), "before": cursor,
            "first": {"type": "integer", "minimum": 0, "maximum": maximum},
            "last": {"type": "integer", "minimum": 0, "maximum": maximum}
        }
    })
}

fn intrinsic_action_output_schema(action_name: &str) -> Value {
    match action_name {
        "read--experimental_features" | "set--experimental_features" => {
            generated_schema::<experimental_features::Report>()
        }
        "fork--conversation" => json!({"type":"object","additionalProperties":false,"required":["kind","conversation_ref","source_conversation_ref","source_message_ref"],"properties":{"kind":{"const":"conversation_forked"},"conversation_ref":{"type":"string"},"source_conversation_ref":{"type":"string"},"source_message_ref":{"type":"string"}}}),
        "rename--conversation" => json!({
            "type": "object", "additionalProperties": false,
            "required": ["kind", "conversation_ref", "title"],
            "properties": {
                "kind": {"const": "conversation_renamed"},
                "conversation_ref": {"type": "string"}, "title": {"type": "string"}
            }
        }),
        "list--conversations" => generated_conversation_list_schema(),
        "read--conversation" => generated_conversation_read_schema(),
        "browse--conversation" => generated_schema::<ConversationBrowseActionResponse>(),
        "archive--conversation" | "restore--conversation" => json!({
            "type": "object", "additionalProperties": false,
            "required": ["kind", "conversation_ref", "archived"],
            "properties": {
                "kind": {"const": "conversation_archived"},
                "conversation_ref": {"type": "string"}, "archived": {"type": "boolean"}
            }
        }),
        _ => json!({}),
    }
}

#[derive(schemars::JsonSchema)]
#[expect(
    dead_code,
    reason = "This type defines the generated action output contract."
)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ConversationListActionResponse {
    ConversationsListed {
        #[serde(flatten)]
        response: crate::intrinsic_cwr::ConversationListResponse,
    },
}

#[derive(schemars::JsonSchema)]
#[expect(
    dead_code,
    reason = "This type defines the generated action output contract."
)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ConversationReadActionResponse {
    ConversationRead {
        #[serde(flatten)]
        response: crate::intrinsic_cwr::ConversationReadResponse,
    },
}

#[derive(schemars::JsonSchema)]
#[expect(
    dead_code,
    reason = "This type defines the generated action output contract."
)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ConversationBrowseActionResponse {
    ConversationBrowsed {
        #[serde(flatten)]
        response: crate::intrinsic_cwr::ConversationBrowseResponse,
    },
}

fn generated_conversation_list_schema() -> Value {
    generated_schema::<ConversationListActionResponse>()
}

fn generated_conversation_read_schema() -> Value {
    generated_schema::<ConversationReadActionResponse>()
}

fn generated_schema<T: schemars::JsonSchema>() -> Value {
    let schema = schemars::generate::SchemaSettings::draft2020_12()
        .for_serialize()
        .into_generator()
        .into_root_schema_for::<T>();
    serde_json::to_value(schema).expect("generated JSON schema is serializable")
}

fn generated_input_schema<T: schemars::JsonSchema>() -> Value {
    let schema = schemars::generate::SchemaSettings::draft2020_12()
        .for_deserialize()
        .into_generator()
        .into_root_schema_for::<T>();
    serde_json::to_value(schema).expect("generated JSON schema is serializable")
}
