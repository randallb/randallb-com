//! Local CWR workspace discovery and explicit selection.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use crate::CwrResult;

/// Resolve an explicitly selected workspace directory or `CWR.toml` file.
pub fn resolve_workspace_manifest(input: impl AsRef<Path>) -> CwrResult<PathBuf> {
    let input = input.as_ref();
    let candidate = if input.is_dir() {
        input.join("CWR.toml")
    } else {
        input.to_owned()
    };
    if candidate.file_name().and_then(|name| name.to_str()) != Some("CWR.toml") {
        return invalid("a CWR workspace must be a directory or CWR.toml entry point");
    }
    fs::canonicalize(&candidate).map_err(|error| {
        io::Error::new(error.kind(), format!("{}: {error}", candidate.display())).into()
    })
}

/// Find the nearest `CWR.toml` in `start` or one of its parent directories.
pub fn discover_workspace_manifest(start: impl AsRef<Path>) -> CwrResult<PathBuf> {
    let start = fs::canonicalize(start.as_ref())?;
    let directory = if start.is_dir() {
        start.as_path()
    } else {
        start
            .parent()
            .ok_or_else(|| invalid_error("workspace discovery start has no parent directory"))?
    };
    for ancestor in directory.ancestors() {
        let candidate = ancestor.join("CWR.toml");
        if candidate.is_file() {
            return fs::canonicalize(candidate).map_err(Into::into);
        }
    }
    invalid(format!(
        "no CWR.toml was found in {} or its parents",
        directory.display()
    ))
}

fn invalid<T>(message: impl Into<String>) -> CwrResult<T> {
    Err(invalid_error(message))
}

fn invalid_error(message: impl Into<String>) -> Box<dyn std::error::Error + Send + Sync> {
    io::Error::new(io::ErrorKind::InvalidInput, message.into()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_uses_the_nearest_ancestor_workspace() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let outer = temporary.path().join("outer");
        let inner = outer.join("packages/inner");
        fs::create_dir_all(&inner).expect("create nested directory");
        fs::write(outer.join("CWR.toml"), "").expect("write outer manifest");
        fs::write(inner.join("CWR.toml"), "").expect("write inner manifest");
        let descendant = inner.join("src");
        fs::create_dir(&descendant).expect("create descendant");

        assert_eq!(
            discover_workspace_manifest(&descendant).expect("discover workspace"),
            fs::canonicalize(inner.join("CWR.toml")).expect("canonical inner manifest")
        );
    }

    #[test]
    fn explicit_selection_accepts_a_directory_or_manifest() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let manifest = temporary.path().join("CWR.toml");
        fs::write(&manifest, "").expect("write manifest");
        let expected = fs::canonicalize(&manifest).expect("canonical manifest");

        assert_eq!(
            resolve_workspace_manifest(temporary.path()).expect("resolve directory"),
            expected
        );
        assert_eq!(
            resolve_workspace_manifest(&manifest).expect("resolve manifest"),
            expected
        );
    }
}
