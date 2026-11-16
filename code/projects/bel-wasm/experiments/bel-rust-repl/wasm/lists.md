# Recursive list functions in standalone Wasm

The Bel compiler can now compile first-order functions that traverse and
construct pairs. Multiple parameters, text literals and symbol names also
support [the compiled expression emitter](emitter.md). `copy-tree.bel`
recursively copies every pair in an input tree, leaving atoms unchanged. The
compiled module includes its allocator and primitives, has no imports, and runs
with ordinary WebAssembly APIs.

## Build and run

From the repository root:

```sh
direnv exec . cargo build --locked -p bel-rust-repl --bins
npm install --prefix target/bel-wasm-tools --no-save --ignore-scripts wabt@1.0.39
python3 experiments/bel-rust-repl/wasm/compile.py --lists \
  experiments/bel-rust-repl/wasm/copy-tree.bel target/bel-list-wasm/copy.wasm
target/bel-wasm-tools/node_modules/.bin/wasm-validate target/bel-list-wasm/copy.wasm
node experiments/bel-rust-repl/wasm/verify-lists.mjs target/bel-list-wasm/copy.wasm copy
python3 experiments/bel-rust-repl/wasm/verify-lists.py
python3 experiments/bel-rust-repl/wasm/verify.py
```

The last two commands exercise the complete new and original workflows. Source
variants, successful artifacts, and rejection inputs remain under
`target/bel-list-acceptance` and `target/bel-wasm-acceptance`. The new verifier
copies each module into a temporary directory containing only that module and
executes a standalone Node test script via stdin. It loads no Bel runtime or
project code when invoking Wasm.

## Supported source

```text
program := definition+
definition := (def name (parameter*) expression)
expression := nil | t | parameter | character | string | (quote constant)
            | (if expression expression expression)
            | (join expression expression) | (id expression expression)
            | (car expression) | (cdr expression) | (type expression)
            | (nom expression) | (err expression) | (name expression*)
constant := nil | t | symbol | character | string
```

The first definition is exported as `choose(i32, ...)->i32`, with its declared
arity. Zero-argument definitions are allowed; calls must match the declared
arity, and parameters must be distinct. All definitions are visible throughout
the program, including forward and recursive calls. Names and parameters are
symbols satisfying the published Bel `variable` predicate, containing only
lowercase ASCII letters, digits, and hyphens. Reserved names
`def fn if quote lit dyn after ccc where bquote mac set thread` and
`join car cdr id type nom err` are excluded. A parameter cannot have the same
name as any function in its program: indirect calls and function-valued
parameters are outside this subset. Duplicate definitions, unknown calls, wrong
arities, extra bodies, unsupported constants, numeric literals, malformed lists,
macros, closures, and other syntax fail compilation. Input is parsed as data and
never executed by the driver.

Function definitions cannot replace bindings present after loading the published
bootstrap, or the implicit `scope` binding. Those globals include macro
constructors and their transitive helpers; rebinding them can change later
native definition expansion. Bootstrap-macro/global mutation is outside this
first-order module contract. Parameters and quoted symbol data are not subject
to this namespace rule.

`bootstrap-boundary.bel` collects binding names in Bel before any compiler
libraries load. It copies the name spine, not the mutable environment cells.
Later compiler-private names remain available for module definitions. The only
two private helpers needed to create the snapshot are excluded from it by the
definition guard; neither exists in the unchanged published bootstrap. No Rust
lookup primitive, copied public-name catalog, or partial dependency list is
used.

Only nil is false. `id` compares atomic identity or pair identity; it does not
compare pair contents. `car` and `cdr` of nil return nil; applying them to other
atoms traps. `join` constructs a fresh pair. `type` returns the symbols `pair`,
`symbol`, or `char`; nil and t have type `symbol`. `nom` returns a fresh list of
characters naming an interned symbol, including nil and t. `err` traps.

Literal characters, strings and quoted symbol names support printable ASCII,
tab, LF and CR. Other literal characters are rejected clearly. Runtime data and
interned names may contain any Unicode scalar, including NUL. Empty strings are
nil. Each string literal occurrence is initialized once per module instance:
repeated execution reuses its pair identity; distinct occurrences stay distinct.

## Data interface

Each value is a 32-bit handle. Node treats handles as unsigned when decoding
characters or symbol payloads. This ABI is separate from the original boolean
compiler, whose values remain nil=0 and t=1 with no exported memory.

| Value              | Encoding                                                     |
| ------------------ | ------------------------------------------------------------ |
| nil                | `0`                                                          |
| t                  | `1`                                                          |
| pair type symbol   | `9`                                                          |
| symbol type symbol | `17`                                                         |
| char type symbol   | `25`                                                         |
| Other symbol       | `(interned_id << 3) \| 1`, with distinct IDs starting at 4   |
| Character          | `(Unicode scalar << 3) \| 2`                                 |
| Pair               | Aligned address at or above 4096 returned by exported `join` |

Create named symbols with the exported `intern(character-list)->i32`, use valid
Unicode scalars, and pass pair handles belonging to the current instance. The
module owns its symbol-name table. `intern` validates a proper list of up to
4096 Unicode scalars, copies new names, and returns the same handle for equal
names. Built-in nil/t/type-symbol handles remain fixed as above. Raw symbol IDs
from the earlier data ABI can still be carried through programs, but an
unregistered ID has no name and `nom` on it traps. Use `intern` when mixing
runtime symbols with source literals or name operations. This milestone does not
include a source reader inside Wasm.

Exports are `choose`, `join(i32,i32)->i32`, `intern(i32)->i32`, and `memory`.
`join` lets an ordinary runner supply pairs at runtime. Each pair occupies eight
bytes: little-endian 32-bit car at offset 0 and cdr at offset 4. Clients should
allocate through `join` and treat pair memory as read-only; reacquire buffer
views after memory growth. No JavaScript callback, host import, or external
allocator is needed.

The arena starts at 4096 in one 64 KiB memory page and grows to at most sixteen
pages. Allocation failure traps without invalidating previous allocations. There
is no collection or reset; discard the instance to reclaim its arena. Inputs and
results remain valid for the instance lifetime. Recursion uses the engine's
stack; sufficiently deep or cyclic input can exhaust it and trap. This is a
small pure-program experiment, not a general untrusted-input runtime.

## Ownership and remaining bootstrap

`list-compiler.bel` validates and lowers source. `list-runtime.bel` and
`text-runtime.bel` supply the Wasm support text emitted inside the module.
`list-text.bel` lowers ASCII text literals and initializes the reserved symbol
names. The generic Rust host runs those Bel definitions and writes their
returned character tree. Python only transports source and invokes WABT; it
contains no compilation decisions. The Rust evaluator and published Bel fixtures
are unchanged; the generic batch writer's larger bounds are described in
[the emitter milestone](emitter.md).

This is a step toward compiling the compiler itself. The compiler still runs on
Rust, and its own implementation still uses macros and higher-order operations
beyond this subset. The expression-emitter component runs in Wasm, as described
in [the emitter milestone](emitter.md). Self-hosting requires further work and a
compiler-generated successor verified without invoking Rust.
