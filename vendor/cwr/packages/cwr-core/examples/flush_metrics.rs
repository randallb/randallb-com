//! Manual diagnostic: cargo run -p cwr-core --example flush_metrics -- <state-file>
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: flush_metrics <runtime-state-file>")?;
    cwr::flush_posthog_metrics(std::path::Path::new(&path))
}
