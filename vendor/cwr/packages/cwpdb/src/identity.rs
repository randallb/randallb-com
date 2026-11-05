use super::{bind_with_retention, BindingRetention, CwpdbResult, RefId};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use sha2::{Digest, Sha256};

/// The durable semantic role and optional owner of a canonical reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityClaim {
    pub kind: String,
    pub owner_ref: Option<String>,
}

/// Return whether a canonical reference currently owns a node or relationship
/// record. A reference binding by itself is not a retained graph record.
pub fn record_exists(db: &Connection, reference: &str) -> CwpdbResult<bool> {
    Ok(db
        .prepare_cached(
            "SELECT EXISTS(
             SELECT 1 FROM ref_bindings b
             WHERE b.canonical_ref=?1 AND (
                 EXISTS(SELECT 1 FROM nodes n WHERE n.ref_id=b.ref_id)
                 OR EXISTS(SELECT 1 FROM edges e WHERE e.ref_id=b.ref_id)
             )
         )",
        )?
        .query_row([reference], |row| row.get::<_, i64>(0))?
        != 0)
}

/// Return the durable semantic identity claim for a canonical reference.
pub fn identity_claim(db: &Connection, reference: &str) -> CwpdbResult<Option<IdentityClaim>> {
    Ok(db
        .prepare_cached(
            "SELECT claim_kind,claim_owner_ref FROM ref_bindings
             WHERE canonical_ref=?1 AND claim_kind IS NOT NULL",
        )?
        .query_row([reference], |row| {
            Ok(IdentityClaim {
                kind: row.get(0)?,
                owner_ref: row.get(1)?,
            })
        })
        .optional()?)
}

/// Atomically claim a canonical reference for one semantic role and owner.
///
/// Repeating the same claim is idempotent. A different role or owner is
/// rejected, including when another graph writer already retained the ref.
pub fn claim_identity(
    db: &Transaction<'_>,
    reference: &str,
    kind: &str,
    owner_ref: &str,
) -> CwpdbResult<RefId> {
    if kind.is_empty() {
        return Err("identity claim kind must not be empty".into());
    }
    if identity_claim(db, reference)?.is_none() && record_exists(db, reference)? {
        return Err(format!("{reference} already identifies an unclaimed graph record").into());
    }
    let id = bind_with_retention(db, reference, BindingRetention::Durable)?;
    bind_with_retention(db, owner_ref, BindingRetention::Durable)?;
    db.execute(
        "UPDATE ref_bindings SET claim_kind=?2,claim_owner_ref=?3
         WHERE ref_id=?1 AND (claim_kind IS NULL OR (claim_kind=?2 AND claim_owner_ref IS NULL))",
        params![id, kind, owner_ref],
    )?;
    let claim = identity_claim(db, reference)?
        .ok_or_else(|| format!("identity claim was not retained for {reference}"))?;
    if claim.kind != kind || claim.owner_ref.as_deref() != Some(owner_ref) {
        return Err(format!(
            "{reference} is already claimed as {} by {}",
            claim.kind,
            claim.owner_ref.as_deref().unwrap_or("an unspecified owner")
        )
        .into());
    }
    Ok(id)
}

pub(super) fn claim_record_identity(
    db: &Transaction<'_>,
    reference: &str,
    kind: &str,
    retention: BindingRetention,
) -> CwpdbResult<RefId> {
    let id = bind_with_retention(db, reference, retention)?;
    match identity_claim(db, reference)? {
        Some(claim) if claim.kind == kind => Ok(id),
        Some(claim) => Err(format!(
            "{reference} is already claimed as {} by {}",
            claim.kind,
            claim.owner_ref.as_deref().unwrap_or("an unspecified owner")
        )
        .into()),
        None => {
            db.execute(
                "UPDATE ref_bindings SET claim_kind=?2
                 WHERE ref_id=?1 AND claim_kind IS NULL",
                params![id, kind],
            )?;
            Ok(id)
        }
    }
}

/// Create a deterministic derived reference from length-delimited input.
///
/// Derived identities make rebuilding a relationship reuse its original
/// binding rather than creating an identity based on update order.
pub fn derived_ref(kind: &str, parts: &[&str]) -> String {
    let mut hash = Sha256::new();
    for part in std::iter::once(kind).chain(parts.iter().copied()) {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part.as_bytes());
    }
    format!(
        "cwp://org.coworkerprotocol/cwpdb/derived/{}",
        hex::encode(hash.finalize())
    )
}
