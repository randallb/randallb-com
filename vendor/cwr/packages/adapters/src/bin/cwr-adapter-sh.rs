use std::{io, process::ExitCode};

fn main() -> ExitCode {
    if let Err(error) = cwr_adapters::adapter::install_shell_stop_handler() {
        eprintln!("cwr-adapter-sh: could not install stop handler: {error}");
        return ExitCode::FAILURE;
    }
    let stdin = io::stdin();
    let stdout = io::stdout();
    match cwr_adapters::adapter::run_sh_adapter_process(io::BufReader::new(stdin), stdout.lock()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("cwr-adapter-sh: {error}");
            ExitCode::FAILURE
        }
    }
}
