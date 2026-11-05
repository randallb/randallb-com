use std::collections::BTreeMap;

use super::*;

include!("sqlite/declared.rs");
include!("sqlite/completion_order.rs");
include!("sqlite/admissions.rs");

include!("sqlite/sync.rs");
#[cfg(test)]
include!("sqlite/sync_lifecycle_tests.rs");
include!("sqlite/commentary.rs");

#[cfg(test)]
#[path = "sqlite_upgrade_tests.rs"]
mod schema_upgrade_tests;

include!("sqlite/participant_versions.rs");

#[cfg(test)]
#[path = "sqlite/tools_tests.rs"]
mod tools_tests;

#[cfg(test)]
#[path = "sqlite/conversation_records_tests.rs"]
mod conversation_records_tests;

#[cfg(test)]
#[path = "sqlite/deferred_history_tests.rs"]
mod deferred_history_tests;
