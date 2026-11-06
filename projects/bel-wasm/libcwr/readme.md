# libcwr: the first literate Wasm workflow

The [maintained chapter](workflow.rbel.md) contains the rules, rbel implementation,
source mapping, examples and adjacent tests for one conversation/action slice.
The generic [test runner](../literate/testing.md) compiles those expressions with
the existing rbel compiler and executes them with WebAssembly. The test host and
acceptance cases are newly authored; no CWR runtime, fixtures, adapters or tests
are dependencies.

From `projects/bel-wasm`, with the existing native compiler bundle, Python 3 and
Node 24:

```sh
python3 literate/test.py libcwr/workflow.rbel.md \
  --entry cw-engine --host libcwr/host.mjs \
  --report target/libcwr/results.json --docs target/libcwr/protocol.md --verbose
python3 libcwr/verify.py
```

The first command runs the actual compiled workflow. The second adds deliberate
rule/expectation mutations, malformed inputs, Wasm traps and resource exhaustion,
cleanup/recovery, pinned-source checks, and existing native/Wasm examples.
Generated reports and docs stay under ignored `target/`; they are local products,
not publication. The source hash in the generated document must match this
chapter. `--rbel /path/to/bundle/bin/rbel` selects a relocated compiler bundle.

## From a fresh source checkout

Use macOS ARM64 and Apple's command-line tools. Rust 1.95.0, Python 3, Node/npm,
and WABT are bootstrap dependencies of the existing compiler, not a CWR engine.
The ordinary test run uses the native compiler bundle, Python and Node; Rust
performs no protocol evaluation.

```sh
cargo build --locked -p bel-rust-repl --bins
npm install --prefix target/bel-wasm-tools --no-save --ignore-scripts wabt@1.0.39
python3 experiments/bel-rust-repl/wasm/verify-selfhost.py
python3 experiments/bel-rust-repl/wasm/command/setup-tools.py
python3 experiments/bel-rust-repl/wasm/command/build-command.py
python3 libcwr/verify.py
```

The separate libcwr CI job follows this source bootstrap. Its macOS runner label
follows [GitHub's runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners).
The unrelated existing site/CWR job remains separate and is not libcwr acceptance.

## What the host supplies

`host.mjs` is an isolated test adapter, not a public ingress API or a second
protocol implementation. It supplies authenticated fictional context, stores
opaque rbel state, runs emitted actions, and returns execution evidence. Every
transition decision is in `cw-step` in the chapter. `configure`, `fault`,
`snapshot`, `redeliver` and `observe` are trusted fixture controls and must never
be exposed to untrusted request authors.

The deterministic `store--text` action atomically replaces an invocation-keyed
UTF-8 artifact and returns the SHA-256 of the requested text. Repeating the exact
input at that exact destination is known safe. `append--text` appends the same
text and is intentionally unsafe to repeat automatically. Its lost-receipt test
checks the real file to prove it was not appended twice. No real participants,
network calls or production state are involved.

The host commits the returned state before executing an effect. A failed commit
prevents execution. Adapter receipts bind invocation, claim, target, input and
result. On restart, the protocol Wasm instance is destroyed and recreated and
values are re-encoded from disk. The separate compiled test-driver instance
keeps only the test continuation. Disk records contain values, never Wasm memory
addresses. State is serialized by one owning host using atomic file replacement;
this verifies process-interruption recovery, not concurrent writers, distributed
storage, power-loss durability, or exactly-once external effects.

## Boundaries

This is a tests-first interface to working rbel code, not a standalone CLI,
native libcwr package, provider adapter, Workloop integration, or full protocol
migration. Only successful action completion is implemented in this slice.
The internal tuple ABI assumes host-validated canonical references and checked
configuration; it is not a public wire API. The chapter lists all unmigrated
standards and the exact reference mapping. The consulted source is read-only and
its revision and hashes are in `reference-baseline.json`; it is not needed to run
the tests. Protocol changes require an explicit decision, rather than changing
an assertion to follow an implementation bug.
