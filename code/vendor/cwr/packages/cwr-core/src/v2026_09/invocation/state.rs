use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    io,
};

use serde_json::json;

use crate::{refs::is_action_name, CwrRef, CwrResult};

use super::{control::*, model::*};

include!("state/core.rs");
include!("state/stop.rs");
include!("state/legacy.rs");
include!("state/validation.rs");
include!("state/transaction.rs");
