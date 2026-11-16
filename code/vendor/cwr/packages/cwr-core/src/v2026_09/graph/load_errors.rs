#[derive(Debug)]
struct NamespaceConflict(String);
impl std::fmt::Display for NamespaceConflict {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for NamespaceConflict {}

fn insert_target(
    source: &Path,
    lookup: &mut BTreeMap<String, ResolvedTarget>,
    reference: String,
    target: ResolvedTarget,
) -> CwrResult<()> {
    if lookup.get(&reference).is_some_and(|old| old != &target) {
        return Err(Box::new(NamespaceConflict(format!(
            "{}: contextual ref {reference} resolves to two declarations",
            source.display()
        ))));
    }
    lookup.insert(reference, target);
    Ok(())
}

#[derive(Debug)]
struct SharedStateLoadError(String);
impl std::fmt::Display for SharedStateLoadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for SharedStateLoadError {}
fn is_fatal_load_error(error: &(dyn std::error::Error + Send + Sync + 'static)) -> bool {
    error.is::<NamespaceConflict>() || error.is::<SharedStateLoadError>()
}

#[derive(Debug)]
struct ReceiverInputError(String);
impl std::fmt::Display for ReceiverInputError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for ReceiverInputError {}
fn receiver_input_error(
    error: Box<dyn std::error::Error + Send + Sync>,
) -> Box<dyn std::error::Error + Send + Sync> {
    Box::new(ReceiverInputError(error.to_string()))
}
