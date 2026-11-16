#[derive(Debug, Clone, Deserialize)]
struct ManifestFile {
    #[serde(rename = "type")]
    type_ref: String,
    #[serde(default)]
    imports: Vec<ResourceObject>,
    #[serde(default)]
    permissions: RetainedDataPermissions,
    #[serde(default)]
    exports: Vec<ResourceObject>,
    input_schema: Option<SchemaLocator>,
    #[serde(default)]
    inputs: Vec<InputDeclaration>,
    #[serde(default)]
    volumes: Vec<VolumeDeclaration>,
}

#[derive(Debug, Clone, Deserialize)]
struct ResourceObject {
    path: Option<String>,
    #[serde(rename = "ref")]
    reference: Option<String>,
    #[serde(rename = "as")]
    alias: Option<String>,
    #[serde(default = "empty_object")]
    inputs: Value,
}

#[derive(Debug, Clone, Deserialize)]
struct CoworkerFile {
    #[serde(default)]
    message_binding_preference: Vec<String>,
    name: Option<String>,
    slug: Option<String>,
    #[serde(default)]
    authority: Vec<String>,
    #[serde(default)]
    imports: Vec<CoworkerImport>,
    #[serde(default)]
    roles: Vec<ResourceObject>,
    #[serde(default)]
    integrations: Vec<IntegrationBinding>,
    #[serde(default)]
    native_tools: Vec<NativeToolFile>,
}

#[derive(Debug, Clone, Deserialize)]
struct CoworkerImport {
    #[serde(flatten)]
    locator: ResourceObject,
    #[serde(default)]
    default_conversation_target: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct NativeToolFile {
    coworker: String,
    action: String,
    binding: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct GatewayFile {
    #[serde(default)]
    authority: Vec<String>,
    role: ResourceObject,
    integration: ResourceObject,
    action_name: String,
    ingress_action_name: Option<String>,
    delivery_action_name: Option<String>,
    connection_ref: Option<String>,
    #[serde(default)]
    params: Parameters,
}

#[derive(Debug, Clone, Deserialize)]
struct RoleFile {
    #[serde(default)]
    instructions: Vec<SchemaLocator>,
    #[serde(default)]
    required_authority: Vec<String>,
    #[serde(default)]
    adapters: Vec<AdapterBinding>,
    #[serde(default)]
    actions: Vec<ActionFile>,
}

#[derive(Debug, Clone, Deserialize)]
struct AdapterBinding {
    #[serde(flatten)]
    locator: ResourceObject,
    #[serde(default)]
    params: Parameters,
}

#[derive(Debug, Clone, Deserialize)]
struct IntegrationBinding {
    #[serde(flatten)]
    locator: ResourceObject,
    #[serde(default)]
    default: bool,
    #[serde(default)]
    message_mode: ActorIntegrationMessageMode,
    #[serde(default)]
    params: Parameters,
}

#[derive(Debug, Clone, Deserialize)]
struct ActionFile {
    #[serde(default)]
    execution_scope: crate::action_contract::ExecutionScope,
    #[serde(default)]
    response_mode: crate::action_contract::ResponseMode,
    #[serde(default = "initial_contract_revision")]
    contract_revision: String,
    verb: String,
    object: String,
    title: String,
    description: String,
    input_schema: SchemaLocator,
    output_schema: SchemaLocator,
}

fn initial_contract_revision() -> String {
    "1".into()
}

impl ActionFile {
    fn action_name(&self) -> String {
        format!("{}--{}", self.verb, self.object)
    }
}

#[derive(Debug, Clone, Deserialize)]
struct SchemaLocator {
    path: Option<String>,
    #[serde(rename = "ref")]
    reference: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct IntegrationFile {
    adapter: ResourceObject,
    #[serde(default)]
    params: Parameters,
}

#[derive(Debug, Clone, Deserialize)]
struct ConversationFile {
    participant_actor_refs: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ActionInvocationFile {
    requester_actor_ref: String,
    target_actor_ref: String,
    conversation_ref: Option<String>,
    actor_integration_ref: Option<String>,
    parent_invocation_ref: Option<String>,
    stop_requested_by_actor_ref: Option<String>,
    action_name: String,
    input: Value,
    #[serde(default)]
    responses: Vec<ActionResponseFile>,
}

#[derive(Debug, Clone, Deserialize)]
struct ActionResponseFile {
    invocation_ref: String,
    sequence: u64,
    status: crate::v2026_09::invocation::ActionResponseStatus,
    #[serde(default, deserialize_with = "deserialize_graph_present_value")]
    result: Option<Value>,
}

