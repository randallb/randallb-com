use std::{io, process::ExitCode};

fn main() -> ExitCode {
    let stdin = io::stdin();
    let stdout = io::stdout();
    match cwr_adapters::adapter::open_responses::run_open_responses_adapter(
        stdin.lock(),
        stdout.lock(),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("cwr-adapter-open-responses: {error}");
            ExitCode::FAILURE
        }
    }
}
