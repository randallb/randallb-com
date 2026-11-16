fn ensure_workspace_identity_ignored(workspace_directory: &Path) -> CwrResult<()> {
    let ignore_path = workspace_directory.join(".gitignore");
    if fs::symlink_metadata(&ignore_path)
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return invalid(format!(
            "{}: workspace ignore file must not be a symlink",
            ignore_path.display()
        ));
    }
    for directory in workspace_directory.ancestors() {
        match fs::read(directory.join(".gitignore")) {
            Ok(contents) if workspace_identity_ignore_rule_present(&contents) => return Ok(()),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    let existing = match fs::read(&ignore_path) {
        Ok(existing) => existing,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error.into()),
    };
    if workspace_identity_ignore_rule_present(&existing) {
        return Ok(());
    }
    use std::io::Write as _;
    let mut ignore = OpenOptions::new()
        .create(true)
        .append(true)
        .custom_flags(libc::O_NOFOLLOW)
        .mode(0o644)
        .open(&ignore_path)?;
    if !existing.is_empty() && !existing.ends_with(b"\n") {
        ignore.write_all(b"\n")?;
    }
    ignore.write_all(b".cwr/\n")?;
    ignore.sync_all()?;
    File::open(workspace_directory)?.sync_all()?;
    Ok(())
}

fn workspace_identity_ignore_rule_present(contents: &[u8]) -> bool {
    contents.split(|byte| *byte == b'\n').any(|line| {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        matches!(
            line,
            b".cwr/" | b"/.cwr/" | b"**/.cwr/" | b".cwr" | b"/.cwr" | b"**/.cwr"
        )
    })
}

#[cfg(test)]
mod workspace_ignore_tests {
    use super::*;

    #[test]
    fn appends_a_workspace_identity_ignore_rule() {
        let temporary = tempfile::tempdir().unwrap();
        fs::write(temporary.path().join(".gitignore"), "target/").unwrap();

        ensure_workspace_identity_ignored(temporary.path()).unwrap();

        assert_eq!(
            fs::read_to_string(temporary.path().join(".gitignore")).unwrap(),
            "target/\n.cwr/\n"
        );
    }

    #[test]
    fn honors_an_ancestor_workspace_identity_ignore_rule() {
        let temporary = tempfile::tempdir().unwrap();
        fs::write(temporary.path().join(".gitignore"), "**/.cwr/\n").unwrap();
        let workspace = temporary.path().join("nested/workspace");
        fs::create_dir_all(&workspace).unwrap();

        ensure_workspace_identity_ignored(&workspace).unwrap();

        assert!(!workspace.join(".gitignore").exists());
    }

    #[test]
    fn refuses_a_symlinked_workspace_ignore_file() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().unwrap();
        let target = temporary.path().join("outside-ignore");
        fs::write(&target, "target/\n").unwrap();
        let workspace = temporary.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        symlink(&target, workspace.join(".gitignore")).unwrap();

        let error = ensure_workspace_identity_ignored(&workspace)
            .unwrap_err()
            .to_string();

        assert!(error.contains("must not be a symlink"));
        assert_eq!(fs::read_to_string(target).unwrap(), "target/\n");
    }
}
