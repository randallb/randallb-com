use std::{ffi::OsString, io, path::PathBuf};

fn main() {
    if std::env::args_os().skip(1).collect::<Vec<_>>() == [std::ffi::OsString::from("--configure")]
    {
        match cwr_adapters::adapter::codex::configuration::configure() {
            Ok(configuration) => {
                println!(
                    "{}",
                    serde_json::to_string(&configuration).expect("configuration serializes")
                );
                return;
            }
            Err(error) => {
                eprintln!("cwr-adapter-codex: {error}");
                std::process::exit(1);
            }
        }
    }
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    let result: Result<(), Box<dyn std::error::Error + Send + Sync>> = if arguments.is_empty() {
        cwr_adapters::adapter::codex::run_codex_adapter(
            io::BufReader::new(io::stdin().lock()),
            io::stdout().lock(),
        )
    } else if arguments
        .first()
        .is_some_and(|mode| mode == "--import-home")
    {
        run_import(&arguments[1..])
    } else if arguments.len() == 1 && arguments[0] == "--setup-status" {
        let readiness = cwr::adapter::codex_login_readiness();
        serde_json::to_writer(io::stdout().lock(), &readiness).map_err(Into::into)
    } else if arguments.len() == 1 && arguments[0] == "--setup-login" {
        cwr::adapter::run_codex_login_repair(io::BufReader::new(io::stdin()), io::stdout())
            .map_err(Into::into)
    } else {
        Err(Box::new(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unsupported adapter mode",
        )))
    };
    if let Err(error) = result {
        eprintln!("cwr-adapter-codex: {error}");
        std::process::exit(1);
    }
}

fn run_import(arguments: &[OsString]) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (source, dry_run, replace) = import_options(arguments)?;
    let report = cwr_adapters::adapter::codex::configuration::import_home(source, dry_run, replace)
        .map_err(|error| {
            // TOML diagnostics can include a source excerpt after the first
            // line. Keep the reason/location without exposing its values.
            let diagnostic: String = error
                .to_string()
                .lines()
                .next()
                .unwrap_or("Codex import failed")
                .chars()
                .take(1000)
                .collect();
            io::Error::other(diagnostic)
        })?;
    serde_json::to_writer(io::stdout().lock(), &report)?;
    println!();
    Ok(())
}

fn import_options(arguments: &[OsString]) -> Result<(PathBuf, bool, bool), io::Error> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: cwr-adapter-codex --import-home <absolute-source> [--dry-run] [--replace]",
        )
    };
    let (source, options) = arguments.split_first().ok_or_else(invalid)?;
    let source = PathBuf::from(source);
    if !source.is_absolute() {
        return Err(invalid());
    }
    let mut dry_run = false;
    let mut replace = false;
    for option in options {
        match option.to_str() {
            Some("--dry-run") if !dry_run => dry_run = true,
            Some("--replace") if !replace => replace = true,
            _ => return Err(invalid()),
        }
    }
    Ok((source, dry_run, replace))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_rejects_incomplete_relative_and_duplicate_arguments() {
        for arguments in [
            vec![],
            vec!["relative"],
            vec!["/source", "--dry-run", "--dry-run"],
            vec!["/source", "--replace", "--replace"],
            vec!["/source", "extra"],
        ] {
            let arguments = arguments
                .into_iter()
                .map(OsString::from)
                .collect::<Vec<_>>();
            assert!(import_options(&arguments).is_err());
        }
        assert_eq!(
            import_options(&[
                OsString::from("/source with spaces"),
                OsString::from("--replace"),
                OsString::from("--dry-run")
            ])
            .unwrap(),
            (PathBuf::from("/source with spaces"), true, true)
        );
    }
}
