use std::error::Error;
use std::fmt;

const SCHEME: &str = "cwp://";
const LIVE_WORKSPACE_SOURCE: &str = "cwp://!cwr/volumes/@";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CwrRoot {
    Mount,
    Named(String),
    IntrinsicCwr,
    DirectKey(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CwrRef {
    raw: String,
    root: CwrRoot,
    checkpoint: Option<String>,
    segments: Vec<String>,
    fragment: Option<String>,
}

impl CwrRef {
    pub fn parse(input: &str) -> Result<Self, CwrRefError> {
        if input.is_empty() {
            return Err(CwrRefError::Empty);
        }
        if input.trim() != input || input.chars().any(char::is_whitespace) {
            return Err(CwrRefError::Whitespace);
        }
        let (body, mount_relative) = if let Some(body) = input.strip_prefix("~@") {
            (body, true)
        } else if let Some(body) = input.strip_prefix(SCHEME) {
            (body, false)
        } else {
            return Err(CwrRefError::InvalidScheme);
        };
        if body.contains('?') {
            return Err(CwrRefError::QueryUnsupported);
        }
        if body.contains('\\') {
            return Err(CwrRefError::BackslashUnsupported);
        }
        let (body, fragment) = match body.split_once('#') {
            Some((body, fragment)) => {
                if fragment.is_empty()
                    || fragment.contains('#')
                    || !fragment.chars().all(is_fragment_char)
                {
                    return Err(CwrRefError::InvalidFragment(fragment.to_owned()));
                }
                (body, Some(fragment.to_owned()))
            }
            None => (body, None),
        };
        if body.ends_with('/') {
            return Err(CwrRefError::EmptySegment);
        }

        let (root, checkpoint, path) = if mount_relative {
            parse_mount(body)?
        } else {
            parse_ordinary_root(body)?
        };
        let segments = parse_segments(
            path,
            body == LIVE_WORKSPACE_SOURCE
                .strip_prefix(SCHEME)
                .expect("live workspace source uses the CWR scheme"),
        )?;

        Ok(Self {
            raw: input.to_owned(),
            root,
            checkpoint,
            segments,
            fragment,
        })
    }

    pub fn as_str(&self) -> &str {
        &self.raw
    }

    pub fn root(&self) -> &CwrRoot {
        &self.root
    }

    pub fn checkpoint(&self) -> Option<&str> {
        self.checkpoint.as_deref()
    }

    pub fn segments(&self) -> &[String] {
        &self.segments
    }

    pub fn fragment(&self) -> Option<&str> {
        self.fragment.as_deref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoworkerRef(CwrRef);

impl CoworkerRef {
    pub fn parse(input: &str) -> Result<Self, CwrRefError> {
        let reference = CwrRef::parse(input)?;
        if !is_coworker_subject(&reference) {
            return Err(CwrRefError::CoworkerSubjectExpected);
        }
        Ok(Self(reference))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    pub fn reference(&self) -> &CwrRef {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionSelector {
    coworker_ref: CoworkerRef,
    action_name: String,
}

impl ActionSelector {
    pub fn parse(coworker_ref: &str, action_name: &str) -> Result<Self, ActionSelectorError> {
        let coworker_ref = CoworkerRef::parse(coworker_ref)?;
        if !is_action_name(action_name) {
            return Err(ActionSelectorError::InvalidActionName(
                action_name.to_owned(),
            ));
        }
        Ok(Self {
            coworker_ref,
            action_name: action_name.to_owned(),
        })
    }

    pub fn coworker_ref(&self) -> &CoworkerRef {
        &self.coworker_ref
    }

    pub fn action_name(&self) -> &str {
        &self.action_name
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CwrRefError {
    Empty,
    InvalidScheme,
    Whitespace,
    InvalidFragment(String),
    QueryUnsupported,
    BackslashUnsupported,
    EmptyRoot,
    InvalidRoot(String),
    InvalidCheckpoint(String),
    EmptySegment,
    UnsafeSegment(String),
    InvalidPathSegment(String),
    CoworkerSubjectExpected,
}

impl fmt::Display for CwrRefError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("ref is empty"),
            Self::InvalidScheme => f.write_str("ref must start with ~@ or cwp://"),
            Self::Whitespace => f.write_str("ref must not contain whitespace"),
            Self::InvalidFragment(fragment) => write!(f, "invalid CWR ref fragment: {fragment}"),
            Self::QueryUnsupported => f.write_str("CWR refs do not use query strings"),
            Self::BackslashUnsupported => f.write_str("CWR refs do not use backslashes"),
            Self::EmptyRoot => f.write_str("ref root is empty"),
            Self::InvalidRoot(root) => write!(f, "invalid CWR ref root: {root}"),
            Self::InvalidCheckpoint(checkpoint) => {
                write!(f, "invalid CWR checkpoint: {checkpoint}")
            }
            Self::EmptySegment => f.write_str("ref path contains an empty segment"),
            Self::UnsafeSegment(segment) => {
                write!(f, "ref path contains unsafe segment: {segment}")
            }
            Self::InvalidPathSegment(segment) => {
                write!(f, "ref path contains an invalid segment: {segment}")
            }
            Self::CoworkerSubjectExpected => {
                f.write_str("ref must identify a canonical coworker subject")
            }
        }
    }
}

impl Error for CwrRefError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionSelectorError {
    Ref(CwrRefError),
    InvalidActionName(String),
}

impl fmt::Display for ActionSelectorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ref(error) => error.fmt(f),
            Self::InvalidActionName(name) => write!(
                f,
                "action name must contain exactly two hyphens between lowercase snake_case verb and object: {name}"
            ),
        }
    }
}

impl Error for ActionSelectorError {}

impl From<CwrRefError> for ActionSelectorError {
    fn from(value: CwrRefError) -> Self {
        Self::Ref(value)
    }
}

fn parse_mount(remainder: &str) -> Result<(CwrRoot, Option<String>, &str), CwrRefError> {
    if remainder.is_empty() {
        return Ok((CwrRoot::Mount, None, ""));
    }
    if let Some(path) = remainder.strip_prefix('/') {
        return Ok((CwrRoot::Mount, None, path));
    }
    Err(CwrRefError::InvalidRoot(format!("~@{remainder}")))
}

fn parse_ordinary_root(body: &str) -> Result<(CwrRoot, Option<String>, &str), CwrRefError> {
    let (root_with_checkpoint, path) = body.split_once('/').unwrap_or((body, ""));
    if root_with_checkpoint.is_empty() {
        return Err(CwrRefError::EmptyRoot);
    }
    let (root_text, checkpoint) = match root_with_checkpoint.split_once('@') {
        Some((root, checkpoint)) => {
            validate_checkpoint(checkpoint)?;
            (root, Some(checkpoint.to_owned()))
        }
        None => (root_with_checkpoint, None),
    };
    if root_text.is_empty() {
        return Err(CwrRefError::EmptyRoot);
    }
    let root = if root_text.starts_with('~') {
        return Err(CwrRefError::InvalidRoot(root_text.to_owned()));
    } else if let Some(key) = root_text.strip_prefix('!') {
        if key == "cwr" {
            CwrRoot::IntrinsicCwr
        } else if key.is_empty() || !key.chars().all(is_direct_key_char) {
            return Err(CwrRefError::InvalidRoot(root_text.to_owned()));
        } else {
            CwrRoot::DirectKey(key.to_owned())
        }
    } else if is_reverse_dns_name(root_text) {
        CwrRoot::Named(root_text.to_owned())
    } else {
        return Err(CwrRefError::InvalidRoot(root_text.to_owned()));
    };
    Ok((root, checkpoint, path))
}

fn validate_checkpoint(checkpoint: &str) -> Result<(), CwrRefError> {
    if checkpoint.is_empty()
        || !checkpoint
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | ':'))
    {
        return Err(CwrRefError::InvalidCheckpoint(checkpoint.to_owned()));
    }
    Ok(())
}

fn parse_segments(path: &str, live_workspace_source: bool) -> Result<Vec<String>, CwrRefError> {
    if path.is_empty() {
        return Ok(Vec::new());
    }
    path.split('/')
        .map(|segment| {
            if segment.is_empty() {
                return Err(CwrRefError::EmptySegment);
            }
            if matches!(segment, "." | "..") {
                return Err(CwrRefError::UnsafeSegment(segment.to_owned()));
            }
            if segment == "@" {
                if live_workspace_source {
                    return Ok(segment.to_owned());
                }
                return Err(CwrRefError::InvalidPathSegment(segment.to_owned()));
            }
            if !segment.chars().all(is_path_char) {
                return Err(CwrRefError::InvalidPathSegment(segment.to_owned()));
            }
            Ok(segment.to_owned())
        })
        .collect()
}

fn is_coworker_subject(reference: &CwrRef) -> bool {
    if reference.checkpoint.is_some() || reference.fragment.is_some() {
        return false;
    }
    match &reference.root {
        CwrRoot::IntrinsicCwr => reference.segments.is_empty(),
        CwrRoot::Mount => {
            reference.segments.len() == 2
                && reference.segments[0] == "coworkers"
                && is_snake_name(&reference.segments[1])
        }
        CwrRoot::Named(_) => reference.segments.is_empty(),
        CwrRoot::DirectKey(_) => false,
    }
}

fn is_reverse_dns_name(value: &str) -> bool {
    let labels: Vec<_> = value.split('.').collect();
    labels.len() >= 2
        && labels
            .into_iter()
            .all(|label| is_hostname_label(label) || is_snake_name(label))
}

pub(crate) fn is_hostname_label(value: &str) -> bool {
    let alphanumeric = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    !value.is_empty()
        && value.len() <= 63
        && value.as_bytes().first().copied().is_some_and(alphanumeric)
        && value.as_bytes().last().copied().is_some_and(alphanumeric)
        && value.bytes().all(|byte| alphanumeric(byte) || byte == b'-')
}

fn is_snake_name(value: &str) -> bool {
    let mut chars = value.chars();
    chars.next().is_some_and(|first| first.is_ascii_lowercase())
        && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
        && value.split('_').all(|part| !part.is_empty())
}

/// Returns whether `value` follows the shared SVO action-name grammar.
pub fn is_action_name(value: &str) -> bool {
    let mut parts = value.split("--");
    let Some(verb) = parts.next() else {
        return false;
    };
    let Some(object) = parts.next() else {
        return false;
    };
    parts.next().is_none() && is_snake_name(verb) && is_snake_name(object)
}

fn is_path_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-')
}

fn is_fragment_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | ':')
}

fn is_direct_key_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | ':')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_canonical_reference_roots_and_paths() {
        let mounted = CwrRef::parse("~@/coworkers/looper").unwrap();
        assert_eq!(mounted.root(), &CwrRoot::Mount);
        assert_eq!(mounted.segments(), ["coworkers", "looper"]);

        let named = CwrRef::parse("cwp://com.getworkloop.bfmono/apps/cwr").unwrap();
        assert_eq!(
            named.root(),
            &CwrRoot::Named("com.getworkloop.bfmono".into())
        );

        let intrinsic = CwrRef::parse("cwp://!cwr").unwrap();
        assert_eq!(intrinsic.root(), &CwrRoot::IntrinsicCwr);

        let direct = CwrRef::parse("cwp://!hashthing/path").unwrap();
        assert_eq!(direct.root(), &CwrRoot::DirectKey("hashthing".into()));
    }

    #[test]
    fn hostname_roots_accept_hyphens_and_digits_without_changing_action_names() {
        for root in [
            "local.randalls-macbook-pro",
            "local.3d-printer",
            "local.123",
            "com.old_namespace",
        ] {
            let reference = format!("cwp://{root}/runtimes/example");
            assert_eq!(
                CwrRef::parse(&reference).unwrap().root(),
                &CwrRoot::Named(root.into())
            );
        }
        for root in [
            "local.MacBook",
            "local.-mac",
            "local.mac-",
            "local..mac",
            "localhost",
        ] {
            assert!(
                CwrRef::parse(&format!("cwp://{root}/runtimes/example")).is_err(),
                "{root}"
            );
        }
        assert!(!is_action_name("send-now--message"));
        assert!(!is_action_name("3d--print"));
        assert!(is_action_name("send_now--message"));
    }

    #[test]
    fn rejects_retired_cwr_scheme() {
        let retired_ref = ["cwr", "://!cwr"].concat();
        let error = CwrRef::parse(&retired_ref).unwrap_err();

        assert_eq!(error, CwrRefError::InvalidScheme);
        assert_eq!(error.to_string(), "ref must start with ~@ or cwp://");
    }

    #[test]
    fn parses_resource_member_fragments_without_treating_them_as_actor_identity() {
        let member =
            CwrRef::parse("~@/policies/workmap_gates#rule_only_intents_and_policies_create_gates")
                .unwrap();
        assert_eq!(member.root(), &CwrRoot::Mount);
        assert_eq!(member.segments(), ["policies", "workmap_gates"]);
        assert_eq!(
            member.fragment(),
            Some("rule_only_intents_and_policies_create_gates")
        );

        assert!(CoworkerRef::parse("cwp://!cwr#cwr").is_err());
    }

    #[test]
    fn parses_checkpoints_only_on_absolute_refs() {
        let named = CwrRef::parse("cwp://com.getworkloop.bfmono@sha256:abc123/apps/cwr").unwrap();
        assert_eq!(named.checkpoint(), Some("sha256:abc123"));

        assert!(CwrRef::parse("~@sha256:abc123/apps/cwr").is_err());
    }

    #[test]
    fn accepts_only_the_reserved_at_path_segment() {
        CwrRef::parse(LIVE_WORKSPACE_SOURCE).unwrap();
        assert_eq!(
            CwrRef::parse("cwp://com.example.resource/path/@").unwrap_err(),
            CwrRefError::InvalidPathSegment("@".into())
        );
    }

    #[test]
    fn parses_canonical_coworker_subjects() {
        for reference in [
            "~@/coworkers/looper",
            "~@/coworkers/looper_2",
            "cwp://com.randallb.builder",
            "cwp://!cwr",
        ] {
            assert_eq!(CoworkerRef::parse(reference).unwrap().as_str(), reference);
        }
    }

    #[test]
    fn rejects_non_subject_and_retired_coworker_forms() {
        for reference in [
            "cwp://builtin/cwr",
            "cwp://!cwr/actions/send-message",
            "~@/coworkers/looper/actions/send-message",
            "~@sha256:abc/coworkers/looper",
            "~@/coworkers/2looper",
            "~@/coworkers/looper__2",
            "~@/coworkers/looper_",
            "cwp://~@/coworkers/looper",
            "cwp://~looper",
            "cwp://!hashthing",
            "~/coworkers/looper/COWORKER.toml",
        ] {
            assert!(CoworkerRef::parse(reference).is_err(), "{reference}");
        }
    }

    #[test]
    fn action_selector_keeps_subject_and_action_separate() {
        let selector = ActionSelector::parse("cwp://!cwr", "send--message").unwrap();
        assert_eq!(selector.coworker_ref().as_str(), "cwp://!cwr");
        assert_eq!(selector.action_name(), "send--message");

        let numbered = ActionSelector::parse("~@/coworkers/looper_2", "send_2--message_2").unwrap();
        assert_eq!(numbered.coworker_ref().as_str(), "~@/coworkers/looper_2");
        assert_eq!(numbered.action_name(), "send_2--message_2");
    }

    #[test]
    fn rejects_invalid_action_names() {
        for action_name in [
            "exec",
            "send_message",
            "send-message",
            "send--message--now",
            "Send--message",
            "send--Message",
            "2send--message",
            "send--2message",
            "send__2--message",
            "send--message__2",
            "send--",
        ] {
            assert!(
                ActionSelector::parse("cwp://!cwr", action_name).is_err(),
                "{action_name}"
            );
        }
    }

    #[test]
    fn rejects_invalid_fragments_queries_whitespace_and_unsafe_segments() {
        for reference in [
            "cwp://!cwr#",
            "cwp://!cwr#two#fragments",
            "cwp://!cwr#not/a/member",
            "cwp://!cwr?actor=cwr",
            " cwp://!cwr",
            "~@/../CWR.toml",
            "~@/coworkers//looper",
            "~@/",
            "cwp://com.randallb.builder/",
            "~@/coworkers/looper\\definition",
        ] {
            assert!(CwrRef::parse(reference).is_err(), "{reference}");
        }
    }
}
