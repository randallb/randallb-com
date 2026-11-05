#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Host capabilities are independent wire flags, not mutually exclusive states."
)]
pub struct ConversationSourceCapabilities {
    pub can_read: bool,
    #[serde(default)]
    pub can_read_timeline: bool,
    pub can_create: bool,
    pub can_continue: bool,
    #[serde(default)]
    pub can_rename: bool,
    #[serde(default)]
    pub can_archive: bool,
    #[serde(default)]
    pub can_transfer: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub can_fork: bool,
    pub can_stop: bool,
    pub can_set_live_status: bool,
}

impl ConversationSourceCapabilities {
    pub const fn read_only() -> Self {
        Self {
            can_read: true,
            can_read_timeline: false,
            can_create: false,
            can_continue: false,
            can_rename: false,
            can_archive: false,
            can_transfer: false,
            can_fork: false,
            can_stop: false,
            can_set_live_status: false,
        }
    }

    pub const fn continuable() -> Self {
        Self {
            can_read: true,
            can_read_timeline: false,
            can_create: false,
            can_continue: true,
            can_rename: false,
            can_archive: false,
            can_transfer: false,
            can_fork: false,
            can_stop: false,
            can_set_live_status: false,
        }
    }

    pub const fn controllable() -> Self {
        Self {
            can_read: true,
            can_read_timeline: false,
            can_create: true,
            can_continue: true,
            can_rename: false,
            can_archive: false,
            can_transfer: false,
            can_fork: false,
            can_stop: true,
            can_set_live_status: true,
        }
    }
}
