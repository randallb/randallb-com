# A Bel expression emitter running in Wasm

`expression-emitter.bel` is compiled by the Bel first-order compiler into an
import-free Wasm module. That module receives a syntax tree at runtime and
returns a tree of WAT characters. WABT assembles its output into a second
import-free module, whose `choose(i32)->i32` export runs in ordinary Node.

Rust bootstraps the first-order compiler to build the emitter once. Neither
runtime emission nor running the generated program invokes Rust, evaluates Bel
on the host, or calls a host compilation callback. This is a compiler component
running in Wasm; the whole compiler is not yet self-hosted.

## Reproduce

From the repository root, with Rust, Python and Node available:

```sh
direnv exec . cargo build --locked -p bel-rust-repl --bins
npm install --prefix target/bel-wasm-tools --no-save --ignore-scripts wabt@1.0.39
python3 experiments/bel-rust-repl/wasm/compile.py --lists \
  experiments/bel-rust-repl/wasm/expression-emitter.bel target/bel-emitter/emitter.wasm
python3 experiments/bel-rust-repl/wasm/emit.py \
  experiments/bel-rust-repl/wasm/choose.ast.json target/bel-emitter/choose.wasm
node --input-type=module - target/bel-emitter/choose.wasm <<'JS'
import { readFileSync } from 'node:fs';
const module = new WebAssembly.Module(readFileSync(process.argv[2]));
const { choose } = new WebAssembly.Instance(module).exports;
console.log(WebAssembly.Module.imports(module), choose(0), choose(1));
JS
```

Expected result: an empty import list, `0`, and `1`. The emitted program's
public values remain nil=0 and t=1; other inputs are outside this boolean ABI.

Run the complete regression, including rebuilding the emitter:

```sh
python3 experiments/bel-rust-repl/wasm/verify-emitter.py
python3 experiments/bel-rust-repl/wasm/verify-lists.py
python3 experiments/bel-rust-repl/wasm/verify.py
direnv exec . cargo test --locked -p bel-rust-repl
```

The emitter verifier builds a small semantic fixture too, then executes six
runtime AST variants in temporary directories containing only the emitter,
generic data adapter and input data. It assembles and runs each returned
program, compares native and compiled emitter output, rejects unsupported syntax
with seeded stale outputs, and verifies failure recovery. Sources, WAT, binaries
and hashes remain under `target/bel-emitter-acceptance`. `--skip-build` can
reuse previously built emitter/feature artifacts for a focused runtime check;
that alone does not establish a current-source rebuild.

## Emitter input grammar

```text
program := (fn (parameter) expression)
expression := nil | t | parameter | (quote nil) | (quote t)
            | (if expression expression expression)
```

Parameters are nonempty symbol names containing lowercase ASCII letters and
hyphens, excluding the self-evaluating nil, t, o and apply. The emitter
validates shape, arity, names and every expression in Wasm. Unknown variables,
unsupported operators/literals, malformed lists and extra forms trap with a
clear CLI failure. It validates the full input even if a target branch would be
dead. Only nil is false within the supported value domain. Unsupported forms are
not evaluated, and failure removes old `.wat` and `.wasm` output before
attempting emission. A valid input can be emitted successfully afterward.

## Runtime data and text boundary

The emitter uses the [first-order compiler's value ABI](lists.md): nil=0,
reserved type symbols, tagged Unicode characters, and allocated pairs. It
exports `choose(i32)->i32`, `join`, `intern`, and `memory`. `intern` and `nom`
operate on the symbol-name table inside the module; a caller never supplies a
symbol lookup callback. Literal strings are initialized once per source
occurrence, preserving identity. `nom` returns fresh character lists.

`choose.ast.json` represents the example as data:

```json
["fn", ["x"], ["if", "x", ["quote", "t"], ["quote", "nil"]]]
```

The generic Node adapter maps strings to symbols via `intern`, arrays to proper
lists via `join`, JSON null to nil, and `{ "pair": [car, cdr] }` to a dotted
pair. Numbers and other JSON values are outside this transport. It performs no
Bel parsing, source validation, operator selection, lowering or WAT generation.
Its output walk only flattens characters/pairs. Python manages files and invokes
Node/WABT; all compilation decisions remain in the compiled Bel source.

The adapter bounds input to 64 KiB, 32768 pair constructions and 128 levels;
output is limited to 64 KiB and 262144 traversal steps. Symbol names are proper
lists of at most 4096 Unicode scalars; malformed, cyclic and oversized names
trap. The finite arena, instance lifetime and engine stack limits in lists.md
also apply. These are small-program experimental limits, not a general-purpose
runtime or untrusted execution service.

## Bootstrap changes and remaining work

The first-order compiler protects the loaded bootstrap namespace from function
redefinitions while allowing local parameter names; see lists.md. The name
snapshot and guard run in Bel before compiler-library loading.

The first-order compiler now supports multiple/zero parameters, exact-arity
calls, ASCII character/string literals, quoted symbol names, `nom` and `err`.
The emitter uses those facilities directly. There is no new Rust semantic
backend and no change to the evaluator or published Bel fixtures.

The emitted emitter's WAT is larger than the old generic batch-writer bound.
`bel-eval-text` now accepts up to 64 KiB and 262144 character-tree traversal
steps, with complete validation before writing stdout. Its oversized/cyclic
output tests still reject safely. The REPL/evaluator's limits remain unchanged.
Compiler-local primitive string traversal avoids the published general-purpose
string predicate's evaluation depth cost while leaving the published source
untouched.

A complete compiler in Wasm still needs more of its own source expressed in the
supported core, or further supported constructs. The later self-hosting check
must have that compiler build its successor and exercise mutation and rejection
without invoking Rust. This milestone does not establish that result.
