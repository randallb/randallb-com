use std::{
    collections::BTreeMap,
    io,
    sync::{Arc, Mutex},
};

use cwr::CwrResult;

pub(super) const MAX_IN_FLIGHT_HOST_REQUESTS: usize = 256;

#[derive(Clone)]
pub(super) struct PendingRequest {
    conversation_ref: Option<String>,
    include_completed_work_refs: bool,
    summary_version: u32,
    terminal_claimed: bool,
    cancel_on_disconnect: bool,
}

pub(super) struct TerminalClaim {
    pub(super) conversation_ref: Option<String>,
    pub(super) include_completed_work_refs: bool,
    pub(super) include_sidebar_metadata: bool,
}

pub(super) type SharedPendingRequests = Arc<Mutex<BTreeMap<String, PendingRequest>>>;

pub(super) fn registry() -> SharedPendingRequests {
    Arc::new(Mutex::new(BTreeMap::new()))
}

pub(super) fn reserve_request(
    pending: &SharedPendingRequests,
    request_id: &str,
    conversation_ref: Option<String>,
    include_completed_work_refs: bool,
) -> CwrResult<()> {
    if matches!(
        request_id,
        super::INTERNAL_SNAPSHOT_REQUEST_ID
            | super::INTERNAL_DESKTOP_SNAPSHOT_REQUEST_ID
            | super::INTERNAL_TRANSFER_PROBE_REQUEST_ID
            | super::INTERNAL_FORK_PROBE_REQUEST_ID
            | super::INTERNAL_BUILD_IDENTITY_REQUEST_ID
    ) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("host request ID {request_id} is reserved"),
        )
        .into());
    }
    let mut pending = lock(pending)?;
    if pending.contains_key(request_id) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("host request ID {request_id} is already in flight"),
        )
        .into());
    }
    if pending.len() >= MAX_IN_FLIGHT_HOST_REQUESTS {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            format!("host already has {MAX_IN_FLIGHT_HOST_REQUESTS} requests in flight"),
        )
        .into());
    }
    pending.insert(
        request_id.into(),
        PendingRequest {
            conversation_ref,
            include_completed_work_refs,
            summary_version: 1,
            terminal_claimed: false,
            cancel_on_disconnect: false,
        },
    );
    Ok(())
}

pub(super) fn mark_cancel_on_disconnect(
    pending: &SharedPendingRequests,
    request_id: &str,
) -> CwrResult<()> {
    if let Some(request) = lock(pending)?.get_mut(request_id) {
        request.cancel_on_disconnect = true;
    }
    Ok(())
}

pub(super) fn disconnect_cancellations(pending: &SharedPendingRequests) -> CwrResult<Vec<String>> {
    Ok(lock(pending)?
        .iter()
        .filter_map(|(request_id, request)| {
            (request.cancel_on_disconnect && !request.terminal_claimed)
                .then_some(request_id.clone())
        })
        .collect())
}

pub(super) fn claim_terminal(
    pending: &SharedPendingRequests,
    request_id: &str,
) -> CwrResult<Option<TerminalClaim>> {
    let mut pending = lock(pending)?;
    let Some(request) = pending.get_mut(request_id) else {
        return Ok(None);
    };
    if request.terminal_claimed {
        return Ok(None);
    }
    request.terminal_claimed = true;
    Ok(Some(TerminalClaim {
        conversation_ref: request.conversation_ref.clone(),
        include_completed_work_refs: request.include_completed_work_refs,
        include_sidebar_metadata: request.summary_version == 3,
    }))
}

pub(super) fn complete_terminal(
    pending: &SharedPendingRequests,
    request_id: &str,
) -> CwrResult<()> {
    lock(pending)?.remove(request_id);
    Ok(())
}

pub(super) fn is_empty(pending: &SharedPendingRequests) -> CwrResult<bool> {
    Ok(lock(pending)?.is_empty())
}

pub(super) fn take_unclaimed(pending: &SharedPendingRequests) -> CwrResult<Vec<String>> {
    let mut pending = lock(pending)?;
    let request_ids = pending
        .iter()
        .filter_map(|(request_id, request)| {
            (!request.terminal_claimed).then_some(request_id.clone())
        })
        .collect::<Vec<_>>();
    for request_id in &request_ids {
        pending.remove(request_id);
    }
    Ok(request_ids)
}

// New summary metadata is opt-in; versions one and two keep their wire shape.
pub(super) fn mark_sidebar_metadata(
    pending: &SharedPendingRequests,
    request_id: &str,
) -> CwrResult<()> {
    if let Some(request) = lock(pending)?.get_mut(request_id) {
        request.summary_version = 3;
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn len(pending: &SharedPendingRequests) -> usize {
    pending.lock().unwrap().len()
}

#[cfg(test)]
pub(super) fn conversation_ref(
    pending: &SharedPendingRequests,
    request_id: &str,
) -> Option<Option<String>> {
    pending
        .lock()
        .unwrap()
        .get(request_id)
        .map(|request| request.conversation_ref.clone())
}

fn lock(
    pending: &SharedPendingRequests,
) -> CwrResult<std::sync::MutexGuard<'_, BTreeMap<String, PendingRequest>>> {
    pending
        .lock()
        .map_err(|_| io::Error::other("runtime bridge request registry was poisoned").into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_takes_only_requests_without_a_terminal_publication_claim() {
        let pending = registry();
        reserve_request(&pending, "publishing", Some("conversation".into()), false).unwrap();
        reserve_request(&pending, "unanswered", None, false).unwrap();

        let claim = claim_terminal(&pending, "publishing")
            .unwrap()
            .expect("terminal publication claim");

        assert_eq!(claim.conversation_ref.as_deref(), Some("conversation"));
        assert_eq!(take_unclaimed(&pending).unwrap(), ["unanswered"]);
        assert_eq!(len(&pending), 1);
        complete_terminal(&pending, "publishing").unwrap();
        assert!(is_empty(&pending).unwrap());
    }
}
