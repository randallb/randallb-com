#[derive(Debug, Clone, Serialize)]
pub struct ResolvedCwrGraph {
    #[serde(rename = "type")]
    pub type_ref: String,
    #[serde(rename = "ref")]
    pub reference: String,
    pub runtime_ref: String,
    pub permissions: RetainedDataPermissions,
    pub resources: Vec<ResolvedCwrResource>,
    pub actors: Vec<ResolvedCwrActor>,
    pub actor_integrations: Vec<ResolvedCwrActorIntegration>,
    pub routes: Vec<ResolvedCwrActionRoute>,
    pub gateways: Vec<ResolvedGateway>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<GraphLoadWarning>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResolvedCwrResource {
    pub source: PathBuf,
    pub scope_ref: String,
    #[serde(rename = "type")]
    pub type_ref: String,
    #[serde(rename = "ref")]
    pub reference: String,
    pub visibility: ResolvedCwrVisibility,
    pub document: Value,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResolvedCwrVisibility {
    Import,
    Export,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ResolvedCwrActorIntegration {
    pub actor_ref: String,
    pub actor_name: Option<String>,
    pub coworker_ref: String,
    pub coworker_source: PathBuf,
    pub working_path: PathBuf,
    pub actor_integration_ref: String,
    pub actor_integration_source: PathBuf,
    pub message_mode: ActorIntegrationMessageMode,
    pub integration_ref: String,
    pub integration_source: PathBuf,
    pub adapter_ref: String,
    pub adapter_name: String,
    pub available_authority: Vec<String>,
    pub missing_required_authority: Vec<String>,
    pub adapter_params: Value,
    pub integration_params: Value,
    pub actor_integration_params: Value,
    #[serde(skip)]
    pub input_uses: Vec<crate::input_delivery::InputUse>,
    #[serde(skip)]
    pub input_plan: crate::input_delivery::InvocationInputs,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActorIntegrationMessageMode {
    #[default]
    Conversational,
    ActionsOnly,
}

impl ResolvedCwrActorIntegration {
    pub fn is_available(&self) -> bool {
        self.missing_required_authority.is_empty()
    }

    pub fn require_available(&self) -> CwrResult<()> {
        if self.is_available() {
            return Ok(());
        }
        invalid(format!(
            "actor integration {} is limited; missing required authority: {}",
            self.actor_integration_ref,
            self.missing_required_authority.join(", ")
        ))
    }

    pub fn process_adapter_route(&self) -> CwrResult<crate::adapter::ResolvedProcessAdapterRoute> {
        let coworker_directory = self
            .coworker_source
            .parent()
            .ok_or_else(|| invalid_error("coworker resource has no parent directory"))?;
        Ok(crate::adapter::ResolvedProcessAdapterRoute::new(
            self.adapter_ref.clone(),
            &self.working_path,
            coworker_directory,
            self.adapter_params.clone(),
            self.integration_params.clone(),
            self.actor_integration_params.clone(),
        )
        .with_authority(self.available_authority.clone()))
    }

    pub fn project_adapter_invocation(
        &self,
        invocation_ref: impl Into<String>,
        action_name: impl Into<String>,
        input: Value,
        output_schema: Value,
        thread: Value,
    ) -> crate::adapter::AdapterInvocation {
        let mut invocation = crate::adapter::AdapterInvocation::new(
            invocation_ref,
            action_name,
            input,
            output_schema,
        );
        invocation.integration_ref = Some(self.integration_ref.clone());
        invocation.local_inputs = Some(self.input_plan.admit(&self.input_uses));
        invocation.conversation_continuity = invocation.action_name
            == crate::action_contract::RESPOND_TO_MESSAGE_ACTION_NAME
            && matches!(
                self.adapter_ref.as_str(),
                crate::adapter::CODEX_ADAPTER_REF | crate::adapter::CLAUDE_ADAPTER_REF
            );
        invocation.thread = thread;
        invocation.adapter_params = self.adapter_params.clone();
        invocation.integration_params = self.integration_params.clone();
        invocation.actor_integration_params = self.actor_integration_params.clone();
        invocation.authority = self.available_authority.clone();
        if let Ok(route) = self.process_adapter_route() {
            let mut route = route;
            if route.retain_input_slots(&mut invocation).is_err() {
                invocation.local_inputs = None;
            }
            invocation.route_fingerprint = Some(route.fingerprint());
            invocation.provider_fingerprint = Some(route.provider_fingerprint());
            invocation.retained_route = Some(route);
        }
        invocation
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ResolvedCwrActionRoute {
    pub role_ref: String,
    pub contract_revision: String,
    pub execution_scope: crate::action_contract::ExecutionScope,
    pub response_mode: crate::action_contract::ResponseMode,
    pub actor_ref: String,
    pub coworker_ref: String,
    pub coworker_source: PathBuf,
    pub working_path: PathBuf,
    pub action_name: String,
    pub action_title: String,
    pub action_description: String,
    pub input_schema: Value,
    pub output_schema: Value,
    pub actor_integration_ref: String,
    pub actor_integration_source: PathBuf,
    pub integration_ref: String,
    pub integration_source: PathBuf,
    pub adapter_ref: String,
    pub available_authority: Vec<String>,
    pub missing_required_authority: Vec<String>,
    pub adapter_params: Value,
    pub integration_params: Value,
    pub actor_integration_params: Value,
    #[serde(skip)]
    pub input_uses: Vec<crate::input_delivery::InputUse>,
    #[serde(skip)]
    pub input_plan: crate::input_delivery::InvocationInputs,
}

impl ResolvedCwrActionRoute {
    pub fn is_available(&self) -> bool {
        self.missing_required_authority.is_empty()
    }

    pub fn require_available(&self) -> CwrResult<()> {
        if self.is_available() {
            return Ok(());
        }
        invalid(format!(
            "action route {} on actor integration {} is limited; missing required authority: {}",
            self.action_name,
            self.actor_integration_ref,
            self.missing_required_authority.join(", ")
        ))
    }

    pub fn process_adapter_route(&self) -> CwrResult<crate::adapter::ResolvedProcessAdapterRoute> {
        let coworker_directory = self
            .coworker_source
            .parent()
            .ok_or_else(|| invalid_error("coworker resource has no parent directory"))?;
        Ok(crate::adapter::ResolvedProcessAdapterRoute::new(
            self.adapter_ref.clone(),
            &self.working_path,
            coworker_directory,
            self.adapter_params.clone(),
            self.integration_params.clone(),
            self.actor_integration_params.clone(),
        )
        .with_authority(self.available_authority.clone()))
    }

    pub fn validate_input(&self, input: &Value) -> CwrResult<()> {
        let validator = json_schema_validator(&self.input_schema).map_err(|error| {
            invalid_error(format!(
                "action input schema for {} is not valid JSON Schema: {error}",
                self.action_name
            ))
        })?;
        if let Some(error) = validator.iter_errors(input).next() {
            let location = error.instance_path().as_str();
            let location = if location.is_empty() { "/" } else { location };
            return invalid(format!(
                "action input for {} does not satisfy its schema at {location}: {error}",
                self.action_name
            ));
        }
        Ok(())
    }

    pub fn project_adapter_invocation(
        &self,
        invocation_ref: impl Into<String>,
        input: Value,
        thread: Value,
    ) -> CwrResult<crate::adapter::AdapterInvocation> {
        self.validate_input(&input)?;
        let mut invocation = crate::adapter::AdapterInvocation::new(
            invocation_ref,
            self.action_name.clone(),
            input,
            self.output_schema.clone(),
        );
        invocation.integration_ref = Some(self.integration_ref.clone());
        invocation.local_inputs = Some(self.input_plan.admit(&self.input_uses));
        invocation.thread = thread;
        invocation.adapter_params = self.adapter_params.clone();
        invocation.integration_params = self.integration_params.clone();
        invocation.actor_integration_params = self.actor_integration_params.clone();
        invocation.authority = self.available_authority.clone();
        let mut route = self.process_adapter_route()?;
        route.retain_input_slots(&mut invocation)?;
        invocation.route_fingerprint = Some(route.fingerprint());
        invocation.provider_fingerprint = Some(route.provider_fingerprint());
        invocation.retained_route = Some(route);
        invocation.execution_scope = self.execution_scope;
        invocation.response_mode = self.response_mode;
        Ok(invocation)
    }
}


/// Configuration failures without parameter values or credential contents.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraphLoadWarning {
    pub source: PathBuf,
    pub message: String,
}

/// Host-owned policy from the entry manifest; imported declarations cannot widen it.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RetainedDataPermissions {
    #[serde(default)]
    pub read_retained_data: RetainedDataReadAccess,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RetainedDataReadAccess {
    #[default]
    Participants,
    All,
}

impl RetainedDataPermissions {
    pub fn allows_all(&self) -> bool {
        self.read_retained_data == RetainedDataReadAccess::All
    }

    pub fn description(&self) -> &'static str {
        if self.allows_all() {
            "Retained data: readable by all callers in this runtime."
        } else {
            "Retained data: conversation reads require participation."
        }
    }
}
