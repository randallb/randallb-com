use std::error::Error;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolNameError {
    InvalidCoworker(String),
    InvalidNamespace(String),
    InvalidActionName(String),
    InvalidVerb(String),
    InvalidObject(String),
}

impl fmt::Display for ToolNameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidNamespace(name) => write!(f, "model tools require the cwp namespace and a coworker--verb--object name: {name}"),
            Self::InvalidCoworker(subject) => {
                write!(f, "tool coworker must be lowercase snake_case: {subject}")
            }
            Self::InvalidActionName(action_name) => write!(
                f,
                "action name must contain exactly two hyphens between verb and object: {action_name}"
            ),
            Self::InvalidVerb(verb) => {
                write!(f, "tool verb must be lowercase snake_case: {verb}")
            }
            Self::InvalidObject(object) => {
                write!(f, "tool object must be lowercase snake_case: {object}")
            }
        }
    }
}

impl Error for ToolNameError {}

/// Project a coworker slug and its verb--object action name into an concrete
/// coworker--verb--object tool name. Each field must be lowercase snake_case;
/// underscores remain inside fields and `--` separates C/V/O.
pub fn project_tool_name(coworker_slug: &str, action_name: &str) -> Result<String, ToolNameError> {
    if !is_lower_snake_case(coworker_slug) {
        return Err(ToolNameError::InvalidCoworker(coworker_slug.to_owned()));
    }

    let Some((verb, object)) = action_name.split_once("--") else {
        return Err(ToolNameError::InvalidActionName(action_name.to_owned()));
    };
    if object.contains("--") {
        return Err(ToolNameError::InvalidActionName(action_name.to_owned()));
    }
    if !is_lower_snake_case(verb) {
        return Err(ToolNameError::InvalidVerb(verb.to_owned()));
    }
    if !is_lower_snake_case(object) {
        return Err(ToolNameError::InvalidObject(object.to_owned()));
    }

    Ok(format!("{coworker_slug}--{verb}--{object}"))
}

/// A logical protocol name. Provider encoding never changes its dispatch target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogicalToolName {
    pub namespace: String,
    pub local_name: String,
}

pub const CWP_TOOL_NAMESPACE: &str = "cwp";
pub const RETURN_RESULT_TOOL: &str = "cwp.cwr--return--result";
pub const RETURN_RESULT_LOCAL_NAME: &str = "cwr--return--result";

impl LogicalToolName {
    pub fn parse(name: &str) -> Result<Self, ToolNameError> {
        let local = name
            .strip_prefix("cwp.")
            .ok_or_else(|| ToolNameError::InvalidNamespace(name.into()))?;
        let (coworker, action) = local
            .split_once("--")
            .ok_or_else(|| ToolNameError::InvalidActionName(local.into()))?;
        Ok(Self {
            namespace: CWP_TOOL_NAMESPACE.into(),
            local_name: project_tool_name(coworker, action)?,
        })
    }

    /// Internal host operation IDs remain stable; new model registrations use CVO.
    /// Unknown legacy IDs are deliberately not guessed from display text.
    pub fn for_host_operation(operation: &str) -> Result<Self, ToolNameError> {
        let local = match operation {
            "cwr.send_request" => "cwr--send--request",
            "cwr.send_message" => "cwr--send--message",
            "cwr.list_conversation_contexts" => "cwr--list--conversation_contexts",
            "cwr.create_conversation" => "cwr--create--conversation",
            "cwr.read_file" => "cwr--read--file",
            "cwr.invoke_action" => "cwr--invoke--action",
            "cwr.start_action" => "cwr--start--action",
            "cwr.stop_invocation" => "cwr--stop--invocation",
            "cwr.list_coworkers" => "cwr--list--coworkers",
            "cwr.list_actions" => "cwr--list--actions",
            "cwr.show_help" => "cwr--show--help",
            "cwr.debug_invocation" | "cwr.inspect_invocation" => "cwr--debug--invocation",
            "cwr.reload_graph" => "cwr--reload--graph",
            _ => return Self::parse(operation),
        };
        Self::parse(&format!("{CWP_TOOL_NAMESPACE}.{local}"))
    }

    pub fn qualified(&self) -> String {
        format!("{}.{}", self.namespace, self.local_name)
    }

    /// Flat providers permit hyphens and underscores, but no namespace dot.
    /// CVO fields cannot contain `--`, so this encoding is injective.
    pub fn flat_name(&self) -> String {
        format!("{}__{}", self.namespace, self.local_name)
    }
}

fn is_lower_snake_case(value: &str) -> bool {
    let mut parts = value.split('_');
    let Some(first) = parts.next() else {
        return false;
    };
    let Some(first_char) = first.chars().next() else {
        return false;
    };
    if !first_char.is_ascii_lowercase()
        || !first
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
    {
        return false;
    }
    parts.all(|part| {
        !part.is_empty()
            && part
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_concrete_cvo_tool_name() {
        assert_eq!(
            project_tool_name(
                "docs_builder",
                "send_multiple_word_verb--message_multiple_word_object"
            )
            .unwrap(),
            "docs_builder--send_multiple_word_verb--message_multiple_word_object"
        );
    }

    #[test]
    fn rejects_objectless_action_name() {
        assert_eq!(
            project_tool_name("docs_builder", "send").unwrap_err(),
            ToolNameError::InvalidActionName("send".into())
        );
    }

    #[test]
    fn rejects_extra_action_name_hyphens() {
        assert_eq!(
            project_tool_name("docs_builder", "send--short--message").unwrap_err(),
            ToolNameError::InvalidActionName("send--short--message".into())
        );
    }

    #[test]
    fn rejects_non_snake_case_cvo_fields() {
        assert!(matches!(
            project_tool_name("DocsBuilder", "send--message"),
            Err(ToolNameError::InvalidCoworker(_))
        ));
        assert!(matches!(
            project_tool_name("1docs_builder", "send--message"),
            Err(ToolNameError::InvalidCoworker(_))
        ));
        assert!(matches!(
            project_tool_name("docs_builder", "Send--message"),
            Err(ToolNameError::InvalidVerb(_))
        ));
        assert!(matches!(
            project_tool_name("docs_builder", "send--message__body"),
            Err(ToolNameError::InvalidObject(_))
        ));
    }
}

#[cfg(test)]
mod logical_tests {
    use super::*;
    #[test]
    fn providers_share_one_logical_cvo_projection() {
        let name = LogicalToolName::for_host_operation("cwr.invoke_action").unwrap();
        assert_eq!(name.qualified(), "cwp.cwr--invoke--action");
        assert_eq!(name.flat_name(), "cwp__cwr--invoke--action");
        assert_eq!(LogicalToolName::parse(&name.qualified()).unwrap(), name);
        assert_ne!(
            LogicalToolName::parse("cwp.alice--resize--photo").unwrap(),
            LogicalToolName::parse("cwp.bob--resize--photo").unwrap()
        );
    }
    #[test]
    fn unknown_helpers_and_ambiguous_encodings_are_not_guessed() {
        for name in [
            "cwr.unknown",
            "cwp.alice--resize--photo--large",
            "cwp.alice-bob--resize--photo",
        ] {
            assert!(LogicalToolName::for_host_operation(name).is_err(), "{name}");
        }
    }
}
