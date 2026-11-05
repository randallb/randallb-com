fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let outcome = match args.as_slice() {
        [] => cwr_adapters::adapter::claude::run_claude_adapter(),
        [mode, socket] if mode == "--mcp" => {
            cwr_adapters::adapter::claude::mcp::run_mcp(std::path::Path::new(socket))
        }
        _ => Err("usage: cwr-adapter-claude [--mcp SOCKET]".into()),
    };
    if let Err(error) = outcome {
        eprintln!("cwr-adapter-claude: {error}");
        std::process::exit(1);
    }
}
