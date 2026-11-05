//! Provider-neutral advisory token estimation.

/// The pinned estimator used when a provider does not expose a native tokenizer.
pub const ESTIMATOR: &str = "tiktoken/o200k_base@4.1.2";

/// An approximate count suitable for scheduling provider-native compaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TokenEstimate {
    pub tokens: usize,
    pub estimator: &'static str,
}

/// Estimates the tokens in model-visible text with the shared CWR tokenizer.
///
/// This is advisory. Adapters remain responsible for provider framing, safety
/// margins, model context windows, and compaction policy.
pub fn estimate_tokens(text: &str) -> TokenEstimate {
    let encoding = tiktoken::get_encoding("o200k_base")
        .expect("the cwr-core build always includes the o200k_base vocabulary");
    TokenEstimate {
        tokens: encoding.count(text),
        estimator: ESTIMATOR,
    }
}
