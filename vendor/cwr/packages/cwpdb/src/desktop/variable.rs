//! Complete variable reads and revision-checked writes, preserving retained envelopes.
use crate::{declared::EntityBinding, ref_read::RefRecord, CwpdbResult, NativeStore};
use serde_json::Value;
const DEFINITION: &str = "cwp://org.coworkerprotocol/cwr/entities/variable";
pub enum Request {
    Read {
        reference: String,
    },
    Create {
        reference: String,
        value: Value,
    },
    Replace {
        reference: String,
        expected_revision: String,
        value: Value,
    },
}
pub fn execute(
    store: &mut NativeStore,
    binding: &EntityBinding,
    request: Request,
) -> CwpdbResult<RefRecord> {
    if let Request::Read { reference } = &request {
        return store.graph_read(|db| binding.get_in_connection(db, DEFINITION, reference));
    }
    store.graph_transaction(|tx| match request {
        Request::Read { .. } => unreachable!(),
        Request::Create { reference, value } => {
            if reference == DEFINITION {
                return Err("entity ref is reserved for a definition".into());
            }
            binding.create_in_transaction(tx, DEFINITION, &reference, &value)
        }
        Request::Replace {
            reference,
            expected_revision,
            value,
        } => {
            let before = binding.get_in_connection(tx, DEFINITION, &reference)?;
            if before.revision.legacy_counter().to_string() != expected_revision {
                return Err("entity revision conflict".into());
            }
            binding.replace_in_transaction(tx, DEFINITION, &reference, &before.revision, &value)
        }
    })
}
