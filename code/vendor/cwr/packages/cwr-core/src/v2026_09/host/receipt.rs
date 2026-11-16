#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HostCommandReceipt {
    pub conversation_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invocation_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stopped_invocation_refs: Vec<String>,
}
