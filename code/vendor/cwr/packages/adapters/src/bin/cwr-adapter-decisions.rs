use std::{io, process::ExitCode};
fn main() -> ExitCode {
    match cwr_adapters::adapter::decisions::run_decisions_adapter(
        io::stdin().lock(),
        io::stdout().lock(),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => {
            eprintln!("cwr-adapter-decisions: invalid adapter invocation");
            ExitCode::FAILURE
        }
    }
}
