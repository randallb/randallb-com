//! Trusted coordinator-side proxy; reusable coordinator credentials never enter
//! the disposable execution container.
use std::{io, process::ExitCode};

fn main() -> ExitCode {
    match cwr::cloud_execution::run_proxy(io::BufReader::new(io::stdin()), io::stdout().lock()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("cwr-adapter-cloud: {error}");
            ExitCode::FAILURE
        }
    }
}
