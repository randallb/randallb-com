use super::{
    pin_regular_file_parent, resolve_graph, resolver::source_file, COWORKER_TYPE, ROOT_TYPE,
};
use std::{fs, path::PathBuf, thread, time::Duration};

fn imported_declaration(contents: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("CWR.toml");
    let coworker = temporary.path().join("COWORKER.toml");
    fs::write(
        &root,
        format!("type = {ROOT_TYPE:?}\n[[imports]]\npath = './COWORKER.toml'\n"),
    )
    .unwrap();
    fs::write(&coworker, contents).unwrap();
    (temporary, root, coworker.canonicalize().unwrap())
}

#[test]
fn graph_load_waits_for_empty_imported_declaration_to_arrive() {
    let (_temporary, root, coworker) = imported_declaration("");
    let publisher_path = coworker.clone();
    let publisher = thread::spawn(move || {
        let staged_file = publisher_path.with_extension("pending");
        fs::write(
            &staged_file,
            format!("type = {COWORKER_TYPE:?}\nname = 'Researcher'\nslug = 'researcher'\n"),
        )
        .unwrap();
        thread::sleep(Duration::from_millis(100));
        fs::rename(staged_file, publisher_path).unwrap();
    });

    let result = resolve_graph(&root);
    publisher.join().unwrap();
    let graph = result.unwrap();
    assert_eq!(graph.actors.len(), 1);
    assert_eq!(graph.actors[0].actor_name.as_deref(), Some("Researcher"));
    assert_eq!(graph.actors[0].coworker_slug, "researcher");
    assert_eq!(graph.actors[0].coworker_source, coworker);
}

#[test]
#[cfg(unix)]
fn empty_import_replaced_by_an_external_symlink_is_rejected() {
    let (_temporary, root, coworker) = imported_declaration("");
    let external = tempfile::tempdir().unwrap();
    let external_coworker = external.path().join("COWORKER.toml");
    fs::write(
        &external_coworker,
        format!("type = {COWORKER_TYPE:?}\nname = 'External'\nslug = 'external'\n"),
    )
    .unwrap();
    let publisher_path = coworker.clone();
    let publisher = thread::spawn(move || {
        let staged_link = publisher_path.with_extension("pending");
        std::os::unix::fs::symlink(external_coworker, &staged_link).unwrap();
        thread::sleep(Duration::from_millis(100));
        fs::rename(staged_link, publisher_path).unwrap();
    });

    let result = resolve_graph(&root);
    publisher.join().unwrap();
    let error = result.unwrap_err().to_string();
    assert!(error.contains(&coworker.display().to_string()), "{error}");
    assert!(
        error.contains("regular non-symlink")
            || error.contains("location changed")
            || error.contains("Too many levels of symbolic links"),
        "{error}"
    );
}

#[test]
#[cfg(unix)]
fn empty_import_is_not_redirected_by_an_ancestor_symlink() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("CWR.toml");
    let declarations = temporary.path().join("declarations");
    fs::create_dir(&declarations).unwrap();
    let coworker = declarations.join("COWORKER.toml");
    fs::write(
        &root,
        format!("type = {ROOT_TYPE:?}\n[[imports]]\npath = './declarations/COWORKER.toml'\n"),
    )
    .unwrap();
    fs::write(&coworker, "").unwrap();

    let external = tempfile::tempdir().unwrap();
    fs::write(
        external.path().join("COWORKER.toml"),
        format!("type = {COWORKER_TYPE:?}\nname = 'External'\nslug = 'external'\n"),
    )
    .unwrap();
    let replaced = temporary.path().join("original-declarations");
    let publisher = thread::spawn({
        let declarations = declarations.clone();
        let external = external.path().to_owned();
        move || {
            thread::sleep(Duration::from_millis(100));
            fs::rename(&declarations, replaced).unwrap();
            std::os::unix::fs::symlink(external, declarations).unwrap();
        }
    });

    let result = resolve_graph(&root);
    publisher.join().unwrap();
    let error = result.unwrap_err().to_string();
    assert!(error.contains(&coworker.display().to_string()), "{error}");
    assert!(error.contains("empty"), "{error}");
}

#[test]
#[cfg(unix)]
fn validated_import_is_not_redirected_before_its_parent_is_pinned() {
    let temporary = tempfile::tempdir().unwrap();
    let declarations = temporary.path().join("declarations");
    fs::create_dir(&declarations).unwrap();
    let coworker = declarations.join("COWORKER.toml");
    fs::write(&coworker, format!("type = {COWORKER_TYPE:?}\n")).unwrap();
    let source = source_file(&coworker, &[temporary.path().canonicalize().unwrap()]).unwrap();

    let external = tempfile::tempdir().unwrap();
    fs::write(
        external.path().join("COWORKER.toml"),
        format!("type = {COWORKER_TYPE:?}\nname = 'External'\nslug = 'external'\n"),
    )
    .unwrap();
    fs::rename(
        &declarations,
        temporary.path().join("original-declarations"),
    )
    .unwrap();
    std::os::unix::fs::symlink(external.path(), &declarations).unwrap();

    assert!(pin_regular_file_parent(&source).is_err());
}

#[test]
#[cfg(unix)]
fn empty_import_replaced_by_a_fifo_is_rejected_without_blocking() {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    let (_temporary, root, coworker) = imported_declaration("");
    let publisher_path = coworker.clone();
    let publisher = thread::spawn(move || {
        thread::sleep(Duration::from_millis(100));
        fs::remove_file(&publisher_path).unwrap();
        let path = CString::new(publisher_path.as_os_str().as_bytes()).unwrap();
        // SAFETY: path is a valid, nul-terminated C string for this call.
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    });

    let result = resolve_graph(&root);
    publisher.join().unwrap();
    let error = result.unwrap_err().to_string();
    assert!(error.contains(&coworker.display().to_string()), "{error}");
    assert!(error.contains("regular file"), "{error}");
}

#[test]
fn empty_import_retries_a_transient_missing_replacement() {
    let (_temporary, root, coworker) = imported_declaration("");
    let publisher_path = coworker.clone();
    let publisher = thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        fs::remove_file(&publisher_path).unwrap();
        thread::sleep(Duration::from_millis(150));
        fs::write(
            publisher_path,
            format!("type = {COWORKER_TYPE:?}\nname = 'Researcher'\nslug = 'researcher'\n"),
        )
        .unwrap();
    });

    let result = resolve_graph(&root);
    publisher.join().unwrap();
    let graph = result.unwrap();
    assert_eq!(graph.actors[0].coworker_slug, "researcher");
}

#[test]
fn permanently_empty_import_reports_empty_contents_instead_of_missing_type() {
    let (_temporary, root, coworker) = imported_declaration(" \t\r\n");
    let error = resolve_graph(&root).unwrap_err().to_string();
    assert!(error.contains(&coworker.display().to_string()), "{error}");
    assert!(error.contains("empty"), "{error}");
    assert!(!error.contains("must contain a string type"), "{error}");
}

#[test]
fn malformed_import_keeps_its_original_toml_error() {
    let malformed = "type = \"unfinished";
    let (_temporary, root, coworker) = imported_declaration(malformed);
    let parse_error = toml::from_str::<toml::Value>(malformed)
        .unwrap_err()
        .to_string();
    let error = resolve_graph(&root).unwrap_err().to_string();
    assert_eq!(error, format!("{}: {parse_error}", coworker.display()));
}

#[test]
fn nonempty_import_without_a_string_type_keeps_its_declaration_error() {
    for contents in ["type = 17\n", "# awaiting an authored declaration\n"] {
        let (_temporary, root, coworker) = imported_declaration(contents);
        assert_eq!(
            resolve_graph(&root).unwrap_err().to_string(),
            format!(
                "{}: a 2026-09 declaration must contain a string type",
                coworker.display()
            )
        );
    }
}

#[test]
fn invalid_utf8_import_keeps_its_read_error() {
    let (_temporary, root, coworker) = imported_declaration("");
    fs::write(&coworker, [0xff, 0xfe]).unwrap();
    let read_error = fs::read_to_string(&coworker).unwrap_err().to_string();
    assert_eq!(
        resolve_graph(&root).unwrap_err().to_string(),
        format!("{}: {read_error}", coworker.display())
    );
}

#[test]
fn nested_resource_metadata_failure_preserves_operational_error() {
    let temporary = tempfile::tempdir().unwrap();
    let parent = temporary.path().join("file");
    fs::write(&parent, "not a directory").unwrap();
    let error = super::resolved_regular_file("nested", &parent.join("child.toml")).unwrap_err();
    assert!(!error.is::<crate::ConfigurationError>());
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::NotADirectory
    );
}
