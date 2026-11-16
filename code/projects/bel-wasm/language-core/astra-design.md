# rbel language core: implementation design

Owner: Randall Bennett. Design: Astra, November 6, 2026. Status: proposed implementation contract, not a claim of implemented or verified behavior. Base: numeric candidate `d38c27f`, plus the focused relative-import implementation `836f990`; the unrelated application/workspace prototype is outside this change.

This document specifies proper tail calls, collection of unreachable values, recoverable errors, `apply` and rest parameters, local binding and sequencing, module namespaces, and Unicode literals. It preserves source self-hosting, import-free generated Wasm, wasm2c native output, immutable pairs, and source-defined exact arithmetic. It adds no interpreter, host arithmetic implementation, OS API, resource finalizer, merge, or release.

## 1. Fixed runtime and compiler contracts

These decisions are ready for implementation. The following sections supply the frontend, compatibility, staging, and acceptance contract.

### 1.1 Value ABI and configuration

Retain the existing value representation: `nil=0`, `t=1`, the fixed type symbols `pair=9`, `symbol=17`, `char=25`, Unicode scalar characters `(scalar << 3) | 2`, other symbols with low bits `001`, and aligned eight-byte pair addresses. Pair fields stay at offsets zero and four. A pair address is valid only if it is in the configured pair arena, aligned, and its allocation metadata says allocated. The old `address >= 4096` heuristic is insufficient. Host readers must call the nonallocating `value_kind`/`pair_valid` exports before reading a cell. Invalid scalars and unregistered symbol IDs are rejected.

Export `rbel_abi_version() = 2` and `rbel_language_version() = 2`. Configuration happens before allocation and before literal initialization; replace the Wasm start function with explicit `runtime_init(pair_cells, root_slots, host_slots, symbol_slots, call_limit) -> status`. The host chooses positive bounded capacities; the compiler embeds documented defaults. A second initialization returns `already-initialized`. An initialization failure leaves an unusable instance that the host discards. There is no reset of a live instance.

The heap budget is explicitly a number of pair cells, eight bytes each. Report the larger total linear-memory commitment separately. The runtime checks all size arithmetic, maximum memory pages, minimum capacity needed by initialization, and `memory.grow` failure. The default remains a 64 MiB **total linear-memory maximum**, rather than silently turning the old 64 MiB arena into a larger allocation. No automatic growth can exceed the configured capacities. Small-heap tests select explicit values; production defaults are not used as evidence of collection.

Lay out linear memory deterministically, with eight-byte alignment between regions:

| Region | Size | Meaning |
| --- | ---: | --- |
| Runtime control and static data | generated, starting at 0 | Control words, UTF-8 literal data, source/function tables; never interpreted as pair cells |
| Compiler/runtime root stack | `4 * root_slots` | Only slots below the current top are live roots |
| Host root table | `16 * host_slots` | Value, generation, occupancy/free-list bookkeeping |
| Symbol descriptors | `16 * symbol_slots` | Symbol ID, name pair value, live/pinned state, bookkeeping |
| Invocation diagnostics | implementation-sized times `call_limit` | Numeric function/site IDs; value fields, if any, must be explicitly traced |
| Cell metadata | `pair_cells` bytes | FREE, WHITE, GRAY, BLACK |
| Mark work stack | `4 * pair_cells` | Pair cell addresses; sufficient because each allocated cell enters once |
| Pair arena | `8 * pair_cells` | Car/cdr layout unchanged; arena base is at least 4096 |

All regions belong to the configured maximum. The collector and the error path allocate no memory. Free cells can store the free-list link in their car word; that word is not a language value and is never traced. A newly allocated pair is fully initialized before becoming visible to any allocating operation.

### 1.2 Root discipline

Use a shadow root stack in linear memory. Wasm locals, Wasm operand-stack values, C locals, JS arrays, and JS closures are **not** traced implicitly. A function reserves its whole statically sized root frame, checks capacity, and clears new slots to nil before any allocating operation. It restores the saved top on every normal, tail-transfer, and error exit. If reservation fails, it changes no root-top state and raises the preinitialized `root-exhausted` error.

Change expression emission to a statement/temporary form. Every expression result is assigned to a known root slot before evaluating an allocating sibling. For `(join (make-left) (make-right))`, evaluate and root `make-left`, then evaluate and root `make-right`, then call join, then root the result. This rule applies equally to argument lists, constant initialization, captures, environments, closures, float packing, symbol-name copying and interning, and hand-emitted runtime helpers. Merely rooting language parameters is incorrect. No nested Wasm call expression may hide a live pair or symbol on the operand stack across an allocator call.

First implementation may retain expression temporaries until their enclosing compiled frame exits. It must clear discarded `do` intermediates, dead protected-expression temporaries before entering a catch handler, and transferred frames before continuing a tail call. Reuse of dead slots is an optimization. Root-frame sizes and slot assignments must be deterministic.

The collector traces these complete root classes:

* All occupied compiler/runtime shadow slots, including helper partial lists, current allocation operands, dispatcher function/argument slots, closure captures, and initialization intermediates.
* Occupied host-root slots and the exported last-result/error handoff slots.
* Generated literal globals, every named/primitive function closure, every code-identity pair, the closure marker, float-tag globals, and any new value-valued runtime globals. Generate one explicit global-root enumerator alongside the globals; do not keep an independently handwritten list.
* Pending tail-transfer function and argument values, and pending error payloads.
* Pinned symbol descriptors; other symbol names are traced only when their symbol is reached.

The collector starts all allocated cells WHITE, traverses roots, changes a cell to GRAY before pushing it, and iteratively scans its car/cdr before making it BLACK. Trace symbols encountered in roots or pair fields as described below. Sweep WHITE cells onto the free list, preserve BLACK cells, and reset live-cell metadata for the next collection. Root/graph cycles do not recurse on the native stack. Run collection when a pair allocation has no free cell, on symbol-table pressure, and through a diagnostic `collect()` export. If collection cannot satisfy the request, raise a recoverable bound error. No partially initialized new object enters the graph.

### 1.3 Interned symbols are weakly registered

Do not root the existing `$symbols` linked list strongly. Replace it with the bounded out-of-line descriptor table above. A descriptor records symbol identity and its character-list name; the table alone does not retain either. Marking a reachable symbol marks its descriptor and traces its name list. Nil, t, fixed type symbols, and built-in error symbols have pinned descriptors. All reachable symbols, including symbols in host roots, quoted values, literal globals, closure environments and error payloads, keep their names.

Interning a currently live equal spelling returns the same ID. After an unreferenced symbol is collected, its descriptor can be reused, but its numeric identity is never reused within the instance. The monotonically increasing 29-bit identity space has a checked exhaustion error; do not wrap. When the table is full, collect before reporting `symbol-exhausted`. Name comparison is by scalar sequence, with no Unicode normalization. An initial linear table search is acceptable for correctness; table indexing is an independent optimization.

During `intern`, root the incoming name, any copied name, and construction intermediates. Publish a new descriptor only after construction succeeds. An interrupted construction returns its reserved descriptor slot. `nom` keeps its existing fresh-list behavior; its source name and output partial lists must be rooted. A dead symbol's name is reclaimed even though a different, explicitly retained `nom` result can survive.

### 1.4 Host roots and export boundaries

Provide `root_retain(value) -> i64 token`, `root_release(token) -> i32 success`. The token is `(generation:u32 << 32) | (slot+1:u32)`; generation begins at one and wrap retires that slot rather than accepting stale tokens. Tokens belong to one module instance, which host wrappers enforce: raw slot/generation bits alone cannot detect a coincidentally identical token from another instance. Retain/release allocate no pairs and do not trigger collection. Retain validates the value; invalid values or a full host-root table return token zero with a separate management-error field. Release returns false for a stale, duplicate or invalid token; the host wrapper rejects foreign-instance tokens. Management operations do not erase the most recent language completion/diagnostic.

Each allocating public value export protects its incoming arguments before doing anything that can collect. It puts its returned value in a traced last-result slot before releasing internal roots. That slot stays live until the next allocating public operation; the host can immediately retain it without a collection window. Unhandled error payloads have an analogous last-error root. Old last-result retention ends at the next allocating operation, not at an arbitrary future collection. Reentrancy into the same instance while a public call is running is rejected; asynchronous hosts retain values between separate calls.

Host code must retain each value that survives a later allocating export, including array elements encoded before their siblings, list accumulator tails, source/examples/tag/request values, pending decoding/traversal stacks, and suspended test continuations across awaits. Root the whole graph before traversal. `symbolname` allocates and therefore requires retention of the rest of the graph. Prefer scoped host helpers with `try/finally` or one C cleanup path; do not auto-retain forever. Every adapter checks ABI version before interpreting values. Coordinated migration is required for WasmValues, self-wat, core-transport, native rbel, all native adapters, test-runner and reused-instance embedding paths.

### 1.5 Tail-call ABI

Use ordinary MVP Wasm calls, loops, globals and linear-memory slots. Do not depend on Wasm tail-call or exception extensions or on a C compiler recognizing tail calls.

Every compiled named function, lambda and primitive wrapper has a step entry:

```text
step(env: i32 value, args: i32 proper-list value) -> i32 status
status = DONE (0) | TAIL (1) | THROW (2)
DONE publishes a rooted output value.
TAIL publishes a rooted next-function closure and argument list.
THROW publishes a rooted error payload and fixed diagnostic fields.
```

All function-valued calls, including statically named tail calls, use the same dispatch identity/arity contract. Keep pair-backed closure representation and stable named/primitive identities; replace existing direct `$bel-*` recursive calls with the invocation contract. A non-tail call invokes the dispatcher loop, checks its completion, and copies the returned value into a caller root slot before further allocation.

`invoke(fn,args)` owns rooted current-function/current-argument slots and a numeric diagnostic frame. It dispatches one step. On TAIL, it first copies the pending transfer into those own slots, clears the transfer slots, and loops. The completed step frame is already released. On DONE or THROW, it restores its own saved root top and returns through the normal Wasm call path, with the output/error still rooted in the handoff globals. No tail transition recursively calls invoke or another step. Tail-call arity validation happens before dispatch, and failure follows the same cleanup path.

Nested non-tail invoke calls can still consume Wasm/C stack. Check `call_limit` before entering another non-tail invocation; exceeding it raises `call-depth-exhausted` before the native runtime limit. Cap the configurable value at a conservative supported maximum, derived from the generated invoke/dispatch/step overhead and verified on both supported adapters; do not accept arbitrary values that exceed the native stack cap. The core's allocation/GC helpers must themselves have bounded native call depth. This is not a substitute for tail-call lowering. Debug statistics report maximum nested invocations, maximum root slots and tail-transfer count. A million tail calls must hold nested invocation depth constant, including mutual calls and calls through a closure or apply.

Evaluation order is function expression first, then arguments left to right. Construct the argument list from already rooted values. A step's function body is in tail position. Selected `if` branches inherit tail position; condition expressions do not. The final `do`/let body expression inherits tail position; earlier expressions and binding initializers do not. `apply` inherits the enclosing call's tail position. The protected body of `try` is not in the enclosing function's tail position because a handler continuation remains; a loop called under one try still executes its own tail calls with constant stack.

### 1.6 Recoverable errors without traps

Reserve `unreachable`, memory traps and wasm2c traps for compiler/runtime invariant violations or hostile direct memory corruption. Language type errors, wrong arity, non-callable values, malformed apply lists, explicit err, invalid Unicode input, and declared resource limits follow the ordinary THROW/status path. Host tooling reports an unexpected trap as a runtime defect and discards that instance; it must not pretend the instance recovered.

Keep `(err value)` as a one-argument primitive: it raises that value. Built-in failures raise pinned descriptive symbols such as `type-error`, `arity-error`, `not-callable`, `invalid-argument-list`, `heap-exhausted`, `root-exhausted`, `symbol-exhausted`, and `call-depth-exhausted`. These errors need no heap allocation. Kind, source site and function identity are separate fixed diagnostic fields, not an allocated error object.

Add exactly this recovery syntax:

```lisp
(try expression (catch problem recovery-expression))
```

Evaluate the protected expression normally. On success return its value without evaluating recovery. On a language error, bind `problem` lexically to the raised value, clear the pending error, and evaluate recovery. The binding is visible only in recovery and follows the ordinary binding-name rules. Errors raised in recovery propagate outward. There is no retry, rollback, resource finalization, stack resumption or continuation capture.

Compile try to a local error target and inline recovery code, not a recursive interpreter or native longjmp. Every potentially failing emitted operation checks the error/status before using its result. Nested invoke calls release their own frames before propagating THROW. Before recovery, root the payload in the catch slot, clear dead protected-expression slots, restore the protected region's root/diagnostic state, then clear the error registers. The recovery expression inherits the try expression's tail position. The protected expression may make normal calls; one handler around a tail-recursive computation does not grow per iteration.

An uncaught error returns to the public wrapper with the instance structurally reusable. Export nonallocating accessors for completion status, error value, error kind, source-site ID and function ID. Mandatory `choose_args(args)` calls the entry with a proper argument list. `choose` may retain its existing positional signature and value return for fixed-arity entries, but hosts **must** check completion status before reading either result; failure returns nil only as a placeholder. Export generic `invoke_value(fn,args)` for retained closure calls and tests. New generic adapters use `choose_args`, which also supports a rest-parameter entry. Explicit collection/root-management errors have their own return status and never silently replace a caught language value.

Set the active diagnostic site before a potentially failing operation. The first raise captures it; propagation does not overwrite the origin. A bounded diagnostic stack contains actual non-tail invocation frames, not a fictitious million-entry tail-call history. Catching an error clears the pending failure but need not allocate or build a trace.

## 2. Source language

### Functions, rest parameters and apply

Keep existing fixed parameters and add dotted and all-rest parameter lists:

```lisp
(def first (a . rest) a)
(def gather arguments arguments)
(fn (a b . rest) rest)
(fn arguments arguments)
(apply gather '(a b c))
```

The parameter specification is either a proper list of distinct binding names, a dotted list ending in a distinct binding name, or one binding name. A fixed prefix requires that many arguments. A proper parameter list requires exactly that many. The rest binding receives the remaining argument-list tail; do not copy it. `(apply (fn rest rest) xs)` returns the identical `xs` pair when `xs` is nonempty. Ordinary source calls construct their argument list after evaluating the function and all argument expressions left to right.

`apply` is a first-class primitive with exactly two arguments: a function value and a finite proper list. It does not accept an additional spread prefix. `nil` is a valid empty argument list; a dotted tail raises `invalid-argument-list`. Use an iterative proper-list check, with cycle detection where supported host-built graphs can enter. Validate the whole list, even for an all-rest target. Arity failures are language errors. A statically known arity mismatch remains a compile-time error and is diagnosed even in an unchosen branch; a computed target is checked at runtime.

The primitive step for apply returns TAIL to the supplied function/argument list. It must not introduce a new invocation merely because apply itself was obtained through a variable. This is required for a loop that repeatedly calls a variable containing apply to remain a tail loop.

`def` and `fn` accept one or more body expressions. The body is an implicit `do`. Empty bodies remain compile errors. No optional/default/destructuring parameters, keyword arguments, local recursive definitions or mutable bindings are added.

### Local binding and sequencing

Use Bel-style single binding:

```lisp
(let suffix '(done)
  (let append-suffix (fn (xs) (join xs suffix))
    (append-suffix '(a b))))

(do expression-1 expression-2 final-expression)
```

`(let name initializer body...)` requires one binding name and at least one body expression. Evaluate the initializer in the outer environment, then evaluate the body with the name bound to that value. The initializer cannot refer to the new binding; an outer binding of the same spelling remains visible there. Nested lets express multiple bindings. Closures capture the lexical value and may outlive the let's compiled frame because their environments are traced.

`do` evaluates its expressions left to right, discards each intermediate result, and returns the last. `(do)` is nil. It stops at the first error. The compiler clears discarded temporary roots before subsequent allocating work, making a sequence of allocate-and-discard expressions collectable.

The syntax names `if`, `fn`, `def`, `quote`, `lit`, `let`, `do`, `try`, `catch`, `module`, `import`, `export`, and nil/t cannot be binding names. Primitive names, including apply and the character primitives below, can be shadowed by lexical parameters, lets and catch bindings. Preserve the existing prohibition on top-level definitions replacing primitives. Resolve lexical bindings before recognizing primitive calls. Continue rejecting unresolved names and malformed syntax throughout all bodies, including branches that cannot execute.

The compiler may lower multiple bodies to do. It should lower let to a rooted lexical slot rather than creating and invoking a temporary closure merely to implement binding. Let is not letrec, and there is no mutation or dynamic scope.

## 3. Modules and relative imports

The existing relative-import loader supplies file discovery, canonical identity, deduplication, cycle errors and entry preservation. Extend that model; do not replace the workspace, reload or conversation protocol.

A new module file opts in with a first declaration, and contains ordinary top-level definitions:

```lisp
(module greeting)
(import "./words.rbel.md" as words)
(export greet)

(def decorate (name) (join name '(welcome)))
(def greet (name) (words/finish (decorate name)))
```

The importing entry can be a module or an existing flat source file:

```lisp
(import "./greeting.rbel.md" as greeting)
(def main (name) (greeting/greet name))
```

The contract is deliberately small:

* A file has at most one `(module simple-name)` declaration and it is the first source form, ignoring comments and prose. That name is a diagnostic label, not a network/package identity. Canonical file identity remains the loader's deduplication key.
* Module headers contain zero or more imports and at most one `(export local-name...)`, before definitions. Missing or empty export means no public definitions. Exports name definitions owned by that file. Unknown names, duplicate names, imported aliases and attempted re-exports are compile errors.
* `(import "relative/path" as alias)` imports a declared module. Its alias is local to the importing file and unique there. Only `alias/exported-name` resolves outside the module. An alias is a namespace, not a value. Namespaces and lexical variable names are separate; binding `words` does not change `words/finish`.
* An unqualified name resolves to a lexical binding, an own-file definition, or a primitive. Other modules' private definitions never enter this lookup. Duplicate definitions are rejected within a module; two different modules can each define `helper`.
* A module file must use aliased imports. Bare `(import "relative/path")` remains the legacy flat import syntax only when both importer and target are legacy files. A legacy entry can mix bare legacy imports with aliased declared modules. Aliasing a legacy file, or importing a module without an alias, is rejected with a migration diagnostic. This avoids silently inventing an export policy for old libraries.
* The first definition of the root input remains the executable entry, regardless of export status. Imports do not replace it. Multiple legacy CLI source files remain one flat namespace, with existing duplicate checks. An executable has one declared root module; unrelated extra module files must be reached through explicit imports.
* All dependencies are statically read and linked. Imports run no top-level expressions. Each canonical module contributes its definitions, literals and named closures once per compiled instance. A diamond import therefore shares function identity. Existing import-cycle rejection remains; cross-module recursive import graphs are not added.

Qualified reference tokens contain one slash, with a simple alias on the left and a legal ordinary or arithmetic name on the right. `/` alone remains division. A token starting like a number is parsed by the numeric reader first, so `1/3` remains an exact number. Bindings and exported local names do not contain slashes. `'words/finish` is ordinary symbol data, and its spelling/identity is not qualified by evaluation.

Implement namespace resolution in the rbel frontend/linker. Resolve each global binding to a deterministic internal ordinal, then rewrite **references** to generated simple symbols such as `sc-global-37`. Rewrite every definition, including legacy definitions, so an authored name cannot collide with an unrewritten internal name. Preserve original names and origins in a side mapping. Traverse lexical scopes correctly for parameters, rest bindings, let (outer initializer, inner body), nested fn, and catch (binding only in recovery). Never rewrite quoted data, strings or numeric literals. The code generator receives plain linked def/fn/etc. forms with unambiguous names; it does not need module semantics.

The host reads bytes, supplies source labels/line maps, extracts existing literate fences, resolves relative paths and canonical file identities, and performs bounded dependency traversal. A rbel `scan-unit` operation parses the actual source and returns import declarations. The host follows those declarations and submits a source bundle plus canonical dependency edges to the rbel linker. Do not expand the C import scanner into a second reader, lexical resolver or namespace implementation. Native and portable transports carry the same source-bundle representation and receive the same frontend results. A filesystem-free host may provide the dependency bundle directly; linking does not require filesystem imports in generated Wasm.

Retain current path restrictions and source/resource bounds unless a separate change explicitly adjusts them. Import diagnostics identify the importing file and declaration origin as well as the missing/invalid target. Canonical paths are loader facts; they are not rewritten into symbol identities.

### Exact frontend data contract

`rbel-core` continues taking one request value. These are proper lists in the existing value/JSON transport; paths, labels, source text and decimal metadata are character lists, not symbols. Unit IDs are deterministic simple symbols such as `unit-0`. Positive line numbers are canonical decimal character lists. The explicitly unknown AST line is `"0"` and is displayed as unknown, never as a real source line.

```text
unit       = (unit-id display-label ((start-line source-text)...))
edge       = (importer-unit-id authored-path target-unit-id)
example    = (owner-unit-id form-line name-symbol expression-source expected-source)
bundle     = (source-bundle-v1 (root-unit-id...) (unit...) (edge...) (example...))

request    = (scan-unit unit)
result     = ((authored-path alias-or-nil import-form-start-line)...)

request    = (read-unit unit)
result     = ((form (display-label form-start-line original-function-name-or-nil))...)

request    = (compile-bundle bundle) | (check-bundle bundle)
result     = WAT character tree
```

Every source block contains complete top-level forms; a plain source file is one block beginning at line one. Preserve block/source order. The first root ID selects the entry; additional root IDs are allowed only for the existing multi-file legacy mode. The host calls scan-unit, follows returned imports, and records one edge per distinct importer/authored-path pair. Two path spellings may resolve to the same canonical target ID. The rbel linker checks that every parsed import has exactly one matching edge, that no extra/contradictory edges exist, and that all target units are present. The host cannot alter visibility by supplying an invented edge. A source file is snapshotted once for the bundle, so discovery and final compilation see identical bytes.

For checks, parse each expression-source and expected-source as exactly one form. Resolve the expression in its owning unit, with access to that unit's private definitions and aliases; treat the expected value as data and never alpha-rename it. Preserve the existing globally unique example selector names and imported-example coverage. Compiling a bundle ignores its example rows; checking a bundle includes them. Metadata/shape errors follow the regular diagnostic error path.

The frontend calls `sc-compile-located(forms, origins)` after linking, with one origin `(display-label line-charlist original-function-name)` for each linked definition. Keep `sc-compile(forms)` as the source-only wrapper supplying unknown locations. Lambdas inherit the enclosing definition origin plus their expression path. Existing `(read source)`, `(compile source)` and `(check source examples)` requests may remain compatibility wrappers for flat in-memory source, using the explicit `<source>` label; new filesystem/module adapters use the bundle contract.

## 4. Source diagnostics that the implementation can substantiate

The minimum delivered location is the original file label, the starting line of the containing top-level form, and the original function name. A compiler expression-path ID can distinguish operations within that form. Do not print an exact offending-token column or underline a range unless the reader actually recorded it.

Preserve source as located units instead of concatenating it into an unlabelled string. Each plain source file or literate definition block has a source ID, display label, original starting line and character list. After skipping whitespace, the rbel reader records the starting input tail for each top-level form. It can calculate line advancement by walking the consumed prefix until the returned input-tail identity; count LF characters, so CRLF counts once. Store each successfully read form with its original start line. A parse failure in that form is reported as “while reading the form beginning at file:line”; this is an honest location even before full token spans exist.

Linker and compiler validation carry that origin through renaming, multi-body/let lowering and lambda creation. Syntax/unknown-name/duplicate/private-export errors name the operation or binding and its containing form. Generated function/site descriptors contain original names, source ID, form-start line and a stable expression path. Runtime steps update the active site before potentially failing operations; error propagation preserves the first failing site. Tail calls replace the current diagnostic function rather than appending history. The runtime can report bounded non-tail callers, with an explicit truncation indication if applicable.

The source table is non-value static data with nonallocating length/address accessors. It is included in total memory accounting. Paths are encoded as UTF-8, which WAT string data can represent directly under the [WebAssembly text-string specification](https://webassembly.github.io/spec/core/text/values.html#strings); escape quote, backslash and control characters correctly. Generated code uses deterministic source labels supplied by the compilation request, not temporary absolute build paths. Canonical absolute file paths, tool hashes and the full source mapping may additionally live in the build provenance file.

The source-only self-host API can keep `sc-compile(forms)` as a wrapper using the explicit label `<ast>` with unknown line. A new located/bundle entry supplies real origins. The wrapper must not invent line numbers for arbitrary transported AST data. A runtime error in source-aware compilation should render, for example, `math.rbel.md:42: divide: divide-by-zero (form starts here)`, followed by a readable raised value when useful.

Host formatting first retains the error value and snapshots numeric diagnostics, then may call allocating `symbolname`. Diagnostic formatting failures must not replace the original error classification. After a caught error or failed compilation, the next ordinary request on the same valid instance must succeed with the expected root/frame baseline.

## 5. Unicode and the text/byte boundary

A character is a Unicode scalar value from U+0000 through U+10FFFF excluding U+D800–U+DFFF. A string remains a proper list of characters. Unicode scalar sequence equality is exact: no case folding or normalization is implicit. A combining sequence and a precomposed character remain different sequences. Identifier expansion to arbitrary Unicode is not required by this milestone; quoted/runtime symbol names can already contain scalar text, and local binding names retain the documented ASCII grammar plus qualified references.

Source files and source transport use strict UTF-8. Reject truncated sequences, bad continuation bytes, overlong encodings, surrogate encodings and values above U+10FFFF. JS adapters reject lone UTF-16 surrogates instead of silently replacing them. Source controls retain existing lexical restrictions; arbitrary scalar values can be expressed through character construction or the character escape below. Data transport and printers are length-aware and preserve embedded NUL values.

Source character literals accept an actual non-ASCII scalar, such as `\é` or `\🦉`, as well as the existing `\sp`, `\tab`, `\lf`, `\cr` and punctuation forms. Add explicit scalar character notation `\u{HEX}` outside strings, with one through six hexadecimal digits and scalar validation. It expresses controls such as `\u{0}` and has no previously accepted named-character meaning. Literal strings accept actual UTF-8 scalar text. Preserve the existing string escape contract: backslash quotes the next scalar, so backslash-n is `n`, not a newline; adding this milestone must not quietly turn all existing strings into JSON-style strings. In particular `\u{...}` inside a string keeps that quoting rule. A future string escape revision would need its own compatibility decision.

Add two small target primitives:

```lisp
(char-code \é)       ; exact integer 233
(code-char 233)       ; character é
(code-char 128521)    ; character 😉
```

`char-code` accepts a valid character and returns the existing canonical exact nonnegative integer representation `(lit num + decimal-digits "1")`. `code-char` accepts a canonical exact integer in scalar range; negative, fractional, malformed, surrogate, oversized or float arguments raise errors. These are target Wasm scalar conversions, not host arithmetic. Existing exact arithmetic and decimal normalization remain source-defined. Runtime packing of the small exact integer follows the same roots and allocation-error checks as every other value constructor.

Replace the ASCII enumeration in `sc-char`. The final compiler calls char-code, obtains its decimal numerator character list, and emits a scalar constant followed by tag operations:

```wat
(i32.or
  (i32.shl (i32.const 233) (i32.const 3))
  (i32.const 2))
```

The compiler needs no host callback or source arithmetic to do this. The bootstrap bridge below introduces the primitive before the compiler itself relies on it. All scalar literals, including those nested in quoted data, strings and symbol names, go through the same lowering.

Retain the existing byte-filter ABI as explicitly **bytes**: each list element is a character-tagged value whose scalar is 0–255, and an output scalar outside that range is an error. The range is a compatibility representation for bytes, not an assertion that byte 233 is the UTF-8 encoding of é. The numeric/character tags are unchanged.

Provide small source library functions `utf8-encode(text)` and `utf8-decode(bytes)` using char-code/code-char and the existing exact arithmetic, with the strict validation above. Encoding é produces two byte values 195 and 169; decoding those bytes produces one character with code 233. Validate a finite proper list and each element's type/range. This makes crossing the boundary explicit while avoiding a new bytevector object type. Host source-file decoding and JSON/string transport obey the same vectors. A raw byte filter retains byte chunks; no streaming Unicode decoder or new text I/O API is implied, and decoding an arbitrary partial chunk as a complete UTF-8 string is not valid.

## 6. Bootstrap and version migration

Keep the compiler and hand-emitted runtime generators in the bootstrap-safe first-order source subset until the final runtime can compile them. The compiler implementation need not use new let/rest/try syntax to emit those forms. New runtime code remains WAT text emitted by rbel source. It is acceptable to split that source into `self-compiler.bel` and `core-runtime.bel`, provided build scripts include and hash both and the self-host input is exactly their documented composition.

The char-code dependency requires a real bridge, not a pretend direct rebuild:

1. Preserve the existing numeric ABI-1 seed and transport. Record its source, binary and tool hashes. Do not overwrite it while bringing up ABI 2.
2. Build **compiler A source**, entirely valid in the old first-order language. A emits the new runtime, roots, step ABI and recognized primitives, including char-code/code-char, but A's own `sc-char` still uses the old ASCII table. Its own source and any source it must lower at this step contain only the old representable characters. This is a committed, reproducible bridge source variant or deterministic documented transformation, not a manually edited binary.
3. Use the old native seed to compile A. For a portable source bootstrap, first evaluate the pinned legacy compiler source from `d38c27f` once in Rust to produce an ABI-1 seed, then use that seed and its legacy transport to compile A. The resulting **A executable is an ABI-1 compiler program** because the old compiler compiled it. Run it with the legacy ABI-1 transport and initialization convention. Its generated products use ABI 2 because A's source emits ABI 2. One reproducible construction for A is the final compiler/runtime AST with exactly its sc-char definition replaced by the pinned legacy sc-char definition; reject zero or multiple replacements, and record both AST/source hashes. This relies on all other compiler/runtime generator source remaining in the old core.
4. Feed **final compiler B source** to A. B is still first-order and contains the new sc-char implementation that calls char-code. A recognizes that target primitive without needing to execute it while compiling. Keep B's authored source ASCII at this bridge step (Unicode test programs follow later). A emits an ABI-2 B executable.
5. Run B using the ABI-2 transport, explicit runtime_init, root retention and completion checks. Have B compile B again. This produces the final stage0 for the new generation. Then compile three successors from the same canonical B source/AST and require byte-identical WAT and Wasm. Bridge A's code is not claimed to match B's output.
6. Build the new reader/frontend/runtime composition through B. The new C shell links only an ABI-2 core and initializes it explicitly. Never link a new ABI-2 shell to the old-seed-produced ABI-1 A program. Test the installed native command reproducing its compiler and complete core from the shipped source, and test a native-seeded rebuild with Rust/Node/Python unavailable on its execution PATH.

Use separate output directories and a manifest naming A, B, every transport version, source composition, input labels, runtime configuration, compiler/tool versions and hashes. ABI-1 modules continue to work with ABI-1 adapters; ABI-2 adapters reject them rather than guessing from the presence of memory. No automatic wrapping of unrooted legacy host handles is safe. Generated artifacts, native command bundles and package manifests advertise both ABI and language versions. No byte identity is promised across the version transition; equality is required among successors within B's version.

## 7. External resource lifetime

GC owns only rbel memory. It does not close file descriptors, cancel work, revoke subscriptions, dispose native instances, or send protocol messages. Reclaiming an unreferenced language value cannot produce externally visible cleanup effects.

An existing or future host resource is represented by an opaque host token with an instance/owner and generation. Its host registry owns the resource. Existing close/cancel protocol actions remain explicit, idempotent operations in that registry, and they invalidate subsequent use according to the protocol's normal error rules. A language value may describe the token but does not own a hidden finalizer. The embedding owner closes outstanding resources when disposing that embedding context. A future `with-resource`/unwind-protect abstraction may compose explicit acquisition and cleanup; this milestone does not claim that try/catch alone guarantees cleanup or add a speculative OS primitive suite.

## 8. Implementation order and ownership boundaries

1. Establish ABI-2 runtime scaffolding, configuration, management errors and diagnostics, preserving pair layout. Introduce source/version manifests and temporary ABI-aware host helpers. Keep GC disabled only while instrumentation/root audits are incomplete; no GC acceptance can pass at this stage.
2. Implement statement/temporary compiler lowering, generated global-root enumeration, root-guarded runtime helpers, generic step entries, arity metadata, dispatcher loop and ordinary status propagation. Bring up the A/B bridge with a small ASCII program before the complete compiler.
3. Implement collector and weak symbols; enable collection at actual allocation pressure and stress safe points. Adapt every maintained host path. Verify repeated compilations and errors on the same instance, not only fresh instances.
4. Add rest/apply, do/let/multiple bodies and try/catch with semantic and adversarial tests. Confirm tail position throughout these forms. This can proceed within the compiler owner once the runtime call contract is fixed.
5. Add source-unit origins, rbel module scanning/linking and the shared source bundle. Host ownership is file I/O, canonical identity and fence extraction; frontend ownership is parsing, namespace resolution, validation and renaming.
6. Complete character primitives, final Unicode sc-char, scalar reader escape and explicit UTF-8 library/boundaries; run the new bridge and full native/portable rebuild.
7. Perform independent implementation review on the complete candidate, resolve concrete findings, then run the existing required regression gates and the acceptance matrix below. Preserve unrelated draft PR chains. Implementation, review, merge and release are separate statuses.

Compiler/runtime work owns the invariants and all generated code. Host adapters own retention scopes and version checks. Module/source work owns one rbel semantic frontend shared by native and portable transports. The integrator must audit interfaces across these assignments; passing each component's tests independently is insufficient for GC or bootstrap correctness.

## 9. Acceptance criteria that distinguish real implementations

All runtime claims require both the portable Wasm path and wasm2c native macOS ARM64 path. Run native tail tests with sibling-call optimization disabled, as in the existing native build, and with the ordinary runtime stack cap. Report platform limits honestly; native Linux is a separate claim requiring its own execution evidence. Generated modules have no imports and use no tail-call/exception proposal opcodes.

Provide nonallocating diagnostic statistics for configured/allocated/live/free/reclaimed pair cells, collection count, live symbol descriptors, current/maximum shadow-root usage, occupied host roots, current/maximum invocation depth, tail-transfer count and total memory commitment. A test can snapshot counters without perturbing collection. Counters use checked/saturating integers; they cannot wrap into misleading acceptance results. A diagnostic stress switch may request collection before every permitted allocation safe point; it must not alter language behavior.

| Area | Required acceptance |
| --- | --- |
| Tail calls | One million direct, mutually recursive, closure-selected and apply-mediated tail steps. Maximum invocation/root depth stays constant with iteration count. Include a function value stored in a rest list and a variable bound to apply. Compare native `-O0` or disabled sibling optimization where practical. |
| Non-tail calls | A deliberately non-tail recursive program reaches the configured recoverable bound before wasm2c/engine stack failure. After catching it or returning to the host, a simple call succeeds and current root/frame counters return to baseline. |
| Collection under pressure | A small fixed heap repeatedly allocates/drops far more cells than capacity. At least several real collections and reclaimed cells are observed. The program succeeds without increasing capacity, recreating the instance, disabling allocation, or dropping live roots. A retained live set genuinely larger than capacity reports heap-exhausted. |
| Intermediate roots | Force collection while evaluating a later argument, while building a closure environment, nested constant joins, `nom` copy, intern registration, float bit packing and the exact char-code result. Previously computed pairs/symbols retain contents and identity. Include an initializer before its global is assigned. |
| Global roots | Collect between repeated uses of each literal, named/primitive function and code identity; identity stays stable for the same occurrence, and distinct pair-literal occurrences remain distinct. Escaping closure captures survive after originating frame release. |
| Tail transfer roots | Force collection during argument construction and immediately before transfer consumption. Function, captured environment, arguments and rest tails survive; old step frames become reclaimable before the next iteration. |
| Deep/cyclic graph traversal | Construct a pair chain much deeper than the native call cap; collection completes iteratively. A test-only host fixture may construct a cycle in otherwise valid allocated cells and verify reachability/sweeping without recursive overflow. This fixture does not add language mutation. |
| Host retention | Encode nested arrays/pairs with allocating siblings; keep native source/examples/tag/request values; decode a symbol-containing result while allocating names; preserve a test continuation across an await and allocating reply encoding; invoke an explicitly retained closure after many collections. Release and observe reclamation. Stale/double/foreign-wrapper release and host-root exhaustion are rejected. |
| Weak symbols | A retained dynamic symbol preserves identity and nom across collections. Drop thousands of distinct symbols under a small table; descriptor/name storage is reclaimed and further interning succeeds. A symbol reachable only through a quoted pair, capture, error or host root survives. IDs never alias a released identity. |
| Errors | Catch explicit arbitrary payloads, type failures, dynamic arity, bad apply tail, non-callable values and selected resource errors. A catch binding survives allocations in recovery. Recovery errors escape to an outer catch. Unchosen recovery does not execute. No root/frame leaks accumulate across thousands of errors. Heap exhaustion is catchable without allocating an error object. |
| Arity and rest | Zero arguments, required prefix, too few/many, dotted parameter specs, all-rest, duplicate names, wrong parameter tail, fixed primitive arity, rest-tail identity, indirect calls and primitive shadowing. Invalid shapes in dead branches still fail compilation. |
| Let and do | Initializer observes outer shadowed value; body observes new binding; escaped nested closures capture correct values; last expression is a tail call; zero do is nil; earlier errors stop later expressions. Discarded intermediate allocations become collectable. |
| Modules | Two modules with same private helper, inaccessible private binding, unknown/duplicate export, alias collision, function identity through a diamond import, cycles, exact entry preservation, legacy flat imports, mixed legacy entry/module dependencies, quoted qualified symbols untouched, lexical shadowing during rename, qualified arithmetic name versus numeric fraction. Native/portable linking outputs agree. |
| Source diagnostics | Syntax error, unknown binding, private export, wrong arity and runtime err in an imported literate definition all name the original file, correct containing-form line and original function when known. No flattened-file line or internal alpha-renamed name leaks into the user diagnostic. Plain AST mode says location unknown. |
| Unicode | Literal/source/transport/nom round trips for ASCII, é, CJK, a supplementary-plane emoji, combining sequences, U+0000 through scalar escape, U+D7FF, U+E000 and U+10FFFF. Reject surrogate scalars, U+110000, invalid UTF-8 and lone JS surrogates. Preserve existing string backslash behavior. Test literals inside quote and nested closures, not only host-injected runtime characters. |
| Bytes versus text | All 256 byte values round-trip through the existing byte filter; UTF-8 encode/decode yields exact expected bytes for multi-byte scalars and embedded NUL; malformed/partial sequences reject. A non-byte output scalar is an error. No test accidentally passes by Latin-1 reinterpretation. |
| Self-hosting | Recorded A/B bridge, final stage0 plus three byte-identical successors, mutation of compiler source changes emitted behavior, installed native rebuild reproduces compiler/core, import-free products, ABI mismatch rejection and relocated execution without development interpreters. |

The important invariant is observable bounded behavior: proper tail calls release frames, collection reclaims unreachable objects while preserving all live values, errors restore a usable execution boundary, and the same source semantics travel through the native and portable paths. Until the tests and independent review establish those properties, the work remains an implementation candidate.
