//! Relative saved-pin moves and their receipt share the existing writer transaction.
use crate::declared::Catalog;
use crate::sidebar_pins::{self, PinKey, PinReplacement, PinScope};
use crate::{
    sequence_receipt::{self, Replay},
    CwpdbResult,
};
use rusqlite::Transaction;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Up,
    Down,
}

/// Eligibility is the caller's current authorization snapshot, not replay identity.
/// The stable semantic intent binds the owner, target and direction. A retained
/// result precedes neighbor lookup, so reordered or changed eligible sets cannot
/// turn result recovery into another relative move.
pub struct RelativeMove<'a> {
    pub owner_key: &'a str,
    pub key: &'a PinKey,
    pub direction: Direction,
    pub include_archived: bool,
    pub anchor: Option<(&'a PinKey, bool)>,
}
pub fn move_once_in_transaction(
    declarations: &Catalog,
    tx: &Transaction<'_>,
    scope: &PinScope,
    intent: RelativeMove<'_>,
    eligible: &[PinKey],
    request_id: &str,
) -> CwpdbResult<PinReplacement> {
    let RelativeMove {
        owner_key,
        key,
        direction,
        include_archived,
        anchor,
    } = intent;
    let reference = sidebar_pins::scope_ref(declarations, scope)?;
    if owner_key.len() > 16384 {
        return Err("invalid pin owner".into());
    }
    if !owner_key.is_empty() {
        declarations.validate_reference(owner_key)?;
    }
    sidebar_pins::target_ref(declarations, key)?;
    let mut args = json!({"operation":"move_relative","scope":scope,"owner_key":owner_key,"key":key,"direction":direction,"include_archived":include_archived});
    if let Some((target, before)) = anchor {
        sidebar_pins::target_ref(declarations, target)?;
        args["anchor"] = json!({"target":target,"before":before});
    }
    match sequence_receipt::check(tx, &reference, request_id, &args)? {
        Replay::Retained(value) => {
            return Ok(PinReplacement::Committed {
                snapshot: serde_json::from_value(value)?,
            })
        }
        Replay::CommittedResultExpired => return Ok(PinReplacement::CommittedResultExpired),
        Replay::NotCommitted => {}
    }
    if eligible.len() > 1000 || serde_json::to_vec(eligible)?.len() > 524288 {
        return Err("pin eligibility limit exceeded".into());
    }
    let allowed = eligible
        .iter()
        .map(|key| sidebar_pins::target_ref(declarations, key))
        .collect::<CwpdbResult<std::collections::BTreeSet<_>>>()?;
    if !allowed.contains(&sidebar_pins::target_ref(declarations, key)?) {
        return Err("pin target is not eligible for owner".into());
    }
    let before = sidebar_pins::read(declarations, tx, scope)?;
    let positions = before
        .members
        .iter()
        .enumerate()
        .filter_map(
            |(index, member)| match sidebar_pins::target_ref(declarations, member) {
                Ok(target) if allowed.contains(&target) => Some(Ok(index)),
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            },
        )
        .collect::<CwpdbResult<Vec<_>>>()?;
    let at = positions
        .iter()
        .position(|index| &before.members[*index] == key)
        .ok_or("pinned target changed; refresh sidebar")?;
    let mut members = before.members.clone();
    if let Some((target, insert_before)) = anchor {
        let target = positions
            .iter()
            .position(|index| &members[*index] == target)
            .ok_or("pinned destination is unavailable for owner")?;
        let mut ordered = positions
            .iter()
            .map(|index| members[*index].clone())
            .collect::<Vec<_>>();
        let mut destination = target + usize::from(!insert_before);
        if at < destination {
            destination -= 1;
        }
        let item = ordered.remove(at);
        ordered.insert(destination, item);
        for (index, item) in positions.iter().zip(ordered) {
            members[*index] = item;
        }
    } else {
        let neighbor = match direction {
            Direction::Up => at.checked_sub(1),
            Direction::Down => at.checked_add(1).filter(|index| *index < positions.len()),
        }
        .ok_or("pinned target changed; refresh sidebar")?;
        members.swap(positions[at], positions[neighbor]);
    }
    let snapshot =
        sidebar_pins::replace_in_transaction(declarations, tx, scope, &before.revision, &members)?;
    sequence_receipt::commit(
        tx,
        &reference,
        request_id,
        &args,
        &serde_json::to_value(&snapshot)?,
        &[],
    )?;
    Ok(PinReplacement::Committed { snapshot })
}

#[cfg(test)]
#[path = "sidebar_pin_move_tests.rs"]
mod tests;
