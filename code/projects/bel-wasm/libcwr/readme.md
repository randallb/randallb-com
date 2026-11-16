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

The portable CI route needs Rust 1.95.0, Python 3 and Node/npm. The existing Rust
reader produces syntax data; the existing rbel compiler executes on Wasm and
makes every compilation decision. Neither Rust nor Python evaluates protocol
rules. The bootstrap shim supports only `read` and `emit`; it is test-build
tooling, not a native rbel distribution or a libcwr CLI.

```sh
cargo build --locked -p bel-rust-repl --bins
npm install --prefix target/bel-wasm-tools --no-save --ignore-scripts wabt@1.0.39
python3 experiments/bel-rust-repl/wasm/verify-selfhost.py
python3 literate/bootstrap-wasm.py
python3 libcwr/verify.py --rbel target/rbel-wasm-bootstrap/bin/rbel --skip-native
```

The Linux libcwr CI job follows this route. `--skip-native` explicitly leaves the
two native-command regressions unverified on that route; all 28 portable
verification groups still run actual compiled Wasm. Default verification on the
native bundle runs all 30 groups, including native regressions. Both compiler
routes produce identical Wasm for the authored workflow in local verification.
The unrelated existing site/CWR job is separate and is not libcwr acceptance.

For the normal native compiler on macOS ARM64, after the first three bootstrap
commands above, run:

```sh
python3 experiments/bel-rust-repl/wasm/command/setup-tools.py
python3 experiments/bel-rust-repl/wasm/command/build-command.py
python3 libcwr/verify.py
```

This native bootstrap also needs Apple's command-line tools. Its installed
compiler does not use Rust, Node or Python for reading and compiling. The test
runner uses Python orchestration and Node's Wasm engine. The source-bootstrap CI
route was added after GitHub could not acquire a macOS ARM64 runner; no cancelled
job is counted as a passing check.

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

## Embedding and native host

The [embedding package](embedding/readme.md) builds a standalone import-free Wasm
core and a native executable containing that same module plus its Wasm runtime.
The native host supplies local snapshots, receipts and deterministic actions.
Use the [native build guide](embedding/native-build.md) for runtime bootstrap and
licensing. Acceptance runs this chapter through the original driver, standalone
Wasm package and native process, comparing all assertions and host traces.
