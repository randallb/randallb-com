# Numeric milestone verification and review

Owner/integration: Assistant. Commission: Randall's accepted exact-by-default
literal design, November 6, 2026. Scope: the numeric extension above PR #10;
this is separate from the earlier libcwr superbuild review ledger and does not
reset its allowances. No merge or release is performed by this milestone.

## Independent review

| Reviewer | Independent scope | Result |
| --- | --- | --- |
| numeric_reader | Exact arithmetic, compiler/float primitives, native/portable build integration, transport, CI and numeric acceptance | No remaining blocking findings; 150 independent exact-arithmetic cases passed |
| numeric_library | Reader and bootstrap/package wiring | No remaining blocking findings after fixes; authored exact library and verifier excluded from independence claim |
| numeric_compiler | Float conversion and printing | Clean after fixes; 124 independent Wasm cases, native boundary and print/read fixtures passed; authored compiler excluded from independence claim |

Review led to finite-bit predicate validation (reject NaN/infinity), reduced
conversion recursion, shared native core/product call limits, strict UTF-8 in
portable transport, atomic portable emit publication, and second-generation
seed dependencies. Full Rust bootstrap also exposed long compiler string
literals; balancing the new runtime text resolved that without changing its WAT.

## Reproducible checks

- `command/verify-numbers.py`: native exact/float oracle groups, failure cases,
  float printing/reading at boundaries, and standalone native executable output.
- The same verifier with `--portable`: compiled Wasm acceptance, explicitly
  omitting standalone native execution.
- `command/verify-command.py`: moved bundle, source/reader errors, native filters,
  failed-example cleanup, aliases, missing tools/recovery, deterministic core
  self-emission, and execution with interpreter tools unavailable.
- `wasm/verify-selfhost.py`: full Rust bootstrap, three identical successor
  generations, program fixtures, compiler mutation, invalid source and cleanup.
- `libcwr/verify.py`: all 31 native-mode groups; all 29 portable-mode groups,
  including actual workflow, intentional semantic failures and existing chapters.
- Native bundle rebuilt from the previous integer seed and from a freshly built
  numeric seed; both succeed. Compiler/runtime and sources ship in the bundle.

Generated reports are under `target/numeric-verification`,
`target/numeric-float-review`, `target/numeric-compiler-smoke`,
`target/rbel-command-verification`, `target/bel-selfhost`, and `target/libcwr`.
Hosted CI runs the portable suite on Linux and retains numeric and workflow
reports. Hosted status belongs to the current PR checks, not this source note.

## Known boundaries

Only finite binary64 values are supported; overflow/nonfinite results fail.
Float text uses an exact fraction with `f`, preserving bits, rather than a compact
shortest decimal. The new raw numeric representations replace the earlier unary
shape and require explicit migration of old persisted numeric trees. Resource
budgets still apply, as documented in [the numeric contract](numbers.md).
