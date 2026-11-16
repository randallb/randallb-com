use serde::{Deserialize, Serialize};

/// Attributed origin reconstructed by a trusted native host from its admitted
/// source invocation and installation approval, never from delegated task input.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DelegationSource {
    pub actor_ref: String,
    pub invocation_ref: String,
    pub installation_ref: String,
    pub release_digest: String,
}
