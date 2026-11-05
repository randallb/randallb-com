//! Declared version request and response values.
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct File {
    pub path: String,
    pub data: String,
    pub mode: u32,
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Version {
    pub id: String,
    pub files: Vec<File>,
    pub folders: Vec<String>,
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Instance {
    pub id: String,
    pub name: String,
    pub folders: Option<Vec<String>>,
}
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionPolicy {
    Compare,
    IfAbsent,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    ReadInstance {
        root: String,
    },
    ReadVersion {
        root: String,
        version: Option<String>,
    },
    Recover {
        root: String,
        version: Option<String>,
    },
    Save {
        root: String,
        instance: Instance,
        expected_revision: Option<String>,
        request_id: String,
    },
    Rename {
        root: String,
        name: String,
        expected_revision: String,
        request_id: String,
    },
    Publish {
        root: String,
        version: Version,
        import: bool,
        request_id: String,
    },
    Import {
        root: String,
        versions: Vec<Version>,
        active: String,
        expected_active: Option<String>,
        request_id: String,
    },
    Select {
        root: String,
        version: String,
        expected_active: Option<String>,
        policy: SelectionPolicy,
        request_id: String,
    },
    Remove {
        root: String,
        expected_revision: String,
        request_id: String,
    },
}
impl Request {
    pub fn is_write(&self) -> bool {
        !matches!(self, Self::ReadInstance { .. } | Self::ReadVersion { .. })
    }
    pub(super) fn root(&self) -> &str {
        match self {
            Self::ReadInstance { root }
            | Self::ReadVersion { root, .. }
            | Self::Recover { root, .. }
            | Self::Save { root, .. }
            | Self::Rename { root, .. }
            | Self::Publish { root, .. }
            | Self::Import { root, .. }
            | Self::Select { root, .. }
            | Self::Remove { root, .. } => root,
        }
    }
    pub(super) fn request_id(&self) -> Option<&str> {
        match self {
            Self::Save { request_id, .. }
            | Self::Rename { request_id, .. }
            | Self::Publish { request_id, .. }
            | Self::Import { request_id, .. }
            | Self::Select { request_id, .. }
            | Self::Remove { request_id, .. } => Some(request_id),
            _ => None,
        }
    }
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum Response {
    Instance {
        instance: Option<Instance>,
        revision: Option<String>,
        membership_ready: bool,
    },
    Version {
        version: Option<Version>,
        membership_ready: bool,
    },
    Committed,
    CommittedResultExpired,
}
