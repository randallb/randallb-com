//! Small, experimental Bel evaluator. This is deliberately not a complete Bel implementation.

mod bootstrap;
#[cfg(test)]
mod comparison;
mod eval;
mod limits;
mod quasiquote;
mod reader;
pub mod repl;
mod value;

pub use bootstrap::{BOOTSTRAP_FRAGMENTS, bootstrap_source, eval_bootstrap};
pub use eval::Interpreter;
pub use reader::{ReadError, read};
pub use value::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Error {}
