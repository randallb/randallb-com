# A Bel compiler that builds its successor in Wasm

`self-compiler.bel` is a complete compiler for a Bel core with fixed-arity function values and lexical closures,
written in a first-order subset of that core. Rust evaluates it once to bootstrap `stage0`.
That import-free Wasm compiler compiles the same source data to `stage1`;
`stage1` builds `stage2`, and `stage2` builds `stage3`. Each successor's WAT and
Wasm binary are byte-for-byte identical to the bootstrap products.

This is self-hosting for the documented subset. It is not a compiler for all
Bel. The earlier boolean compiler, list compiler, and expression emitter remain
separate experiments; this compiler provides a complete path through its own
source instead of extending their macro-heavy implementation.

For a native Unix executable built from this core through Wasm and C, see
[the native byte-filter example](native/readme.md).

## Reproduce

From this standalone source distribution's root, with Rust/Cargo, Python 3 and
Node/npm available:

```sh
cargo build --locked -p bel-rust-repl --bins
npm install --prefix target/bel-wasm-tools --no-save --ignore-scripts wabt@1.0.39
python3 experiments/bel-rust-repl/wasm/verify-selfhost.py
```

The verifier saves all generations, fixture products, logs produced by the
caller, and `selfhost-results.json` under `target/bel-selfhost`. Redirect its
stdout/stderr to a file to retain its verification summary. `--skip-bootstrap`
reuses stage0 and the reader data; it does not demonstrate a current-source
bootstrap rebuild.

The bootstrap commands used by the verifier are equivalent to:

```sh
mkdir -p target/bel-selfhost
target/debug/bel-read-json experiments/bel-rust-repl/wasm/self-compiler.bel \
  > target/bel-selfhost/compiler.ast.json
target/debug/bel-eval-text --max-text-bytes 8388608 --max-eval-depth 4096 \
  --call sc-compile experiments/bel-rust-repl/wasm/self-compiler.bel \
  experiments/bel-rust-repl/wasm/self-compiler.bel \
  > target/bel-selfhost/stage0.wat
target/bel-wasm-tools/node_modules/.bin/wat2wasm \
  target/bel-selfhost/stage0.wat -o target/bel-selfhost/stage0.wasm
python3 experiments/bel-rust-repl/wasm/self-compile.py \
  target/bel-selfhost/stage0.wasm target/bel-selfhost/compiler.ast.json \
  target/bel-selfhost/stage1.wasm
```

After parsing data and bootstrapping stage0, successor generation runs in a
fresh temporary directory containing only the compiler Wasm binary, a generic
Node data adapter, and JSON input. No Rust executable, Bel evaluator, compiler
library, or host compilation callback is called. WABT remains the external WAT
assembler. This verifier supplies already-read source data. The
[native command](native-command.md) now includes a compiled reader written in rbel
and can reproduce the compiler text from source without the Rust reader.

For the compiled foundation, lexical closure behavior, native verification, and
remaining language limits, see [higher-order compilation](higher-order.md).

## Supported core

```text
program    := definition+
definition := (def name (parameter*) expression)
expression := nil | t | parameter | function-name | primitive-name | character | string
            | (quote constant)
            | (if expression*)
            | (fn (parameter*) expression)
            | (join expression expression) | (id expression expression)
            | (car expression) | (cdr expression) | (type expression)
            | (nom expression) | (sym expression) | (err expression)
            | (expression expression*)
constant   := nil | t | symbol | character | (constant . constant)
```

The first definition is exported as `choose` with its declared number of i32
parameters and one i32 result. Calls may be forward, mutually recursive, or
self-recursive, and require exact arity. Names are nonempty lowercase ASCII
letters, digits, and hyphens, excluding the reserved syntax/primitive names in
`sc-reserved`, and nil/t. Parameters must be distinct; they may shadow function or primitive names,
but not the syntax names if/fn/def/quote/lit or nil/t. Definitions are parsed as data and never installed in the
bootstrap host. Unlike the previous list compiler, target names do not need a
snapshot of the host bootstrap namespace.

Only nil is false. `id` compares atomic or pair identity. Literal strings and
quoted constants are initialized once per source occurrence, so returning the
same literal from repeated calls preserves identity. Distinct pair-literal
occurrences have distinct identity. `join` makes fresh pairs. `car`/`cdr` of nil
return nil; other atoms trap. `type` returns `pair`, `symbol`, or `char`.
`nom` returns a fresh list naming an interned symbol; `err` traps.

Source character/string/symbol literals support printable ASCII, tab, LF, and
CR. Runtime interned names may contain any Unicode scalar. Exact numeric and explicit finite-float literals
and the source-defined arithmetic library are now supported; see
[integers](../../../literate/integers.rbel.md). Macros, dynamic scope, mutation, OS I/O, threads, and
continuations are outside this core. Exhaustive shape/name/arity checks reject
unsupported expressions, including expressions in dead branches.

## Runtime and bootstrap limits

Values use the tagged i32 ABI described in `lists.md`: nil=0, t=1, fixed type
symbols 9/17/25, tagged characters, interned symbols, and aligned pair handles.
The module exports `choose`, `join`, `intern`, `symbolname`, and `memory`. The runtime has no
imports. Its arena starts at 4096, grows from one page to at most 1024 pages
(64 MiB), and has no collection or reset. Discard an instance to reclaim it.
Recursion uses the WebAssembly engine stack; deep or cyclic data can trap.

The separate `self-wat.mjs` adapter accepts at most 8 MiB JSON, 524288 pair
constructions, and 128 nesting levels. It writes at most 8 MiB text after at
most 33554432 character-tree traversal steps. Strings represent symbols,
arrays represent proper lists, null represents nil, `{"pair":[a,d]}` represents
a dotted pair, and `{"char":"x"}` represents one Unicode scalar character.
The adapter transports data and flattens output; it does no source validation,
macro expansion, instruction selection, or lowering.

`bel-read-json` serializes the existing Rust reader's values without evaluating
them. The batch evaluator's explicit `--max-text-bytes` option permits 64 KiB
through 16 MiB. `--max-eval-depth` permits 1 through 4096 and runs evaluation on
a 64 MiB native thread stack. The verifier opts into 8 MiB text and depth 4096
for this bootstrap. Default batch bounds and the REPL bounds are unchanged;
no primitives or published Bel bootstrap definitions were changed.

## Verification

The verifier checks three successor generations for exact WAT/binary equality,
validates each module, and exercises all three successors on 12 program
fixtures: runtime branches, changed output, nested conditionals, forward and
multiple-argument calls, literal identity, symbol names, characters/strings,
types, nil access, and recursive pair copying. Program instances have no imports.

It also edits the compiler's own export-name literal, compiles that modified
compiler with its successor, and verifies that the modified compiler emits a
program exporting the changed name. This checks that compiler source affects
behavior beyond reproducing one fixed output.

Thirty invalid-source cases are rejected by each successor. Malformed JSON,
missing assemblers, stale-product cleanup, subsequent successful compilation,
and protection against overwriting the input compiler are checked. The normal
Rust tests and the earlier three external Wasm regression scripts cover the
existing host and previous compilation paths separately. Equality across these
stages is a useful bootstrap consistency check, not an independent proof of
Bel semantics: stages share the source, runtime, reader data, assembler, and
WebAssembly engine.
