use crate::{Error, Interpreter};

/// Published Bel source fragments in their required evaluation order.
pub const BOOTSTRAP_FRAGMENTS: &[(&str, &str)] = &[
    (
        "00-foundation.bel",
        include_str!("../examples/bootstrap/00-foundation.bel"),
    ),
    (
        "01-variadic.bel",
        include_str!("../examples/bootstrap/01-variadic.bel"),
    ),
    (
        "02-macro-kernel.bel",
        include_str!("../examples/bootstrap/02-macro-kernel.bel"),
    ),
    (
        "03-core-macros.bel",
        include_str!("../examples/bootstrap/03-core-macros.bel"),
    ),
    (
        "04-equality.bel",
        include_str!("../examples/bootstrap/04-equality.bel"),
    ),
    (
        "05-parameters-and-lists.bel",
        include_str!("../examples/bootstrap/05-parameters-and-lists.bel"),
    ),
    (
        "06-control-frontmatter.bel",
        include_str!("../examples/bootstrap/06-control-frontmatter.bel"),
    ),
    (
        "07-evaluator-foundation.bel",
        include_str!("../examples/bootstrap/07-evaluator-foundation.bel"),
    ),
    (
        "08-evaluator-forms.bel",
        include_str!("../examples/bootstrap/08-evaluator-forms.bel"),
    ),
    (
        "09-evaluator-dispatch.bel",
        include_str!("../examples/bootstrap/09-evaluator-dispatch.bel"),
    ),
    (
        "10-evaluator-primitives.bel",
        include_str!("../examples/bootstrap/10-evaluator-primitives.bel"),
    ),
    (
        "11-evaluator-closures.bel",
        include_str!("../examples/bootstrap/11-evaluator-closures.bel"),
    ),
    (
        "12-higher-order.bel",
        include_str!("../examples/bootstrap/12-higher-order.bel"),
    ),
    (
        "13-unary-rationals.bel",
        include_str!("../examples/bootstrap/13-unary-rationals.bel"),
    ),
    (
        "14-number-representation.bel",
        include_str!("../examples/bootstrap/14-number-representation.bel"),
    ),
    (
        "15-numeric-tower.bel",
        include_str!("../examples/bootstrap/15-numeric-tower.bel"),
    ),
    (
        "16-places-and-mutation.bel",
        include_str!("../examples/bootstrap/16-places-and-mutation.bel"),
    ),
    (
        "17-stream-buffering.bel",
        include_str!("../examples/bootstrap/17-stream-buffering.bel"),
    ),
    (
        "18-reader-lists.bel",
        include_str!("../examples/bootstrap/18-reader-lists.bel"),
    ),
    (
        "19-reader-dispatch.bel",
        include_str!("../examples/bootstrap/19-reader-dispatch.bel"),
    ),
    (
        "20-reader-words.bel",
        include_str!("../examples/bootstrap/20-reader-words.bel"),
    ),
    (
        "21-self-hosted-bquote.bel",
        include_str!("../examples/bootstrap/21-self-hosted-bquote.bel"),
    ),
    (
        "22-printer-core.bel",
        include_str!("../examples/bootstrap/22-printer-core.bel"),
    ),
    (
        "23-printer-complete.bel",
        include_str!("../examples/bootstrap/23-printer-complete.bel"),
    ),
    (
        "24-sequence-utilities.bel",
        include_str!("../examples/bootstrap/24-sequence-utilities.bel"),
    ),
    (
        "25-loops-and-rounding.bel",
        include_str!("../examples/bootstrap/25-loops-and-rounding.bel"),
    ),
    (
        "26-random-prelude.bel",
        include_str!("../examples/bootstrap/26-random-prelude.bel"),
    ),
    (
        "27-mutation-and-ordering.bel",
        include_str!("../examples/bootstrap/27-mutation-and-ordering.bel"),
    ),
    (
        "28-file-and-recording.bel",
        include_str!("../examples/bootstrap/28-file-and-recording.bel"),
    ),
    (
        "29-arrays-and-tables.bel",
        include_str!("../examples/bootstrap/29-arrays-and-tables.bel"),
    ),
    (
        "30-templates.bel",
        include_str!("../examples/bootstrap/30-templates.bel"),
    ),
];

/// Reassemble the complete unchanged published source.
pub fn bootstrap_source() -> String {
    BOOTSTRAP_FRAGMENTS
        .iter()
        .map(|(_, source)| *source)
        .collect()
}

/// Evaluate the complete published source one fragment at a time.
pub fn eval_bootstrap(interpreter: &mut Interpreter) -> Result<usize, Error> {
    let mut forms = 0;
    for (name, source) in BOOTSTRAP_FRAGMENTS {
        let result = if *name == "21-self-hosted-bquote.bel" {
            interpreter.eval_bootstrap_bquote_source(source)
        } else {
            interpreter.eval_source(source)
        };
        forms += result
            .map_err(|error| Error(format!("bootstrap fragment {name}: {error}")))?
            .len();
        if *name == "15-numeric-tower.bel" {
            interpreter.register_native_source_shortcuts();
        }
    }
    interpreter.register_native_source_shortcuts();
    Ok(forms)
}
