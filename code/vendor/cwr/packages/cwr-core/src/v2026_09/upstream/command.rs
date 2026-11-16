use super::{
    ConversationContextPolicy, DelegationSource, UpstreamCoworker, UpstreamGatewayHandler,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum UpstreamCommand {
    /// A trusted native service ingress preserves software authorship for one
    /// already permitted local recipient. This is not coworker routing across
    /// runtimes: no destination runtime or pack-selected provider is accepted.
    SubmitDelegatedText {
        provider_session_ref: String,
        author_actor_ref: String,
        target_actor_ref: String,
        source: DelegationSource,
        text: String,
    },
    ConfigureInstalledSchedules {
        owner_context_ref: String,
        availability: String,
        declarations: Vec<InstalledScheduleActivation>,
    },
    /// Trusted selected host participant invites through the ordinary CWR policy.
    InviteConversationActor {
        conversation_ref: String,
        actor_ref: String,
    },
    SuspendConversationContexts,
    SetConversationContexts {
        policy: Option<ConversationContextPolicy>,
    },
    Register {
        coworkers: Vec<UpstreamCoworker>,
    },
    RegisterGatewayHandlers {
        handlers: Vec<UpstreamGatewayHandler>,
    },
    Enable {
        provider_session_ref: String,
        actor_ref: String,
        enabled: bool,
    },
    Complete {
        invocation_ref: String,
        terminal: Value,
    },
    /// Withdraw this provider's result from its still-pending consuming turn.
    ProtectResult {
        invocation_ref: String,
    },
    ValidateResult {
        validation_ref: String,
        allowed: bool,
        #[serde(default)]
        authority_guards: Vec<cwpdb_store::conversation::AuthorityGuard>,
    },
    RevokeResult {
        invocation_ref: String,
    },
    /// Confirms that this provider's execution has ended, not merely receipt
    /// of the stop request. A disconnected provider has not confirmed exit.
    AcknowledgeStop {
        invocation_ref: String,
    },
}

/// Authenticated native installer input. Ordinary coworker actions cannot
/// create schedules or supply an installed owner context.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InstalledScheduleActivation {
    pub declaration_ref: String,
    pub revision: String,
    pub target_actor_ref: String,
    pub action_name: String,
    pub input: Value,
    pub anchor_unix_ms: u64,
    pub interval_ms: u64,
}
