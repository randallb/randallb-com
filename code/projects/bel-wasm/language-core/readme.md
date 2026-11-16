# rbel language core, ABI2

Owner: Randall Bennett. Implementation and integration: Assistant, November 7,
2026. Design: Astra, requested at maximum reasoning effort. This is a draft
implementation above the numeric milestone, not a release. The preserved
[Astra design](astra-design.md) describes the agreed contract; this page records
implemented behavior, build paths, and deliberate refinements.

## Language behavior

- Proper tail calls work for direct, mutual, computed, closure and `apply` calls.
  A portable trampoline keeps tail recursion independent of the physical Wasm/C
  stack. Ordinary non-tail recursion has a configurable, recoverable bound.
- Unreachable pairs and closure environments are reclaimed by an iterative,
  nonmoving mark-and-sweep collector. Interned symbols have weak descriptors;
  reachable symbol identities remain stable. Pair cycles do not recurse in the
  collector. Configured heap, roots, symbols and invocation limits are explicit.
- `(try expression (catch name recovery))` catches a language error payload.
  Type, dynamic arity and resource errors use this path. Invalid syntax and
  statically known invalid calls are compilation errors, including dead branches.
  Diagnostics preserve the original file, containing-form line and function.
- `(fn args body...)` accepts fixed, dotted or all-rest parameters. Rest lists
  share their original tail. `(apply function argument-list)` validates and uses
  a proper argument list; primitive functions can be passed as values.
- `(let name initializer body...)` binds after evaluating its initializer in the
  enclosing scope. `(do expression...)` sequences expressions, returning the last
  result or nil. Functions and definitions accept multiple body expressions.
- `(module name)`, `(export name...)` and `(import "relative/path" as alias)`
  provide private definitions and qualified `alias/name` references. Bare imports
  retain the legacy combined namespace only between legacy units. Binding and
  global names are renamed hygienically; root entry order is preserved.
- Characters are Unicode scalar values. Literals accept actual UTF-8 and
  `\u{HEX}` scalar escapes outside strings. Existing string backslash behavior
  remains unchanged. `char-code` and `code-char` convert through exact integers;
  [utf8](../literate/utf8.rbel.md) provides explicit strict byte-list conversion.

The [executable language chapter](../literate/language-core.rbel.md) and
[module example](../literate/module-example.rbel.md) show the syntax. Numeric
syntax from the preceding milestone is retained: integers, fractions, decimals
and exponents are exact; the `f` suffix requests a finite binary64 value.

Files, processes, subscriptions and host connections still require explicit
close/cancel behavior in their owning APIs. GC has no finalizers and introduces
no new OS operations.

## Host contract and migration

Generated modules export `rbel_abi_version() = 2` and
`rbel_language_version() = 2`. Hosts must initialize before calling language code:
`runtime_init(pair_cells, root_slots, host_slots, symbol_slots, call_limit)`.
There is no start function and no live-instance reset. Generated Wasm has no
imports and requires neither Wasm tail-call nor exception-handling extensions.

Handles remain tagged 32-bit values with eight-byte pair cells. A pair must be
validated with `pair_valid`/`value_kind`; its numeric address alone is insufficient.
Host locals are not GC roots. Retain a value with `root_retain`, release the
returned generation-bearing token with `root_release`, and preserve values across
any allocating call. Language execution, error handoff and compiler temporaries
have explicit traced roots. Root-management status is separate from the pending
language error. An invariant trap invalidates the instance.

The maintained [JavaScript adapter](../literate/wasm-values.mjs) wraps roots in
frozen owner-bound objects. The [native adapter](../experiments/bel-rust-repl/wasm/command/value-roots.h)
adds instance epochs to reject disposed-instance handles even when native memory
addresses and raw runtime tokens are reused. Raw integers are not durable IDs.
Host adapters expose diagnostic text without allocating while reporting an error.

Default user instances reserve two million pair cells; compiler transports use
four million. Total configured linear memory, including tracing metadata and
roots, remains below the 64 MiB maximum. The native call limit is capped at 512
so the explicit error path precedes WABT's physical stack bound. Small-heap
acceptance tests exercise actual collection rather than relying on these defaults.

ABI1 artifacts and adapters must be rebuilt together. Existing persisted numeric
representations already require the migration described by the numeric milestone;
ABI2 does not make old raw pair addresses portable. No existing workspace or
persisted application data is migrated by this change.

## Source compilation and bootstrap

The reader, exact arithmetic, module resolver and compiler run as rbel. Python,
C and JavaScript transports snapshot bytes, discover files using rbel scan
results, transport source bundles, assemble output and perform filesystem work.
They do not implement expression evaluation, namespace visibility or arithmetic.
Source bundles retain authored filenames and containing-form start lines. Unknown
AST-only origins use `<ast>` and line zero instead of fabricated locations.

The ABI transition is explicit in
[bootstrap-language.py](../experiments/bel-rust-repl/wasm/bootstrap-language.py):

1. A pinned ABI1 compiler and reader bootstrap the updated source with an ASCII
   character helper. Only that helper is substituted in the transitional compiler.
2. That compiler emits the complete ABI2 compiler and runtime.
3. The ABI2 compiler reproduces its source through three successor generations.
4. [core_build.py](../literate/core_build.py) builds the source-aware command core
   and verifies that the shipped core recompiles its own `core.rbel` identically.

The native command also rebuilds its canonical core using an installed ABI2
`--seed` command, without Rust, Node or Python in the installed execution path.
The source-bootstrap Wasm command is a separate portable CI transport.

Example names are local to their source unit. Generated selectors are unique
ordinals, so imported modules may reuse an example label. This intentionally
relaxes the earlier cross-file label restriction. Source discovery and output
alias validation finish before an existing output is removed. Subsequent semantic,
example or compilation failures leave no stale output. Both transports reject
malformed UTF-8 throughout the original file, including literate prose.

## Verification

Run from `projects/bel-wasm`, after the existing native/WABT bootstrap dependencies
are available:

```sh
python3 experiments/bel-rust-repl/wasm/verify-selfhost.py
python3 literate/bootstrap-wasm.py
python3 experiments/bel-rust-repl/wasm/verify-runtime-core.py
python3 experiments/bel-rust-repl/wasm/verify-language-compiler.py --native
python3 experiments/bel-rust-repl/wasm/command/build-command.py
python3 experiments/bel-rust-repl/wasm/command/verify-command.py
python3 experiments/bel-rust-repl/wasm/command/verify-modules.py
python3 experiments/bel-rust-repl/wasm/command/verify-numbers.py
python3 libcwr/verify.py
```

`--native` is macOS ARM64 acceptance. Linux CI runs the same runtime, compiler,
modules, Unicode, numeric, libcwr and embedding checks through Wasm. Reports and
bootstrap hashes live under ignored `target`; the [review record](review.md)
distinguishes component review, integrated review, local tests and hosted checks.
