//! Sidebar domain operations on the host's existing NativeStore.
use super::OperationError;
use crate::{
    declared::Catalog,
    sidebar_pin_move::{self, RelativeMove},
    sidebar_pins::{self},
    NativeStore,
};
use serde::{Deserialize, Serialize};
#[derive(Debug, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Read {
        scope: PinScope,
    },
    MoveRelative {
        scope: PinScope,
        owner_key: String,
        key: PinKey,
        direction: Direction,
        #[serde(default)]
        anchor: Option<Anchor>,
        #[serde(default)]
        include_archived: bool,
        eligible: Vec<PinKey>,
        request_id: String,
    },
    Replace {
        scope: PinScope,
        expected_revision: String,
        members: Vec<PinKey>,
        request_id: String,
    },
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Anchor {
    target: PinKey,
    before: bool,
}
#[derive(Debug, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Response {
    Snapshot { snapshot: PinSnapshot },
    CommittedResultExpired,
}
pub fn is_write(request: &Request) -> bool {
    !matches!(request, Request::Read { .. })
}
pub fn execute(
    store: &mut NativeStore,
    declarations: &Catalog,
    request: Request,
) -> Result<Response, OperationError> {
    let writing = is_write(&request);
    let work = |tx: &rusqlite::Transaction<'_>| match request {
        Request::Read { scope } => Ok(Response::Snapshot {
            snapshot: sidebar_pins::read(declarations, tx, &scope)?,
        }),
        Request::MoveRelative {
            scope,
            owner_key,
            key,
            direction,
            anchor,
            include_archived,
            eligible,
            request_id,
        } => {
            match sidebar_pin_move::move_once_in_transaction(
                declarations,
                tx,
                &scope,
                RelativeMove {
                    owner_key: &owner_key,
                    key: &key,
                    direction,
                    include_archived,
                    anchor: anchor.as_ref().map(|a| (&a.target, a.before)),
                },
                &eligible,
                &request_id,
            )? {
                sidebar_pins::PinReplacement::Committed { snapshot } => {
                    Ok(Response::Snapshot { snapshot })
                }
                sidebar_pins::PinReplacement::CommittedResultExpired => {
                    Ok(Response::CommittedResultExpired)
                }
            }
        }
        Request::Replace {
            scope,
            expected_revision,
            members,
            request_id,
        } => {
            match sidebar_pins::replace_once_in_transaction(
                declarations,
                tx,
                &scope,
                &expected_revision,
                &members,
                &request_id,
            )? {
                sidebar_pins::PinReplacement::Committed { snapshot } => {
                    Ok(Response::Snapshot { snapshot })
                }
                sidebar_pins::PinReplacement::CommittedResultExpired => {
                    Ok(Response::CommittedResultExpired)
                }
            }
        }
    };
    let result = if writing {
        store.graph_transaction(work)
    } else {
        store.graph_read_transaction(work)
    };
    result.map_err(super::domain_errors::map)
}

pub use crate::{
    sidebar_pin_move::Direction,
    sidebar_pins::{PinKey, PinScope, PinSnapshot, RevisionConflict},
};

#[cfg(test)]
mod tests {
    use super::*;
    fn execute_fixture(
        store: &mut NativeStore,
        request: Request,
    ) -> Result<Response, OperationError> {
        execute(store, &super::super::test_catalog().unwrap(), request)
    }
    use serde_json::json;
    #[test]
    fn module_replays_replace_and_preserves_state_on_stale_write() {
        let mut store = NativeStore::open_in_memory().unwrap();
        let input = json!({"operation":"replace","scope":{"workspace":"/fixture","viewer":"cwp://com.example/viewer"},"expected_revision":"0","members":[{"sourceRef":"cwp://com.example/source","ref":"cwp://com.example/conversation"}],"request_id":"pin-request"});
        let first = serde_json::to_value(
            execute_fixture(&mut store, serde_json::from_value(input.clone()).unwrap()).unwrap(),
        )
        .unwrap();
        let replay = serde_json::to_value(
            execute_fixture(&mut store, serde_json::from_value(input.clone()).unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(first, replay);
        let mut stale = input;
        stale["request_id"] = json!("stale-request");
        assert!(execute_fixture(&mut store, serde_json::from_value(stale).unwrap()).is_err());
        let read = json!({"operation":"read","scope":{"workspace":"/fixture","viewer":"cwp://com.example/viewer"}});
        assert_eq!(
            serde_json::to_value(
                execute_fixture(&mut store, serde_json::from_value(read).unwrap()).unwrap()
            )
            .unwrap(),
            first
        );
    }
}
