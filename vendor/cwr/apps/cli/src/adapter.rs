//! Local adapter setup commands that do not select a workspace or runtime.

use std::{
    error::Error,
    ffi::{OsStr, OsString},
    io::{self, BufReader, Write},
    path::PathBuf,
};

use cwr::adapter::{
    adapter_setup_readiness, configure_adapter, import_adapter_home, run_adapter_repair,
    AdapterRepairKind,
};
use serde_json::json;

pub const USAGE: &str =
    "usage: cwr adapter configure <adapter-ref> | cwr adapter status <adapter-ref> | cwr adapter repair <adapter-ref> provider_login | cwr adapter import <adapter-ref> --source <absolute-path> [--dry-run] [--replace]";

pub fn run(arguments: &[OsString]) -> Result<(), Box<dyn Error + Send + Sync>> {
    match arguments {
        [command, adapter_ref, options @ ..] if command == OsStr::new("import") => {
            let options = parse_import_options(options)?;
            let report = import_adapter_home(
                adapter_ref.to_str().unwrap_or(""),
                &options.source,
                options.dry_run,
                options.replace,
            )?;
            serde_json::to_writer(io::stdout().lock(), &report)?;
            println!();
            Ok(())
        }
        [command, adapter_ref] if command == OsStr::new("configure") => {
            let configuration = configure_adapter(adapter_ref.to_str().unwrap_or(""))?;
            serde_json::to_writer(io::stdout().lock(), &configuration)?;
            println!();
            Ok(())
        }
        [command, adapter_ref] if command == OsStr::new("status") => {
            let readiness = adapter_setup_readiness(adapter_ref.to_str().unwrap_or(""));
            serde_json::to_writer(io::stdout().lock(), &readiness)?;
            println!();
            Ok(())
        }
        [command, adapter_ref, kind] if command == OsStr::new("repair") => {
            if kind != OsStr::new("provider_login") {
                return unsupported_repair();
            }
            run_adapter_repair(
                adapter_ref.to_str().unwrap_or(""),
                AdapterRepairKind::ProviderLogin,
                BufReader::new(io::stdin()),
                io::stdout().lock(),
            )?;
            Ok(())
        }
        _ => Err(USAGE.into()),
    }
}

#[derive(Debug, PartialEq)]
struct ImportOptions {
    source: PathBuf,
    dry_run: bool,
    replace: bool,
}

fn parse_import_options(
    arguments: &[OsString],
) -> Result<ImportOptions, Box<dyn Error + Send + Sync>> {
    let mut source = None;
    let mut dry_run = false;
    let mut replace = false;
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--source") if source.is_none() => {
                let path = PathBuf::from(arguments.next().ok_or(USAGE)?);
                if !path.is_absolute() {
                    return Err("Codex import source must be absolute".into());
                }
                source = Some(path);
            }
            Some("--dry-run") if !dry_run => dry_run = true,
            Some("--replace") if !replace => replace = true,
            _ => return Err(USAGE.into()),
        }
    }
    Ok(ImportOptions {
        source: source.ok_or(USAGE)?,
        dry_run,
        replace,
    })
}

fn unsupported_repair() -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut output = io::stdout().lock();
    serde_json::to_writer(
        &mut output,
        &json!({
            "event": "error",
            "code": "adapter_repair_unavailable",
            "message": "Sign-in is currently unavailable for this adapter."
        }),
    )?;
    output.write_all(b"\n")?;
    output.flush()?;
    Err(io::Error::other("adapter_repair_unavailable").into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_and_repair_require_exact_arguments() {
        for arguments in [
            vec![OsString::from("status")],
            vec![OsString::from("repair"), OsString::from("codex")],
            vec![
                OsString::from("repair"),
                OsString::from("codex"),
                OsString::from("provider_login"),
                OsString::from("extra"),
            ],
        ] {
            assert_eq!(run(&arguments).unwrap_err().to_string(), USAGE);
        }
    }

    #[test]
    fn import_requires_one_absolute_source_and_unique_flags() {
        for arguments in [
            vec![],
            vec!["--source"],
            vec!["--source", "relative"],
            vec!["--source", "/source", "--source", "/other"],
            vec!["--source", "/source", "--dry-run", "--dry-run"],
            vec!["--source", "/source", "--replace", "--replace"],
            vec!["--source", "/source", "--unknown"],
        ] {
            let arguments = arguments
                .into_iter()
                .map(OsString::from)
                .collect::<Vec<_>>();
            assert!(parse_import_options(&arguments).is_err());
        }
        let options = ["--dry-run", "--source", "/source with spaces", "--replace"]
            .into_iter()
            .map(OsString::from)
            .collect::<Vec<_>>();
        assert_eq!(
            parse_import_options(&options).unwrap(),
            ImportOptions {
                source: PathBuf::from("/source with spaces"),
                dry_run: true,
                replace: true,
            }
        );
    }
}
