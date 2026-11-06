# Bel in WebAssembly

An experimental Rust-hosted Bel bootstrap and a compiler written in Bel that emits WebAssembly. This is a standalone source distribution from randallb.com.

## libcwr Wasm milestone

The [libcwr source](libcwr/workflow.rbel.md) implements the first conversation and
controlled-action slice in literate rbel. Its adjacent tests run on Wasm with a
new deterministic host, persistence, restart, duplicate completion and uncertain
execution checks. See [setup and verification](libcwr/readme.md) and the
[generic test format](literate/testing.md). This is an implementation candidate;
independent review and hosted acceptance are tracked with the build, not implied
by this README. Existing CWR is reference-only and is not a dependency.

## Literate rbel foundation

The language/toolchain direction is **rbel** (pronounced “rebel,” short for
Randall Bel). The first implemented stage is the
[foundation chapter](literate/foundation.rbel.md): a reading path from no programming
knowledge, five original Bel definitions, worked evaluation traces, and 67
executable examples in one maintained source.

The chapter explains rbel using language-level rules. Setup, Rust host details,
source pins, and checking mechanics are in the separate
[bootstrap guide](literate/bootstrap.md). The
[reading audit](literate/reading-audit.md) records editorial review; beginner
comprehension has not yet been verified with a novice reader.

Run from this project directory with Python 3 and Rust 1.95.0:

```sh
python3 literate/check.py
python3 literate/check_test.py
```

The first command verifies SHA-256 pins for the foundation and complete imported
bootstrap, extracts definitions and examples under `target/literate`, then checks
them on the Rust host. `--extract-only` performs the byte checks and extraction
without Cargo. Generated output is ignored by Git. Expected results are data;
each example loads only the extracted definitions in a fresh interpreter.
The bootstrap reference remains unchanged. The foundation now also has a compiled
native verification route; CWR workflows remain future work. Definition drift and example failures
are errors; see the bootstrap guide for the source boundary and attribution.

## Foundation revision direction

Randall’s accepted HLQ direction is to teach from no prior programming knowledge,
explain rbel using implementation-independent language rules, and keep Rust in
separate bootstrap documentation. The chapter has been revised to that direction,
with executable checks and an editorial reading audit. The
[owning revision plan](../../../../../Projects/rbel/readme.md)
records the accepted decisions, strategic and tactical Intents, verification, and
next action. This planning link is for the connected workspace; standalone source
exports need a publication-safe version when this material is released.

## Native rbel compiler command

The [native compiler command](experiments/bel-rust-repl/wasm/native-command.md)
now reads literate source, checks examples as native code, and builds executables
without Rust, Node, or Python in its installed execution path. It bundles native
WABT tools; Apple's C compiler remains a build dependency.

```sh
target/rbel-command/bin/rbel build literate/unix-filter.rbel.md -o target/hello
printf 'hello, rbel!\n' | target/hello
target/rbel-command/bin/rbel check literate/foundation.rbel.md
```

The [reader chapter](literate/reader.rbel.md) maintains its explanation, definitions,
and 12 examples together. See the command guide for the relocatable bundle,
initial bootstrap, native-seed rebuild, supported syntax, and failure checks.

## Native macOS ARM64 command

The [literate Unix filter](literate/unix-filter.rbel.md) builds into a native
stdin/stdout command. It uppercases ASCII lowercase bytes and passes other bytes
through unchanged. Its executable needs only macOS system libraries at runtime.

After [native toolchain setup](experiments/bel-rust-repl/wasm/native/readme.md):

```sh
python3 literate/build-native.py
printf 'hello, rbel!\n' | target/bel-native/rbel-uppercase
```

The build checks seven literate examples, then uses the Bel-written self-hosted
compiler, Wasm, wasm2c, and Apple clang to produce the command. The older script route uses Rust during
bootstrap and source reading; the native command reads source in rbel instead.
The resulting executable uses no Rust runtime or external Wasm engine. The compiler now supports fixed-arity function values and lexical closures.
The [foundation compiler milestone](experiments/bel-rust-repl/wasm/higher-order.md)
compiles all 67 foundation examples and builds a
[filter using the foundation](literate/foundation-filter.rbel.md).
See [self-hosting](experiments/bel-rust-repl/wasm/selfhost.md) for that boundary.

## Run the interpreter

Tested with Rust 1.95.0.

```sh
cargo build --locked -p bel-rust-repl --bins
cargo run --locked -p bel-rust-repl
```

At the prompt, enter `:bootstrap`, then `(map atom '(a b (c)))`.

## Compile and verify Wasm

Requires Python 3 and Node/npm as well as Rust. Run from this archive's root:

```sh
npm install --prefix target/bel-wasm-tools --no-save --ignore-scripts wabt@1.0.39
python3 experiments/bel-rust-repl/wasm/verify.py
python3 experiments/bel-rust-repl/wasm/verify-lists.py
python3 experiments/bel-rust-repl/wasm/verify-emitter.py
```

The compiler supports documented subsets, not every Bel program. See `experiments/bel-rust-repl/wasm/readme.md`, `lists.md`, and `emitter.md` for language coverage, bootstrap boundaries, and limitations.

## Tests

```sh
cargo test --locked -p bel-rust-repl
cargo clippy --locked -p bel-rust-repl --all-targets -- -D warnings
```

## Source and attribution

`ORIGIN.json` identifies the upstream revision. The original bootstrap definition fragments remain unchanged. Local additions
and explicit batch-host allowances support literate examples, self-hosting, and
native filters. [NATIVE-ORIGIN.json](NATIVE-ORIGIN.json) records the recovered
pipeline and local adaptations. This package includes a minimal Cargo workspace,
its lockfile, documentation, and attribution files. Commands in upstream documentation that start with `direnv exec .` can be run without that prefix when the tools are installed.

The original project explores [Paul Graham's Bel](https://paulgraham.com/bel.html), including its published source as bootstrap fixtures. The host does not implement every Bel feature. See the original README for detailed limits and source references.
