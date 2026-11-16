#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversationAdmissionState {
    Recorded,
    Admitted,
    Completed,
    Failed,
    Stopped,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConversationRecipientVisibility {
    #[default]
    #[serde(alias = "contextual")]
    Implicit,
    Explicit,
}

impl ConversationRecipientVisibility {
    fn is_implicit(&self) -> bool {
        *self == Self::Implicit
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConversationAdmissionRecord {
    pub integration_ref: String,
    pub provider_event_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ingress_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_input_fingerprint: Option<String>,
    pub conversation_ref: String,
    pub source_message_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_conversation_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_actor_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_integration_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invocation_ref: Option<String>,
    pub state: ConversationAdmissionState,
    #[serde(
        default,
        skip_serializing_if = "ConversationRecipientVisibility::is_implicit"
    )]
    pub reply_recipient_visibility: ConversationRecipientVisibility,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_message_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PreparedConversationAdmission {
    pub record: ConversationAdmissionRecord,
    pub conversation: RetainedConversation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_conversation: Option<RetainedConversation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invocation: Option<AdapterInvocation>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConversationPreparationOutcome {
    New(Box<PreparedConversationAdmission>),
    Existing(Box<ConversationAdmissionRecord>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConversationAdmissionOutcome {
    Started(Box<AdmittedConversationTurn>),
    Existing(Box<ConversationAdmissionRecord>),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AdmittedConversationTurn {
    pub record: ConversationAdmissionRecord,
    pub conversation: RetainedConversation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_conversation: Option<RetainedConversation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invocation: Option<AdapterInvocation>,
}

