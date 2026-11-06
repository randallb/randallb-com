# Running and checking the foundation

This guide is for working on the chapter and its tooling. The
[foundation chapter](foundation.rbel.md) explains rbel without requiring knowledge
of the software executing it. This guide describes that software and its limits.

## Current bootstrap

The current runner is the experimental Rust host in
`experiments/bel-rust-repl`. It supplies the starting primitive operations and
initial forms used by the chapter: `join`, `car`, `cdr`, `id`, `type`, `err`,
quotation, conditionals, function creation, and named definitions. The chapter
uses the five extracted Bel definitions on top of those facilities.

Some forms supplied initially by the host also have later published Bel macro
definitions. Those later fragments are not loaded by this chapter's checker.
“Starting operation” in the chapter describes a language boundary, not a claim
that it must always be implemented in Rust.

Randall's direction is to replace Rust as soon as reasonable. The chapter and
its language-level examples should remain valid across that replacement. A
replacement must establish the operations and evaluation rules these examples
rely on; passing this small corpus alone would not establish full Bel conformance.
The present checker is a Rust execution route, not a portable chapter runtime.

## Run the checks

Install Python 3 and Rust/Cargo 1.95.0. The project has no third-party Rust
package dependencies. From the `projects/bel-wasm` directory:

```sh
python3 literate/check.py
python3 literate/check_test.py
```

`check.py` validates the source pins, then writes generated files:

- `target/literate/foundation.rbel`: definition blocks in chapter order.
- `target/literate/foundation-examples.bel`: named expression/expected-value rows.

It runs `rbel-check-examples` through Cargo. Each example starts with a fresh
interpreter, loads all five extracted definitions, then evaluates its expression.
Examples do not share bindings or changes. The pedagogical reading order explains
prerequisites before use; the harness installs all five definitions before every
example. It does not prove that the chapter is free of forward dependencies.
That requires the reading audit described below.

Expected values are parsed as data, never executed. Comparison uses bounded
printed values; tests of pair identity belong inside an expression using `id`.
Parse, evaluation, rendering, and mismatch errors fail the command. Mismatches
name the example. There is no expected-error block notation yet: the chapter
checks skipped errors using unchosen branches, while the runner's integration
tests cover evaluation errors and malformed rows.

For byte verification and extraction without Cargo:

```sh
python3 literate/check.py --extract-only
```

An alternate chapter can be checked with `--chapter PATH`. It must extract the
same pinned definition bytes. This is useful for checking a proposed revision
or intentionally demonstrating a failing example. An unsuccessful check may
leave older generated output on disk; only a successful command verifies it.

Edit the chapter, not the ignored generated files. No network fetch, commit,
export, or deployment occurs.

## Authoring blocks

A fence headed `rbel definition=NAME` contributes its content bytes to the
extracted source. Concatenation preserves whitespace and source-header comments.
A fence headed `rbel example=NAME` contains exactly one expression, a separator
line `=>`, and exactly one expected value. Use unique names across both kinds.
Ordinary Markdown fences are explanatory. Unsupported rbel fence attributes,
duplicate names, missing separators, and unclosed fences fail extraction. The
Rust reader and row checker reject invalid Bel syntax or malformed example rows.

## Preserved source

The chapter's definition blocks concatenate byte-for-byte to
`experiments/bel-rust-repl/examples/bootstrap/00-foundation.bel`, imported from
`getworkloop/wlp-mono` revision `2aabe143b8a1ed65f4adc1b452ad7074581b5091`.
[ORIGIN](../ORIGIN.json) and [NOTICE](../NOTICE.txt) preserve import attribution.

[source-pin.json](source-pin.json) records SHA-256 for that foundation and the
ordered complete 31-fragment bootstrap. The checker verifies both the reference
and extracted foundation against the pin, checks provenance against ORIGIN, and
verifies the complete snapshot. The pins were also compared with the actual
upstream revision during the first-stage implementation.

Definition changes require an explicit language/source decision rather than
quietly updating the reference or hashes to make a failing check pass. Explanation
and examples can evolve without changing the original definitions.

## Current execution limits

The host is experimental and does not implement every Bel facility. Input size,
syntax depth, evaluation steps/depth, and rendering are bounded; exceeding a
limit is an error, not a successful example. See
[host documentation](../experiments/bel-rust-repl/readme.md) and
[limit constants](../experiments/bel-rust-repl/src/limits.rs).
These bounds describe today's runner, not a requirement that every future
implementation use the same numbers. OS I/O, scheduling, persistent continuations,
and other facilities outside this chapter have separate implementation limits.

For changed tooling, run the relevant Rust integration tests:

```sh
cargo test --locked -p bel-rust-repl --test literate
```

The full Rust suite passed 86 tests during the first stage. Strict Clippy then
reported three existing function-length violations in `eval.rs` and
`quasiquote.rs`; that result is separate from chapter correctness.

## Reading review and decision history

The [reading audit](reading-audit.md) records prerequisite order and the worked
traces checked in this revision. It is an editorial review. Actual beginner
comprehension remains to be checked with a reader.

The planning home is `randallb.com/Projects/rbel`. Randall accepted no assumed
programming knowledge, then corrected the execution explanation to hide Rust
and preserve implementation-independent language rules. These replies retain
authorship and reply context:

- [No knowledge assumed](cwp://local.randalls-macbook-pro/runtimes/4cac58c87863ac806d760c8b519e39cadacff2dc08a5c91aa18fa80e657d62ea/messages/659c6e9d5c8d4476ae63c425d6e27e74).
- [No programming experience](cwp://local.randalls-macbook-pro/runtimes/4cac58c87863ac806d760c8b519e39cadacff2dc08a5c91aa18fa80e657d62ea/messages/1770d96e65124e3da5a68220fc359a01).
- [Rust correction](cwp://local.randalls-macbook-pro/runtimes/4cac58c87863ac806d760c8b519e39cadacff2dc08a5c91aa18fa80e657d62ea/messages/15e409e3338b41e5a01200f110a04440).
- [Implementation-independent rules accepted](cwp://local.randalls-macbook-pro/runtimes/4cac58c87863ac806d760c8b519e39cadacff2dc08a5c91aa18fa80e657d62ea/messages/6794bebc047244338efdc76b05706703).
- [Revision authorized](cwp://local.randalls-macbook-pro/runtimes/4cac58c87863ac806d760c8b519e39cadacff2dc08a5c91aa18fa80e657d62ea/messages/92d3a38075b5473987e707d2f5abf6ab).

## Native continuation

After Randall's read-through, the native pipeline was restored in the maintained
checkout. The [literate Unix filter](unix-filter.rbel.md) now builds a macOS ARM64
stdin/stdout executable using `python3 literate/build-native.py` after the
[native setup](../experiments/bel-rust-repl/wasm/native/readme.md).
It has no Rust or external Wasm engine dependency at runtime. The small compiler
supports direct first-order calls, not the entire foundation's higher-order
functions. The current host and native pipeline passed 89 release-mode Rust
tests, seven native chapter examples, native regression, and self-hosting checks.
The foundation's 67 examples and five extraction checks also pass after integration.
Original definition hashes are unchanged; provenance checks compare the original
source identity, while ORIGIN records subsequent local adaptations.

## Foundation compiled natively

The compiler has since gained fixed-arity function values and lexical closures.
All 67 foundation examples now pass as native code, retaining the original
five definitions. The [higher-order guide](../experiments/bel-rust-repl/wasm/higher-order.md)
records the proof and remaining language limits; the
[foundation filter](foundation-filter.rbel.md) uses all/some/reduce and captures
bindings in a real stdin/stdout executable. Rust remains a build-time bootstrap
and reader dependency, rather than a dependency of the produced executables.

## Native reader and compiler command

The [native command guide](../experiments/bel-rust-repl/wasm/native-command.md)
now provides the normal literate build/check interface without Rust, Node, or
Python. The [reader chapter](reader.rbel.md) explains the rbel reader and includes
12 examples. The old Python/Rust check command above remains a development
reference and source-pin check; it is no longer the only execution route.
The initial tool bootstrap can still use Rust/Node, while an existing native
command can seed its replacement without either. Packaging scripts are separate
from the installed command's native operations.
