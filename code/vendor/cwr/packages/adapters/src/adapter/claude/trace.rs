use std::{
    fmt::Display,
    io::{self, Write},
    sync::OnceLock,
    time::Instant,
};

static ENABLED: OnceLock<bool> = OnceLock::new();
static STARTED: OnceLock<Instant> = OnceLock::new();

pub(super) fn emit(component: &str, event: impl Display) {
    if !*ENABLED.get_or_init(|| std::env::var_os("CWR_CLAUDE_TRACE").is_some()) {
        return;
    }
    let elapsed = STARTED.get_or_init(Instant::now).elapsed().as_millis();
    let mut diagnostics = io::stderr().lock();
    let _ = writeln!(
        diagnostics,
        "cwr-adapter-claude: trace pid={} elapsed_ms={elapsed} component={component} event={event}",
        std::process::id()
    );
}
