# Compiling the literate foundation and lexical closures

The Bel-written compiler now compiles the unchanged five foundation definitions,
all 67 expressions from the [foundation chapter](../../../literate/foundation.rbel.md),
function values, and fixed-arity lexical closures. A native filter also uses
all, some, reduce, atom, and no through that compiled path.

## Reproduce

From `projects/bel-wasm`, after the usual Rust/Python/Node/WABT setup and native
runtime download described in [the native guide](native/readme.md):

```sh
cargo build --locked -p bel-rust-repl --bins
python3 experiments/bel-rust-repl/wasm/verify-selfhost.py
python3 experiments/bel-rust-repl/wasm/verify-foundation-native.py
python3 literate/build-foundation-native.py
python3 literate/verify-foundation-filter.py
printf 'hello, rbel!\n' | target/bel-native/rbel-foundation-uppercase
```

The final line prints `HELLO, RBEL!` and a newline. The
[literate filter source](../../../literate/foundation-filter.rbel.md) explains
how it uses the existing foundation and letter mapping. Its builder extracts
and checks those sources; the original foundation definition bytes and SHA-256
pins are unchanged.

## Language coverage

Beyond the prior first-order core, the compiler supports:

- Named functions and primitives as values, with stable identity per module.
- Calls through parameters or an arbitrary function-valued expression.
- `(fn (parameters...) body)` with fixed, distinct parameters, including zero
  parameters. A fresh closure captures the surrounding lexical bindings by value.
- Escaping, nested, and transitively captured closures. A closure retains its
  environment for the lifetime of the module instance.
- Parameter shadowing of named functions and primitives. Special syntax names
  `if`, `fn`, `def`, `quote`, and `lit`, plus nil/t, cannot be parameters.
- The chapter's multi-branch `if`, optional final fallback, and nil result when
  no condition matches and no fallback exists. Only nil is false.

Ordinary known named calls remain direct Wasm calls and are checked for arity
at compile time. Function-valued calls use a generated dispatcher and check
arity at runtime. Non-function values, wrong dynamic arity, and invalid closure
handles trap. Primitive calls in this compiled subset require exact arity;
the Rust host's omitted-primitive-argument defaults are not implemented here.

Bodies and branches are compiled even when statically unchosen. Unsupported
syntax, unknown free variables, duplicate parameters, rest parameters, and
multiple-body fn forms are rejected. Optional/destructuring/typed parameters,
macros, apply, mutation, set, dynamic scope, OS I/O, threads, continuations,
and the broader Bel numeric tower remain outside this compiler.

The chapter is fully covered; this does not claim full Bel compilation.
Source character literal support remains printable ASCII, tab, LF, and CR.
The native byte interface can transport all 256 byte characters.

## Compiler and runtime design

`self-compiler.bel` remains written in a first-order subset of its own language.
It emits all function/closure behavior itself, without a host compilation hook
or a target interpreter. A lambda's syntax path gives it a stable internal name.
Each lambda becomes a native Wasm function plus an entry in the dispatcher.

Function values are pair-backed closures containing a private marker, a unique
code-identity pair, and an environment of captured name/value bindings. The
runtime uses the same tagged values and arena as the preceding compiler. Named
and primitive function values are initialized once; fn creation allocates a
fresh closure. Captured names resolve against the saved environment; inner
parameters take precedence. Named recursive functions still resolve globally.

This compiled function representation is internal. General inspection or
construction of the published Bel literal closure representation is not a
supported compatibility boundary. Atom/pair type and function identity behavior
used by the chapter are checked explicitly.

Arena lifetime and memory limits are unchanged: no garbage collector, at most
64 MiB per instance, instance destruction for reclamation. The native filter
creates a fresh instance per input chunk. Call depth is bounded by the native
adapter/runtime's 1024-frame limit; dynamic dispatch adds frames. Unsupported
programs and resource failures remain failures, not accepted equivalence.

The native builder accepts an optional `--adapter` for a generic C entry point.
The default remains the byte filter. `value-main.c` supplies a selector to choose
and serializes its returned value as JSON. The generated module exports
`symbolname` for symbol transport. The C adapter performs no expression evaluation,
closure binding, function dispatch, or compilation. WABT-generated unused local
assignments are excluded from C warnings-as-errors; other warning checks remain.

## Verification and evidence

`verify-foundation-native.py` extracts the actual chapter and checks the source
pins. It generates one selectable module containing the original definitions
and original example expressions, compiles it through the Wasm compiler and
wasm2c, and builds a real native executable. Every example runs in a fresh
process/module instance from an otherwise empty temporary directory, with an
empty PATH. Results are serialized and compared with each original expected
value, parsed as data rather than executed.

All 67 chapter examples pass. Fifteen additional cases cover captured values,
escaping functions, nested/transitive captures, parameter shadowing, named and
primitive identity, fresh closure identity, zero/multiple arguments, literal
identity, and closure truth/type. Four bad dynamic calls trap. Six malformed or
unsupported closure programs are rejected, including an unknown free variable
inside an unchosen branch.

`target/bel-foundation-native/results.json` records counts, platform, source hash,
binary hash, and system-library dependencies. The executable is
`target/bel-foundation-native/rbel-foundation-check`; for example:

```sh
target/bel-foundation-native/rbel-foundation-check same-suffix
```

Its JSON answer is `"t"`. The source/AST preparation still uses the Rust reader;
this is independent of Rust evaluation, not an independent validation of reading
syntax. The host and target also share the published definitions and the chosen
value ABI. The expected results are explicit, not derived from the target.

The self-hosting verifier separately checks three successor generations for
identical WAT and Wasm, 12 program fixtures, compiler-source mutation, and 30
source rejections per successor. The new accepted parameter-shadowing and if
forms replace earlier restrictions; rejection fixtures now use actually invalid
parameter/syntax forms. The native byte-filter regressions still pass.

The foundation-based uppercase executable additionally passes empty input, all
byte values, embedded NUL, UTF-8, 255/256/257-byte boundaries, and more than 1 MiB
of input, plus help/usage statuses, in isolation. Verified platform: macOS ARM64,
linked only to libSystem. Linux execution is not established by these results.

## Native command continuation

The [native rbel command](native-command.md) now reads source with a reader written
in rbel and performs the example-check/build pipeline without Rust, Node, or
Python in its installed execution path. The command reproduces the compiler
and its own reader/compiler core WAT byte-for-byte. sym is now a compiled
primitive so the reader can construct interned symbols from character lists.
