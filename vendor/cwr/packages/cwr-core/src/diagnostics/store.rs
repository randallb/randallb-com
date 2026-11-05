use serde_json::Value;
use std::{
    ffi::CString,
    os::fd::{AsRawFd, FromRawFd, RawFd},
};
use std::{
    fs::{self, DirBuilder, OpenOptions},
    io::{self, BufRead, BufReader},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

pub(super) struct MatchingRecord {
    pub path: PathBuf,
    pub line: usize,
    pub reference: Option<String>,
    pub value: Value,
}

pub(super) struct RecordLookup {
    pub records: Vec<MatchingRecord>,
    pub unreadable: bool,
}

pub(super) fn records_for_invocation(
    directory: &Path,
    invocation_ref: &str,
) -> io::Result<RecordLookup> {
    let directory_handle = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(directory)?;
    let fd = directory_handle.as_raw_fd();
    let mut names = fs::read_dir(directory)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            diagnostic_file_day(&name).map(|day| (day, name))
        })
        .collect::<Vec<_>>();
    names.sort();
    let mut records = Vec::new();
    let mut unreadable = false;
    for (_, name) in names {
        let file = match open_read_file(fd, &name) {
            Ok(file) => file,
            Err(_) => {
                unreadable = true;
                continue;
            }
        };
        for (line, value) in BufReader::new(file).lines().enumerate() {
            let Ok(value) = value else {
                unreadable = true;
                continue;
            };
            let Ok(value) = serde_json::from_str::<Value>(&value) else {
                unreadable = true;
                continue;
            };
            if value.get("invocation_ref").and_then(Value::as_str) == Some(invocation_ref) {
                records.push(MatchingRecord {
                    path: directory.join(&name),
                    line: line + 1,
                    reference: None,
                    value,
                });
            }
        }
    }
    let database = directory.join("wlpdb.sqlite3");
    if database.exists() || fs::symlink_metadata(&database).is_ok() {
        let lookup = (|| -> crate::CwrResult<Vec<MatchingRecord>> {
            let mut db = crate::cwpdb::NativeStore::open_read_only(&database)?;
            let page = db.diagnostics_for_invocation(invocation_ref, 1000)?;
            if page.truncated {
                unreadable = true;
            }
            Ok(page
                .records
                .into_iter()
                .map(|record| MatchingRecord {
                    path: database.clone(),
                    line: 0,
                    reference: Some(record.reference),
                    value: record.value,
                })
                .collect())
        })();
        match lookup {
            Ok(saved) => records.extend(saved),
            Err(_) => unreadable = true,
        }
    }
    Ok(RecordLookup {
        records,
        unreadable,
    })
}

fn diagnostic_file_day(name: &str) -> Option<u64> {
    name.strip_prefix("failures-")?
        .strip_suffix(".1.jsonl")
        .or_else(|| name.strip_prefix("failures-")?.strip_suffix(".jsonl"))?
        .parse()
        .ok()
}

pub(super) fn write_record(directory: &Path, record: &Value) -> io::Result<PathBuf> {
    let result = (|| -> crate::CwrResult<PathBuf> {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)?;
        let metadata = fs::symlink_metadata(directory)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err("invalid diagnostic directory".into());
        }
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        let path = directory.join("wlpdb.sqlite3");
        let mut db = crate::cwpdb::NativeStore::open(&path)?;
        let reference = format!(
            "cwp://org.coworkerprotocol.local/diagnostics/{}",
            uuid::Uuid::new_v4()
        );
        db.append_diagnostic(&reference, record)?;
        Ok(path)
    })();
    result.map_err(|e| io::Error::other(e.to_string()))
}

fn open_read_file(fd: RawFd, name: &str) -> io::Result<fs::File> {
    let name = CString::new(name)?;
    // SAFETY: openat borrows a valid directory fd and a live C string.
    let raw = unsafe { libc::openat(fd, name.as_ptr(), libc::O_RDONLY | libc::O_NOFOLLOW) };
    if raw == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: raw is a newly owned file descriptor from the successful openat.
    let file = unsafe { fs::File::from_raw_fd(raw) };
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("invalid diagnostic file"));
    }
    Ok(file)
}
