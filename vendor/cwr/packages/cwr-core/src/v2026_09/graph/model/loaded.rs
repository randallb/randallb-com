fn deserialize_graph_present_value<'de, D>(deserializer: D) -> Result<Option<Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Value::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, Deserialize)]
struct AdapterFile {
    #[serde(default)]
    name: String,
    #[serde(rename = "ref")]
    reference: String,
    #[serde(default)]
    authority_capabilities: Vec<String>,
    coworker_params_schema: Option<SchemaLocator>,
    integration_params_schema: Option<SchemaLocator>,
    actor_integration_params_schema: Option<SchemaLocator>,
}

#[derive(Debug, Clone)]
enum ResourceKind {
    Coworker(CoworkerFile),
    Gateway(GatewayFile),
    Role(RoleFile),
    Integration(IntegrationFile),
    Adapter(AdapterFile),
    Conversation(ConversationFile),
    ActionInvocation(ActionInvocationFile),
    Other,
}

#[derive(Debug, Clone)]
struct LoadedResource {
    source: PathBuf,
    type_ref: String,
    effective_ref: String,
    scope_id: usize,
    visibility: Visibility,
    document: Value,
    kind: ResourceKind,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum ResolvedTarget {
    Resource(usize),
    External(String),
}

#[derive(Debug)]
struct ResolvedScope {
    source: PathBuf,
    root_ref: String,
    lookup: BTreeMap<String, ResolvedTarget>,
}

#[derive(Debug)]
struct ResolvedGraph {
    root_scope: usize,
    scopes: Vec<ResolvedScope>,
    resources: Vec<LoadedResource>,
    working_path: PathBuf,
    source_roots: Vec<PathBuf>,
    coworker_volumes: BTreeMap<PathBuf, BTreeMap<String, (PathBuf, String)>>,
    disabled: BTreeSet<PathBuf>,
    warnings: Vec<GraphLoadWarning>,
}

#[derive(Debug, Clone)]
enum AdapterSource {
    File(PathBuf),
    BuiltIn(crate::adapter::BuiltInAdapterResource),
}

#[derive(Debug, Clone)]
struct ResolvedAdapter {
    definition: AdapterFile,
    effective_ref: String,
    source: AdapterSource,
    schema_root_ref: Option<String>,
}

/// Supplies absolute CWR refs to the checker as trusted, materialized files.
///
/// The field selecting the ref determines whether the file is decoded as a
/// typed TOML resource, a `CWR.toml` entry point, or a JSON Schema document.
pub trait CheckResourceResolver {
    fn resolve(&self, reference: &str) -> CwrResult<Option<PathBuf>>;
    fn source_roots(&self) -> Option<&[PathBuf]> {
        None
    }
}

pub struct EmptyResourceResolver;

impl CheckResourceResolver for EmptyResourceResolver {
    fn resolve(&self, _reference: &str) -> CwrResult<Option<PathBuf>> {
        Ok(None)
    }
}
