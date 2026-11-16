use std::{
    env,
    error::Error,
    ffi::OsStr,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

type InitResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

const BASE_CWR_TOML: &str = r#"type = "cwp://org.coworkerprotocol/2026-09/schemas"
"#;

pub fn run_init_command(workspace_directory: Option<&OsStr>) -> InitResult<()> {
    let workspace_directory = workspace_directory
        .map(PathBuf::from)
        .map_or_else(env::current_dir, Ok)?;
    let manifest = initialize_workspace(workspace_directory, |manifest| {
        cwr::v2026_09::graph::check(manifest).map(drop)
    })?;
    println!(
        "Initialized CWR workspace at {}",
        fs::canonicalize(&manifest)?.display()
    );
    Ok(())
}

fn initialize_workspace(
    workspace_directory: PathBuf,
    validate: impl FnOnce(&Path) -> InitResult<()>,
) -> InitResult<PathBuf> {
    fs::create_dir_all(&workspace_directory).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "could not create workspace directory {}: {error}",
                workspace_directory.display()
            ),
        )
    })?;

    let manifest = workspace_directory.join("CWR.toml");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&manifest)
        .map_err(|error| {
            let message = if error.kind() == io::ErrorKind::AlreadyExists {
                format!("{} already exists", manifest.display())
            } else {
                format!("could not create {}: {error}", manifest.display())
            };
            io::Error::new(error.kind(), message)
        })?;
    if let Err(error) = file.write_all(BASE_CWR_TOML.as_bytes()) {
        drop(file);
        let error = io::Error::new(
            error.kind(),
            format!("could not write {}: {error}", manifest.display()),
        );
        return Err(clean_up_manifest(&manifest, error.into()));
    }
    drop(file);

    if let Err(error) = validate(&manifest) {
        return Err(clean_up_manifest(&manifest, error));
    }
    Ok(manifest)
}

fn clean_up_manifest(
    manifest: &Path,
    error: Box<dyn Error + Send + Sync>,
) -> Box<dyn Error + Send + Sync> {
    match fs::remove_file(manifest) {
        Ok(()) => error,
        Err(cleanup_error) => io::Error::new(
            cleanup_error.kind(),
            format!(
                "{error}; could not remove incomplete {}: {cleanup_error}",
                manifest.display()
            ),
        )
        .into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_validation_removes_the_new_manifest() {
        let temporary = tempfile::tempdir().expect("temporary workspace");

        let result = initialize_workspace(temporary.path().to_path_buf(), |_| {
            Err(io::Error::other("validation failed").into())
        });

        assert_eq!(result.unwrap_err().to_string(), "validation failed");
        assert!(!temporary.path().join("CWR.toml").exists());
    }
}
