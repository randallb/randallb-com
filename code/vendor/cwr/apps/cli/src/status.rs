use std::io::{self, Write};

pub(super) fn print_responding(handle: &str) -> io::Result<()> {
    let stderr = io::stderr();
    let mut stderr = stderr.lock();
    writeln!(stderr, "{handle} is responding…")?;
    stderr.flush()
}
