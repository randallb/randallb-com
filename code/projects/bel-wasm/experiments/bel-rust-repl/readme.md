# Bel REPL experiment

A dependency-free Rust host for a small Bel bootstrap. Run from the repository
root:

```sh
direnv exec . cargo run --locked -p bel-rust-repl
```

At the prompt, load the executable definitions:

```text
:bootstrap
(no nil)
(atom '(a b))
(atom 'a)
(all atom '(a b c))
(some atom '((a) b c))
(reduce join '(a b c))
(cons 'a 'b 'c)
(append '(a b) '(c d))
(snoc '(a b) 'c)
(list 'a 'b 'c)
(map atom '((a) b (c)))
(map join '(a b) '(c d))
((fn (x) 'ignored x) 'macro-fn)
(do 'ignored 'macro-do)
(let x 'macro-let x)
(or nil 'macro-or)
(and 'a 'macro-and)
(= '(a (b)) '(a (b)))
([join _ 'tail] 'head)
(mem 'b '(a b c))
(with (x 'a y 'b) (list x y))
(rev '(a b c))
```

The results are `t`, `nil`, `t`, `t`, `(b c)`, `(a b . c)`, `(a b . c)`,
`(a b c d)`, `(a b c)`, `(a b c)`, `(nil t nil)`, and
`((a . c) (b . d))`, `macro-fn`, `macro-do`, `macro-let`, `macro-or`, and
`macro-and`, `t`, `(head . tail)`, `(b c)`, `(a b)`, and `(c b a)`. The
ordered files under `examples/bootstrap` concatenate
to the unchanged published Bel source from `no`
through templates and `readas`; that concatenation is the complete
1,801-line published source and the experiment's acceptance fixture.
`:bootstrap` loads those fragments in order. `:load` uses the rest of the line as its path, including
spaces, relative to the current working directory. `:cancel` discards an
incomplete expression; `:help` lists commands and `:quit` exits.

Supported Bel includes plain symbols, characters, strings, numeric literals,
proper and dotted pairs, quotation, backquote/unquote/splicing, bracket
functions, composition and complement shorthand, typed and intrasymbol
abbreviations, structural equality, optional/typed/destructuring parameters,
and lexical closures with fixed, rest-only, or dotted parameters. The published
macros, higher-order library, numeric tower, mutable-pair and place helpers,
self-hosted evaluator, list-backed reader/printer, arrays, tables, and templates
load unchanged and have focused executable coverage. Host places support
variables, `car`, `cdr`, and table entries. Dynamic bindings,
dynamic-extent escape continuations, guaranteed cleanup, virtual literal calls,
and mutable `xar`/`xdr` pairs are implemented by the host.

`apply` accepts a function, zero or more direct arguments, and one final proper
argument list. Only `nil` is false. Primitive omitted arguments become `nil`;
excess primitive arguments, missing required closure arguments, and excess
arguments to fixed-arity closures are errors. Pairs use identity for `id`.
Functions are inspectable Bel literal pairs, definitions survive later inputs,
and the published `bel` evaluator runs quotation, conditionals, primitives, and
closures on top of the Rust host.

This is not a complete Bel runtime despite loading the complete source.
OS-backed `open`/`close` and byte I/O, `coin`, concurrent `thread` scheduling,
persistent continuations, and arbitrary source-registered host places remain
unsupported. The host reader also omits shared-structure `#` syntax; that syntax
exists only in the loaded self-hosted reader definitions. Numeric operations
remain source-defined, so larger arithmetic loops can reach the evaluation
budget even when the same definitions work on small inputs.
Files are read as UTF-8, parsed completely, then evaluated in order. Evaluation
errors can leave earlier definitions installed. Input, nesting, evaluation, and
output limits report errors so the prompt remains usable. Each result is
rendered before printing, with limits of 16 KiB, 8192 rendering steps, and 128
nesting levels; shared pairs can exceed these limits even when evaluation is
small. An oversized result reports an error without printing a partial value.
Ctrl-D ends input; use `:cancel` for an incomplete form.

Run focused checks with:

```sh
direnv exec . cargo test --locked -p bel-rust-repl
direnv exec . cargo clippy --locked -p bel-rust-repl --all-targets -- -D warnings
```

Compare core evaluation and selected helpers against the published source:

```sh
direnv exec . cargo test --locked -p bel-rust-repl --lib comparison
```

The fixtures under `tests/comparison/` cover core calls, parameter binding,
dynamic scope, table lookup/mutation ordering, backquote, and handled errors.
Add `--release` to use the optimized test profile. The comparison tests also run
with the package's full test suite.

Each row is `(expression expected-result)`, where the expected result is data.
Those rows run directly on the Rust host and via `(bel 'expression)` on the
published evaluator. Core cases use literal closures: the current published
macro definitions cannot bootstrap themselves through `bel` (`fn` needs
backquote, which needs `let`, which expands through `fn`). This harness records
that limitation rather than accepting resource-limit errors as equivalence.

Table and backquote rows instead use the explicit format
`(expression (source reference-expression) expected-result)` or
`(expression (expand reference-expression) expected-result)`. `source` evaluates
the named published helper on the Rust host. `expand` evaluates the published
backquote macro's closure as an ordinary function, then executes its returned
expansion. These are comparisons against source helper definitions, not full
self-hosted evaluator tests. Table coverage includes lookup, existing-entry
locations, equality scope, and key mutation ordering; source `tabloc` insertion
still requires an unsupported host place.

Rows within a fixture share state; each fixture and route gets its own syntax,
interpreter, complete bootstrap, and identical host-evaluated setup. Later rows
observe mutation and recovery. Identity is checked inside Bel with `id` and in a
separate unchanged-template test, since printed pair contents cannot expose
sharing. Both routes must match explicit expectations as well as each other.
Failures identify the fixture, route, row, and expression. Unexpected evaluation
or rendering errors fail; error fixtures compare language-visible recovery, not
engine-specific diagnostic wording.

Only the reference receives a test-only evaluation-depth allowance of 4,096 on a
dedicated 64 MiB thread stack. Running an interpreter inside the host multiplies
recursion depth. The host under test and REPL retain normal limits. The routes
share the Rust reader, primitives, bootstrap, and fixture setup; this is not an
independent validation of those layers. The corpus is small and deterministic
and does not cover scheduling, persistent continuations, OS I/O, large
arithmetic, arbitrary mutation, or cyclic output graphs.

The project intent, research, results, and example session are maintained in
Randall's TeamWorkspace personal volume under
`Projects/bel-rust-repl`. Reference:
[Paul Graham's Bel introduction, guide, and source](https://paulgraham.com/bel.html).

A narrow compiler written in Bel now produces an import-free Wasm function.
See [portable function compiler](wasm/readme.md) for the supported subset,
bootstrap boundary, reproducible commands, and complete external regression.
The generic `bel-eval-text` batch executable loads the same bootstrap and prints
its final character tree, without REPL framing; `--call FUNCTION DATA FILE...`
passes all parsed DATA forms as one list to FUNCTION after evaluating FILEs.
