use super::*;

/// Browse one conversation using small excerpts. Individual serialized messages
/// above 8 MiB are rejected before decoding; use a full archive read for them.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConversationBrowseRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "browse_optional_nonempty_ref_schema")]
    pub conversation_ref: Option<String>,
    pub operation: ConversationBrowseOperation,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConversationBrowseOperation {
    Recent {
        #[schemars(schema_with = "browse_recent_page_schema")]
        page: ConversationRecordPageRequest,
        #[serde(default = "default_excerpt_bytes")]
        #[schemars(schema_with = "browse_excerpt_bytes_schema")]
        excerpt_bytes: usize,
    },
    #[schemars(schema_with = "browse_around_schema")]
    Around {
        #[schemars(schema_with = "browse_nonempty_ref_schema")]
        message_ref: String,
        #[serde(default = "default_around_count")]
        before: usize,
        #[serde(default = "default_around_count")]
        after: usize,
        #[serde(default = "default_excerpt_bytes")]
        #[schemars(schema_with = "browse_excerpt_bytes_schema")]
        excerpt_bytes: usize,
    },
    Search {
        #[schemars(schema_with = "browse_search_query_schema")]
        query: String,
        #[serde(default)]
        after_position: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(schema_with = "browse_optional_nonempty_ref_schema")]
        after_revision: Option<String>,
        #[serde(default = "default_search_count")]
        #[schemars(schema_with = "browse_result_limit_schema")]
        limit: usize,
        #[serde(default = "default_excerpt_bytes")]
        #[schemars(schema_with = "browse_excerpt_bytes_schema")]
        excerpt_bytes: usize,
    },
    Excerpt {
        #[schemars(schema_with = "browse_nonempty_ref_schema")]
        message_ref: String,
        #[serde(default)]
        offset_bytes: usize,
        #[serde(default)]
        action_payload_offset_bytes: usize,
        #[serde(default = "default_excerpt_bytes")]
        #[schemars(schema_with = "browse_nonempty_excerpt_bytes_schema")]
        max_bytes: usize,
    },
    Links {
        #[schemars(schema_with = "browse_nonempty_ref_schema")]
        message_ref: String,
        #[serde(default)]
        #[schemars(schema_with = "browse_optional_nonempty_ref_schema")]
        after_reply_message_ref: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(schema_with = "browse_optional_nonempty_ref_schema")]
        after_revision: Option<String>,
        #[serde(default = "default_search_count")]
        #[schemars(schema_with = "browse_result_limit_schema")]
        limit: usize,
    },
}

fn browse_around_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    let max_other = crate::v2026_09::conversation::MAX_CONVERSATION_BROWSE_PAGE_SIZE - 1;
    let default_other = max_other - default_around_count();
    let mut schema = serde_json::json!({
        "type": "object", "additionalProperties": false,
        "properties": {
            "message_ref": {"type": "string", "minLength": 1},
            "before": {"type": "integer", "minimum": 0, "maximum": max_other, "default": default_around_count()},
            "after": {"type": "integer", "minimum": 0, "maximum": max_other, "default": default_around_count()},
            "excerpt_bytes": {"type": "integer", "minimum": 4, "maximum": 2048, "default": 1024}
        },
        "required": ["message_ref"]
    });
    let mut rules = vec![
        serde_json::json!({
            "if": {"not": {"required": ["before"]}},
            "then": {"properties": {"after": {"maximum": default_other}}}
        }),
        serde_json::json!({
            "if": {"not": {"required": ["after"]}},
            "then": {"properties": {"before": {"maximum": default_other}}}
        }),
    ];
    for before in 0..=max_other {
        rules.push(serde_json::json!({
            "if": {"required": ["before"], "properties": {"before": {"const": before}}},
            "then": {"properties": {"after": {"maximum": max_other - before}}}
        }));
    }
    schema["allOf"] = serde_json::Value::Array(rules);
    schema.try_into().expect("around-window schema is valid")
}

fn browse_excerpt_bytes_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"type": "integer", "minimum": 4, "maximum": 2048})
}

fn browse_nonempty_ref_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"type": "string", "minLength": 1})
}

fn browse_optional_nonempty_ref_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"type": ["string", "null"], "minLength": 1})
}

fn browse_nonempty_excerpt_bytes_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"type": "integer", "minimum": 4, "maximum": 2048})
}

fn browse_search_query_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": "string", "minLength": 1, "maxLength": 256,
        "pattern": "\\S",
        "description": "Non-whitespace search text, at most 256 Unicode characters."
    })
}

fn browse_result_limit_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"type": "integer", "minimum": 1, "maximum": 25})
}

fn browse_recent_page_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    let cursor = serde_json::json!({
        "type": "object", "additionalProperties": false,
        "required": ["position"],
        "properties": {
            "position": {"type": "integer"},
            "revision": {"type": "string", "minLength": 1}
        }
    });
    schemars::json_schema!({
        "type": "object", "additionalProperties": false,
        "anyOf": [{"required": ["first"]}, {"required": ["last"]}],
        "properties": {
            "after": cursor.clone(), "before": cursor,
            "first": {"type": "integer", "minimum": 0, "maximum": 25},
            "last": {"type": "integer", "minimum": 0, "maximum": 25}
        }
    })
}

pub(super) const fn default_excerpt_bytes() -> usize {
    1024
}
const fn default_around_count() -> usize {
    10
}
const fn default_search_count() -> usize {
    20
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConversationBrowseResponse {
    pub conversation_ref: String,
    pub records: ConversationBrowseRecords,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConversationBrowseRecords {
    Page {
        page: crate::v2026_09::conversation::ConversationBrowsePage,
    },
    Search {
        page: crate::v2026_09::conversation::ConversationSearchPage,
    },
    Excerpt {
        entry: Box<crate::v2026_09::conversation::ConversationBrowseEntry>,
    },
    Links {
        links: crate::v2026_09::conversation::ConversationMessageLinks,
    },
}
