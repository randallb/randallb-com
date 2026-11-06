fn main() -> std::io::Result<()> {
    bel_rust_repl::repl::run(
        std::io::stdin().lock(),
        std::io::stdout().lock(),
        std::io::stderr().lock(),
    )
}
