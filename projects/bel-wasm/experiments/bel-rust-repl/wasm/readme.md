# Portable Bel function compiler

The [expression emitter](emitter.md) is a Bel compiler component compiled to
Wasm: it accepts a runtime syntax tree and produces executable WAT.

Use [`--lists`](lists.md) for first-order recursive functions over pairs,
symbols, and characters. The original boolean workflow below remains available.

The compiler in `compiler.bel` validates one Bel function and generates Wasm
text (WAT). WABT assembles that text into an import-free binary. An ordinary
Node WebAssembly instance runs the exported `choose` function; execution needs
neither this project nor its Rust host.

## Build and run

Run from the repository root with Rust/Cargo, Python 3, and Node/npm installed.
The repository development shell supplies Rust. WABT is installed in ignored
build output, with a pinned version and no install scripts:

```sh
direnv exec . cargo build --locked -p bel-rust-repl --bins
npm install --prefix target/bel-wasm-tools --no-save --ignore-scripts wabt@1.0.39
python3 experiments/bel-rust-repl/wasm/compile.py \
  experiments/bel-rust-repl/wasm/choose.bel target/bel-wasm/choose.wasm
target/bel-wasm-tools/node_modules/.bin/wasm-validate target/bel-wasm/choose.wasm
target/bel-wasm-tools/node_modules/.bin/wasm-objdump -x target/bel-wasm/choose.wasm
node --input-type=module -e '
import {readFileSync} from "node:fs";
const bytes = readFileSync("target/bel-wasm/choose.wasm");
const module = new WebAssembly.Module(bytes);
const instance = new WebAssembly.Instance(module);
console.log("valid", WebAssembly.validate(bytes));
console.log("imports", WebAssembly.Module.imports(module));
console.log("nil", instance.exports.choose(0));
console.log("t", instance.exports.choose(1));
'
```

Expected: `valid true`, `imports []`, `nil 0`, `t 1`. The example is
`(fn (x) (if x 't 'nil))`. Its branch is selected by the argument at runtime.
The `.wat` file beside the `.wasm` retains the exact Bel-generated text. Node
26.12.0 and WABT 1.0.39 were used for the initial demonstration. The runner
above uses only Node's filesystem and standard WebAssembly APIs.

## Supported source and values

After the existing host reader expands reader abbreviations, the grammar is:

```text
program   := (fn (parameter) expression)
expression := nil | t | parameter
            | (quote nil) | (quote t)
            | (if expression expression expression)
```

`parameter` must be a symbol satisfying the published Bel `variable` predicate,
and must not be `fn`, `if`, or `quote`. In particular, the self-evaluating
literals `nil`, `t`, `o`, and `apply` cannot be parameter names. Exactly one
program and one body expression are required. Conditionals may be nested. Quote
shorthand is accepted by the unchanged host reader. A proper list written with
an equivalent dotted-nil tail has the same meaning.

The sole export has signature `choose(i32) -> i32`. The public value domain is
`0` for Bel nil and `1` for Bel t, for both arguments and results. Only nil is
false. WAT conditionals test for zero; the documented ABI admits only 0 and 1.
Other Bel values, including numeric zero, are rejected rather than being
mistaken for nil. Calling with other i32 values is outside this ABI.

Arithmetic, arbitrary quoted symbols, pairs as runtime values, calls, closures,
multiple parameters or body forms, and other syntax are unsupported. Unsupported
source reports `unsupported-wasm-source` and exits unsuccessfully. Malformed
reader input reports the host's parse error. Resource limits can also reject
large inputs; this demonstration does not enlarge the host's limits.

## Compiler and bootstrap boundary

Bel checks the grammar, chooses all instructions and constants, and constructs
the complete module as a tree of character fragments. No fixture-specific module
or precomputed result is embedded in the host or Python driver.

`bel-eval-text` is generic bootstrap support: it loads the unchanged published
source, evaluates library files, passes all parsed data forms to a named Bel
function, and prints its final character tree. Character trees consist of
characters, nil, and pairs of character trees, traversed left to right. It
validates the complete result before writing stdout, with a 16 KiB text limit
and 32,768 traversal-step limit. Long flat-string concatenation in Bel exceeded
the existing evaluator depth limit; fragments keep this small path practical.

The direct compiler call is:

```sh
target/debug/bel-eval-text --call wasm-compile-forms \
  experiments/bel-rust-repl/wasm/choose.bel \
  experiments/bel-rust-repl/wasm/compiler.bel > target/bel-wasm/choose.wat
```

The Python driver invokes this call and WABT. It removes previous `.wasm` and
`.wat` products at the requested output path before compiling, so failure cannot
leave a stale successful artifact. Choose an output basename reserved for build
products. The source must be distinct from both products. It assembles in a
private temporary directory and publishes products after assembly succeeds.
`--host` and `--wat2wasm` allow other built-host and WABT paths.

Rust still runs the compiler during bootstrap. This is a portable compiled
program, not compiler self-hosting or a Rust-free build. No evaluator semantics
or published source fixtures were changed, and the evaluator macro bootstrap
cycle did not block this path.

## Full regression

```sh
direnv exec . cargo test --locked -p bel-rust-repl
direnv exec . cargo clippy --locked -p bel-rust-repl --all-targets -- -D warnings
python3 experiments/bel-rust-repl/wasm/verify.py
```

The external regression validates modules with WABT and Node, checks both
runtime branches and empty imports, changes the non-nil branch result and
observes the changed output, exercises nested and constant tests, rejects
unsupported/malformed sources after seeding stale products, and recompiles the
original successfully. It retains source variants, WAT and Wasm under
`target/bel-wasm-acceptance`. Redirect its output to a file to retain commands
and observed results. Cargo tests also cover the generic batch transport,
partial-output prevention, bounds, source-dependent lowering, and the existing
host/source-fidelity regressions.
