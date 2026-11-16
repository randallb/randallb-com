use std::{io, process::ExitCode};

fn main() -> ExitCode {
    let stdin = io::stdin();
    let stdout = io::stdout();
    match cwr_adapters::adapter::run_rng_adapter(stdin.lock(), stdout.lock()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("cwr-adapter-rng: {error}");
            ExitCode::FAILURE
        }
    }
}
