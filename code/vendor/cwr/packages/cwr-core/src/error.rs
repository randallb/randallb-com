//! Error boundary shared by every CWR protocol version and runtime adapter.

pub type CwrResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// A rejected authored configuration, distinct from failures accessing it.
#[derive(Debug)]
pub struct ConfigurationError(pub String);

impl std::fmt::Display for ConfigurationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ConfigurationError {}

pub(crate) fn configuration_file_error(
    path: &std::path::Path,
    error: std::io::Error,
) -> Box<dyn std::error::Error + Send + Sync> {
    let message = format!("{}: {error}", path.display());
    if error.kind() == std::io::ErrorKind::NotFound {
        Box::new(ConfigurationError(message))
    } else {
        Box::new(std::io::Error::new(error.kind(), message))
    }
}
