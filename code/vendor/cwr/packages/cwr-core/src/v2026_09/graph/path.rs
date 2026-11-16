use std::{
    ffi::CString,
    fs,
    fs::{File, OpenOptions},
    io,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::OpenOptionsExt},
    },
    path::{Component, Path, PathBuf},
};

use cwr_core::CwrRef;

use super::{invalid, invalid_error, CwrResult};

pub(super) fn resolve_regular_file(
    declaration: &Path,
    relative: &str,
    label: &str,
) -> CwrResult<PathBuf> {
    if !relative.starts_with("./") {
        return invalid(format!(
            "{}: {label} must start with ./",
            declaration.display()
        ));
    }
    let relative_path = Path::new(relative);
    for component in relative_path.components() {
        match component {
            Component::CurDir | Component::Normal(_) => {}
            Component::ParentDir => {
                return invalid(format!(
                    "{}: {label} must not contain ..",
                    declaration.display()
                ))
            }
            Component::RootDir | Component::Prefix(_) => {
                return invalid(format!(
                    "{}: {label} must be relative",
                    declaration.display()
                ))
            }
        }
    }
    let base = declaration.parent().ok_or_else(|| {
        invalid_error(format!("{} has no parent directory", declaration.display()))
    })?;
    let canonical_base = base.canonicalize()?;
    let candidate = base.join(relative_path);
    let metadata = fs::symlink_metadata(&candidate)
        .map_err(|error| crate::error::configuration_file_error(&candidate, error))?;
    if metadata.file_type().is_symlink() {
        return invalid(format!(
            "{}: {label} must not identify a symlink",
            candidate.display()
        ));
    }
    if !metadata.is_file() {
        return invalid(format!(
            "{}: {label} must identify a regular file",
            candidate.display()
        ));
    }
    let canonical = candidate.canonicalize()?;
    if !canonical.starts_with(&canonical_base) {
        return invalid(format!(
            "{}: {label} escapes the declaration directory",
            candidate.display()
        ));
    }
    Ok(canonical)
}

pub(super) fn validate_identity_ref(path: &Path, reference: &str) -> CwrResult<()> {
    CwrRef::parse(reference).map(|_| ()).map_err(|error| {
        invalid_error(format!(
            "{}: invalid ref {reference}: {error}",
            path.display()
        ))
    })
}

pub(super) fn absolute_path(path: &Path) -> CwrResult<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    resolved_regular_file("source", &absolute)
}

pub(super) fn resolved_regular_file(reference: &str, path: &Path) -> CwrResult<PathBuf> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| crate::error::configuration_file_error(path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return invalid(format!(
            "resolved resource {reference} at {} must be a regular non-symlink file",
            path.display()
        ));
    }
    path.canonicalize().map_err(Into::into)
}

pub(super) struct PinnedRegularFile {
    directory: File,
    name: CString,
}

impl PinnedRegularFile {
    pub(super) fn open(&self) -> io::Result<File> {
        // SAFETY: openat borrows a live directory descriptor and C string. A
        // successful descriptor is transferred exactly once into File.
        let raw = unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                self.name.as_ptr(),
                libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if raw == -1 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: raw is newly owned after the successful openat call.
        let file = unsafe { File::from_raw_fd(raw) };
        if !file.metadata()?.is_file() {
            return Err(io::Error::other("declaration must be a regular file"));
        }
        Ok(file)
    }
}

pub(super) fn pin_regular_file_parent(path: &Path) -> CwrResult<PinnedRegularFile> {
    let source = path;
    let parent = source
        .parent()
        .ok_or_else(|| invalid_error(format!("{} has no parent directory", source.display())))?;
    let mut directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(Path::new("/"))?;
    for component in parent.components() {
        let Component::Normal(name) = component else {
            continue;
        };
        let name = CString::new(name.as_bytes())?;
        // SAFETY: openat borrows a live directory descriptor and C string. A
        // successful descriptor is transferred exactly once into File.
        let raw = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if raw == -1 {
            return Err(io::Error::last_os_error().into());
        }
        // SAFETY: raw is newly owned after the successful openat call.
        directory = unsafe { File::from_raw_fd(raw) };
    }
    let name = source
        .file_name()
        .ok_or_else(|| invalid_error(format!("{} has no file name", source.display())))?;
    Ok(PinnedRegularFile {
        directory,
        name: CString::new(name.as_bytes())?,
    })
}
