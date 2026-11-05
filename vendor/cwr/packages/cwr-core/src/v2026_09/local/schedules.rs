/// A release declaration contains timing and an exact action contract, never an
/// owner. The native installer binds this to an installation server-side.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ScheduleDeclaration {
    pub declaration_ref: String,
    pub revision: String,
    pub target: ExactActionTarget,
    pub input: Value,
    pub anchor_unix_ms: u64,
    pub interval_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InstalledSchedule {
    pub schedule_ref: String,
    pub owner_context_ref: String,
    pub author_actor_ref: String,
    pub definition: ScheduleDeclaration,
    pub availability: String,
    pub paused: bool,
    pub next_due_unix_ms: u64,
    pub active_occurrence_ref: Option<String>,
    pub last_admission_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ScheduleOccurrence {
    pub occurrence_ref: String,
    pub schedule_ref: String,
    pub definition: ScheduleDeclaration,
    pub due_unix_ms: u64,
    pub outcome: String,
    pub invocation_ref: Option<String>,
    pub conversation_ref: Option<String>,
    pub archived: bool,
}

/// Owner is bound once by the authenticated host, never widened by schedule ref.
pub struct LocalSchedulesModule<'a> {
    runtime: &'a LocalRuntime,
    owner: String,
    actor: String,
    host_owner_only: bool,
}

impl LocalRuntime {
    /// Authorize the installed owner selected by a native operator surface.
    /// The actor comes from its authenticated session, not schedule input.
    pub fn require_installed_schedule_owner(&self, owner: &str, actor: &str) -> CwrResult<()> {
        let state = self.lock()?;
        require_installed_schedule_owner_in_state(&state, owner, actor)
    }

    /// Host-selected collection reads recheck owner policy inside each owning
    /// store transaction, so a context change cannot slip between selection
    /// and read.
    pub fn host_schedules_module_as(
        &self,
        owner: &str,
        actor: &str,
    ) -> CwrResult<LocalSchedulesModule<'_>> {
        let mut module = self.schedules_module_as(owner, actor)?;
        module.host_owner_only = true;
        Ok(module)
    }

    /// Trusted native embedding. Each operation checks current actor identity;
    /// the host is responsible for authenticating this installed-context scope.
    pub fn schedules_module_as(
        &self,
        owner: &str,
        actor: &str,
    ) -> CwrResult<LocalSchedulesModule<'_>> {
        if owner.trim().is_empty() || owner.len() > 2048 {
            return invalid("installed owner context is required");
        }
        validate_schedule_ref(actor)?;
        Ok(LocalSchedulesModule {
            runtime: self,
            owner: owner.into(),
            actor: actor.into(),
            host_owner_only: false,
        })
    }
}

fn require_installed_schedule_owner_in_state(
    state: &LocalRuntimeState,
    owner: &str,
    actor: &str,
) -> CwrResult<()> {
    if owner.trim().is_empty() || owner.len() > 2048 {
        return invalid("installed owner context is required");
    }
    local_catalog(&state.graph, actor)?;
    if actor != state.host_identity.acting_actor_ref {
        return invalid("installed schedule controls require the selected host actor");
    }
    if !state
        .admission
        .context_policy
        .as_ref()
        .is_some_and(|policy| {
            owner != policy.default_context_ref && policy.contexts.contains_key(owner)
        })
    {
        return invalid("installed owner context is unavailable");
    }
    Ok(())
}

fn validate_schedule_ref(reference: &str) -> CwrResult<()> {
    let parsed = CwrRef::parse(reference)?;
    if reference.starts_with("~@") || parsed.fragment().is_some() {
        return invalid("schedule binding requires absolute unfragmented refs");
    }
    Ok(())
}

fn next_schedule_tick(definition: &ScheduleDeclaration, now: u64) -> CwrResult<u64> {
    if definition.interval_ms < 1000 {
        return invalid("schedule interval must be at least 1000 milliseconds");
    }
    if now < definition.anchor_unix_ms {
        return Ok(definition.anchor_unix_ms);
    }
    let ticks = (now - definition.anchor_unix_ms) / definition.interval_ms;
    ticks
        .checked_add(1)
        .and_then(|ticks| ticks.checked_mul(definition.interval_ms))
        .and_then(|elapsed| definition.anchor_unix_ms.checked_add(elapsed))
        .ok_or_else(|| invalid_error("schedule deadline overflow"))
}

fn schedule_identity(kind: &str, value: impl Serialize) -> CwrResult<String> {
    Ok(format!(
        "cwp://org.coworkerprotocol.local/{kind}/{}",
        hex::encode(Sha256::digest(serde_json::to_vec(&value)?))
    ))
}

include!("schedules/storage.rs");
include!("schedules/controls.rs");

include!("schedules/worker.rs");

include!("schedules/actions.rs");


pub const SCHEDULES_SERVICE_REF: &str = "cwp://org.coworkerprotocol/cwr/services/schedules";

impl LocalRuntime {
    /// Imported binding follows the conversations module's exact-contract rule.
    /// The native host supplies an authenticated installed owner, not action input.
    pub fn imported_schedules_module(&self, owner: &str) -> CwrResult<LocalSchedulesModule<'_>> {
        let actor = {
            let state = self.lock()?;
            for source in [
                include_str!("../../../resources/base/schedules/service.toml"),
                include_str!("../../../resources/base/schedules/relationships.toml"),
                include_str!(
                    "../../../resources/base/schedules/definitions/installed-schedule.toml"
                ),
                include_str!(
                    "../../../resources/base/schedules/definitions/schedule-occurrence.toml"
                ),
                include_str!("../../../resources/base/schedules/definitions/schedule-owner.toml"),
            ] {
                let expected: Value = toml::from_str(source)?;
                if !state.graph.resources.iter().any(|r| r.document == expected) {
                    return invalid(
                        "import the exact supported schedules module before binding its service",
                    );
                }
            }
            state.host_identity.acting_actor_ref.clone()
        };
        self.schedules_module_as(owner, &actor)
    }
}
