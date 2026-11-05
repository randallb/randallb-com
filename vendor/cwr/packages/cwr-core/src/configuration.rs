//! Startup configuration and setting definitions from the shared repository catalog.
use crate::CwrResult;

include!(concat!(env!("OUT_DIR"), "/configuration.rs"));

pub(crate) struct BooleanSetting {
    pub(crate) default: bool,
    pub(crate) workspace_parameter: String,
    pub(crate) title: String,
    pub(crate) description: String,
}

pub(crate) fn boolean_setting(key: &str) -> CwrResult<BooleanSetting> {
    if key != "desktop.human_reply_inbox" {
        return Err(format!("Unknown configuration key: {key}").into());
    }
    Ok(BooleanSetting {
        default: HUMAN_REPLY_INBOX_DEFAULT,
        workspace_parameter: HUMAN_REPLY_INBOX_PARAMETER.to_owned(),
        title: HUMAN_REPLY_INBOX_TITLE.to_owned(),
        description: HUMAN_REPLY_INBOX_DESCRIPTION.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_shared_human_reply_inbox_definition() {
        let setting = boolean_setting("desktop.human_reply_inbox").unwrap();
        assert!(!setting.default);
        assert_eq!(setting.workspace_parameter, "human_response_actions");
        assert_eq!(setting.title, "Human reply inbox");
        assert!(setting.description.contains("Needs reply"));
    }
}
