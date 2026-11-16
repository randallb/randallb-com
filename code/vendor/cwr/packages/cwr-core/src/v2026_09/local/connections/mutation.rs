use super::*;
use crate::{credentials::SecretStore, workspace_environment::EntryMetadata};

pub fn save_project_connections(
    path: &Path,
    fingerprint: &str,
    revision: &str,
    assignments: BTreeMap<String, String>,
) -> CwrResult<ProjectConnections> {
    save_with(path, fingerprint, revision, assignments, |id| {
        Ok(crate::workspace_environment::workspace_secret_store()?.contains(id)?)
    })
}

pub(super) fn save_with(
    path: &Path,
    fingerprint: &str,
    revision: &str,
    assignments: BTreeMap<String, String>,
    mut secret_exists: impl FnMut(&str) -> CwrResult<bool>,
) -> CwrResult<ProjectConnections> {
    let target = connection_target(path)?;
    let definition = definition_for_target(&target)?;
    if definition.fingerprint != fingerprint {
        return invalid("This project changed. Review its connections again before saving.");
    }
    let mut locks = acquire_grants_lock(&target.grants_path)?;
    let mut grants = read_grants(&target.grants_path)?;
    reject_stale_grants(&grants, revision)?;
    let mut next = BTreeMap::new();
    let mut changed = Vec::new();
    for (alias, key_id) in assignments {
        let declaration = definition
            .requirements
            .iter()
            .find(|r| r.alias == alias && workspace_kind(r).is_some())
            .ok_or("Choose a declared workspace input.")?;
        if let Some(previous) = grants
            .connections
            .get(&alias)
            .filter(|g| g.fingerprint == fingerprint && g.key_id == key_id)
        {
            next.insert(alias, previous.clone());
            continue;
        }
        let backend = if crate::credentials::validate_id(&key_id).is_ok() {
            Backend::Legacy
        } else {
            Backend::Workspace
        };
        if backend == Backend::Legacy && declaration.type_ref != SECRET_TYPE {
            return invalid("Choose a workspace variable for this input.");
        }
        changed.push(ChangedAssignment {
            reference: key_id.clone(),
            backend,
            kind: workspace_kind(declaration).ok_or("Choose a declared workspace input.")?,
        });
        next.insert(
            alias,
            Grant {
                backend,
                key_id,
                fingerprint: fingerprint.into(),
                token: format!("{PREFIX}{}", Uuid::new_v4()),
            },
        );
    }
    // Removing assignments never needs their old store. Unchanged assignments
    // likewise cannot prevent revoking another input when that store is offline.
    let mut access = AssignmentAccess::open(&target, &changed)?;
    let entries = access.validate(&changed)?;
    if entries.iter().flatten().any(|m| m.credential_id.is_some()) {
        // Native access can block. Let removal/replacement finish while it runs,
        // then check the same entry revisions and grant snapshot before saving.
        drop((access, locks));
        for metadata in entries.iter().flatten() {
            if let Some(id) = &metadata.credential_id {
                if !secret_exists(id).unwrap_or(false) {
                    return invalid(
                        "The workspace secret is unavailable. Unlock Keychain or replace it.",
                    );
                }
            }
        }
        locks = acquire_grants_lock(&target.grants_path)?;
        grants = read_grants(&target.grants_path)?;
        reject_stale_grants(&grants, revision)?;
        access = AssignmentAccess::open(&target, &changed)?;
        if access.validate(&changed)? != entries {
            return invalid("Workspace entries changed. Review the latest choices before saving.");
        }
    }
    // Execution rechecks this definition. Do not hold revocation coordination
    // across source reads from a potentially offline workspace mount.
    grants.connections = next;
    for alias in grants.connections.keys() {
        grants.plaintext.remove(alias);
    }
    write_grants(&target.grants_path, &grants)?;
    drop((access, locks));
    let resolved = resolve(&target, &definition, &grants, None)?;
    Ok(report(&target, &definition, &grants, resolved.rows))
}

struct ChangedAssignment {
    reference: String,
    backend: Backend,
    kind: EnvironmentEntryKind,
}

struct AssignmentAccess {
    environment: Option<EnvironmentAccess>,
    legacy: Option<LegacyAccess>,
}

impl AssignmentAccess {
    fn open(target: &ConnectionTarget, changed: &[ChangedAssignment]) -> CwrResult<Self> {
        Ok(Self {
            environment: changed
                .iter()
                .any(|c| c.backend == Backend::Workspace)
                .then(|| EnvironmentAccess::open_canonical(&target.workspace, || false))
                .transpose()?,
            legacy: changed
                .iter()
                .any(|c| c.backend == Backend::Legacy)
                .then(|| LegacyAccess::open(|| false))
                .transpose()?,
        })
    }

    fn validate(&self, changed: &[ChangedAssignment]) -> CwrResult<Vec<Option<EntryMetadata>>> {
        changed
            .iter()
            .map(|assignment| match assignment.backend {
                Backend::Workspace => {
                    let metadata = self
                        .environment
                        .as_ref()
                        .and_then(|db| db.metadata(&assignment.reference).ok())
                        .filter(|m| m.kind == assignment.kind)
                        .ok_or("Choose an available entry of the declared kind.")?;
                    Ok(Some(metadata))
                }
                Backend::Legacy
                    if self
                        .legacy
                        .as_ref()
                        .is_some_and(|keys| keys.contains(&assignment.reference)) =>
                {
                    Ok(None)
                }
                _ => invalid("Choose an available entry of the declared kind."),
            })
            .collect()
    }
}

pub fn save_plaintext_connection(
    path: &Path,
    fingerprint: &str,
    revision: &str,
    alias: &str,
    value: Value,
) -> CwrResult<ProjectConnections> {
    if !value.is_object() {
        return invalid("Local configuration must be a JSON object.");
    }
    let target = connection_target(path)?;
    let definition = definition_for_target(&target)?;
    if definition.fingerprint != fingerprint {
        return invalid("This project changed. Review its connections again before saving.");
    }
    if !definition
        .requirements
        .iter()
        .any(|r| r.alias == alias && r.type_ref == PLAINTEXT_TYPE && workspace_kind(r).is_none())
    {
        return invalid("Choose a local configuration input.");
    }
    let locks = acquire_grants_lock(&target.grants_path)?;
    let mut grants = read_grants(&target.grants_path)?;
    reject_stale_grants(&grants, revision)?;
    grants.connections.remove(alias);
    grants.plaintext.insert(
        alias.into(),
        PlaintextGrant {
            fingerprint: fingerprint.into(),
            value,
        },
    );
    write_grants(&target.grants_path, &grants)?;
    drop(locks);
    let resolved = resolve(&target, &definition, &grants, None)?;
    Ok(report(&target, &definition, &grants, resolved.rows))
}
