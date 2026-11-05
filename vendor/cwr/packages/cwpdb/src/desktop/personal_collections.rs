//! Internal Desktop operations for Workloop personal conversation collections.
use super::OperationError;
use crate::{
    declared::Catalog, personal_collections as collections, sidebar_pins::PinKey, NativeStore,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    ReadList {
        scope: collections::Scope,
    },
    ReadMembers {
        scope: collections::Scope,
        collection_ref: String,
    },
    Create {
        scope: collections::Scope,
        name: String,
        request_id: String,
    },
    Rename {
        scope: collections::Scope,
        collection_ref: String,
        name: String,
        request_id: String,
    },
    Delete {
        scope: collections::Scope,
        collection_ref: String,
        request_id: String,
    },
    MoveCollection {
        scope: collections::Scope,
        collection_ref: String,
        target_ref: String,
        before: bool,
        request_id: String,
    },
    MoveMember {
        scope: collections::Scope,
        collection_ref: String,
        key: PinKey,
        target: PinKey,
        before: bool,
        request_id: String,
    },
    SetMember {
        scope: collections::Scope,
        collection_ref: String,
        key: PinKey,
        present: bool,
        request_id: String,
    },
}

#[derive(Debug, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Response {
    List {
        snapshot: collections::ListSnapshot,
    },
    Members {
        snapshot: collections::MembersSnapshot,
    },
    Changed {
        collection_ref: String,
    },
    CommittedResultExpired,
}

pub fn is_write(request: &Request) -> bool {
    !matches!(
        request,
        Request::ReadList { .. } | Request::ReadMembers { .. }
    )
}

pub fn execute(
    store: &mut NativeStore,
    declarations: &Catalog,
    request: Request,
) -> Result<Response, OperationError> {
    let writing = is_write(&request);
    let work = |tx: &rusqlite::Transaction<'_>| match request {
        Request::ReadList { scope } => Ok(Response::List {
            snapshot: collections::read_list(declarations, tx, &scope)?,
        }),
        Request::ReadMembers {
            scope,
            collection_ref,
        } => Ok(Response::Members {
            snapshot: collections::read_members(declarations, tx, &scope, &collection_ref)?,
        }),
        Request::Create {
            scope,
            name,
            request_id,
        } => changed(
            declarations,
            tx,
            &scope,
            collections::Change::Create { name },
            &request_id,
        ),
        Request::Rename {
            scope,
            collection_ref,
            name,
            request_id,
        } => changed(
            declarations,
            tx,
            &scope,
            collections::Change::Rename {
                collection_ref,
                name,
            },
            &request_id,
        ),
        Request::Delete {
            scope,
            collection_ref,
            request_id,
        } => changed(
            declarations,
            tx,
            &scope,
            collections::Change::Delete { collection_ref },
            &request_id,
        ),
        Request::MoveCollection {
            scope,
            collection_ref,
            target_ref,
            before,
            request_id,
        } => changed(
            declarations,
            tx,
            &scope,
            collections::Change::MoveCollection {
                collection_ref,
                target_ref,
                before,
            },
            &request_id,
        ),
        Request::MoveMember {
            scope,
            collection_ref,
            key,
            target,
            before,
            request_id,
        } => changed(
            declarations,
            tx,
            &scope,
            collections::Change::MoveMember {
                collection_ref,
                key,
                target,
                before,
            },
            &request_id,
        ),
        Request::SetMember {
            scope,
            collection_ref,
            key,
            present,
            request_id,
        } => changed(
            declarations,
            tx,
            &scope,
            collections::Change::SetMember {
                collection_ref,
                key,
                present,
            },
            &request_id,
        ),
    };
    let result = if writing {
        store.graph_transaction(work)
    } else {
        store.graph_read_transaction(work)
    };
    result.map_err(super::domain_errors::map)
}

fn changed(
    declarations: &Catalog,
    tx: &rusqlite::Transaction<'_>,
    scope: &collections::Scope,
    change: collections::Change,
    request_id: &str,
) -> crate::CwpdbResult<Response> {
    match collections::mutate_once(declarations, tx, scope, change, request_id)? {
        Some(collection_ref) => Ok(Response::Changed { collection_ref }),
        None => Ok(Response::CommittedResultExpired),
    }
}

pub use crate::personal_collections::{Change, Collection, ListSnapshot, MembersSnapshot, Scope};
