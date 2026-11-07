# The native rbel compiler command

`rbel` is now a native macOS ARM64 command. It reads rbel source with the
[reader written in literate rbel](../../../literate/reader.rbel.md), runs the
existing Bel-written compiler, checks literate examples as compiled native code,
and builds a Unix stdin/stdout executable. Rust, Node, Python, and a separately
installed Wasm engine are not used by the installed command.

Apple's C compiler and command-line tools remain required to build executables.
Native WABT 1.0.39 tools and their C runtime sources are included in the bundle.
Keep `bin`, `libexec`, and `share` together; the command locates them relative to
its own executable and can be moved to another directory.

## Use the bundle

After unpacking `rbel-macos-arm64.tar.gz`, from its parent directory:

```sh
./rbel-macos-arm64/bin/rbel build \
  rbel-macos-arm64/examples/unix-filter.rbel.md -o hello
printf 'hello, rbel!\n' | ./hello
```

Expected output is `HELLO, RBEL!` followed by a newline. This builds the executable
from the explanation's definition blocks, checking its seven examples first.
The new executable needs only macOS system libraries; it does not need the bundle.

To build the filter that uses the foundation:

```sh
./rbel-macos-arm64/bin/rbel build \
  rbel-macos-arm64/examples/foundation-filter.rbel.md \
  --include rbel-macos-arm64/examples/foundation.rbel.md \
  --include rbel-macos-arm64/examples/unix-filter.rbel.md -o foundation-uppercase
```

The command checks all 77 examples across those files. Definitions are combined
in input order; the first definition in the main file is the entry function.
It must accept one proper list of byte characters and return a proper list of
byte characters. The native adapter handles independent chunks of up to 256
bytes, standard input/output, and exit statuses, as documented in the
[native filter guide](native/readme.md). This command does not introduce a more
general OS I/O interface to the language.

## Commands and source format

```text
rbel build SOURCE [--include SOURCE ...] -o OUTPUT
rbel check SOURCE [--include SOURCE ...]
rbel emit SOURCE [--include SOURCE ...] -o OUTPUT.wat
rbel read SOURCE
rbel lint SOURCE [--file-limit N] [--block-limit N]
```

`--help` and `--version` return 0. Usage errors return 2; source, example, tool,
or resource failures return 1. `--quiet` suppresses successful build/check messages.

`.md` inputs use the existing `rbel definition=NAME` and `rbel example=NAME`
fences. Literate inputs require both kinds, except a definition module containing imports may omit examples. Names must be unique across
all loaded inputs. Example bodies contain one expression, an entire line `=>`, and one
expected value. Backtick and tilde fences, up to three leading spaces, CRLF,
and explanatory fences follow the extractor's supported rules. Unknown rbel
attributes, duplicate names, unclosed fences, and malformed examples fail.
Raw `.rbel`/`.bel` inputs contain definitions directly; a raw build may have no
examples, but `check` requires examples. There is no implicit foundation import.

`check` compiles a separate example module and runs each named check in a fresh
native process. Expected values are read as data, never evaluated. The structural
comparison helper is generated in Bel. Results and traps identify the example,
source file, and opening-fence line. The prefix `rbel-internal-` is reserved for
these generated checks. There is no expected-error fence notation yet.

`emit` checks any examples before writing Wasm text. It is useful for compiler
bootstrapping. `read` reports parsed definitions as a JSON value tree, using
`{"pair":[head,tail]}` for pairs, `{"char":"x"}` for characters, strings for
symbols, and null for nil. It does not evaluate source. Rendering completes in
memory before stdout is written; a bounded-render failure prints no partial JSON.

## Relative source imports

A raw `.rbel`/`.bel` file or a literate definition block can load another source:

```lisp
(import "foundation.rbel.md")
(import "../shared/helpers.rbel")
(def main (xs) (helper xs))
```

Paths resolve relative to the containing file's canonical location, including
nested imports. They must be nonempty relative paths: absolute paths, home
expansion, and refs/URI schemes are rejected. `.` and `..` work normally;
imports are dependency loading, not a directory access boundary. Strings follow
the reader's escape rule: a backslash quotes the next character literally.
Whitespace and semicolon comments are allowed within the import form.

Only unquoted top-level `(import "path")` forms in definition source are
directives. Quoted data, function bodies, examples, prose, and other-language
fences do not load files. `import` is reserved in this top-level position;
there are no namespaces, aliases, exports, package lookup, or CWP resolution.
The installed command handles imports for `build`, `check`, `emit`, and `read`;
the older development-script/interpreter routes do not gain import support.
`lint` continues to inspect only its explicitly supplied file.

Each file contributes its own definitions first, then its dependencies in
first-encounter depth-first order. The main file's first definition remains the
entry function even when import forms precede it. A wrapper containing only
imports uses its first dependency's entry. Canonical path and filesystem identity
prevent duplicate loading, including repeated `--include` inputs, symlinks, and
hard links. Cycles and missing dependencies fail. All imported examples are
checked once; existing global block-name uniqueness still applies. The 32-file
bound includes the entire dependency graph; each file has at most 32 import forms.
Combined definition/example byte bounds remain unchanged.

Try [the import example](../../../literate/import-filter.rbel.md):

```sh
bin/rbel build examples/import-filter.rbel.md -o ../import-uppercase
```

The command snapshots the complete dependency graph before touching the output.
Output/source aliases, hard links to any loaded input, protected source extensions,
and bundle-resource output paths are rejected before cleanup. Discovery failures
such as missing files, cycles, or malformed imports preserve any existing output.
After graph and output validation, a previous output is removed before ordinary
fence/example validation and compilation, so those failures leave no stale product.
Successful products are renamed from a private directory beside the output.
Filenames are passed as process arguments, never interpolated into shell commands.

Run `python3 experiments/bel-rust-repl/wasm/command/verify-imports.py` for isolated
native import checks, including nested and diamond dependencies, cycles, missing
files, quoted forms, main-entry behavior, imported examples, and transitive aliases.

## Lint file and block lengths

```sh
bin/rbel lint path/to/workflow.rbel.md
bin/rbel lint path/to/workflow.rbel.md --file-limit 600 --block-limit 100
```

The command warns when a file exceeds 500 physical lines or a rbel fenced
block exceeds 80 content lines. Both limits accept integers from 0 through
131072 (the maximum line count in a supported 128 KiB file). Exactly the limit
is allowed. Blank lines and comments count; a trailing newline adds no extra
line, and an unterminated final line counts once. LF and CRLF are supported.
Block counts exclude the opening/closing fences. Whole-file counts include
prose and fences. Only `.md` inputs receive block checks; other files receive
the whole-file check.

Warnings include the path, opening-fence location (line 1 for a file warning),
actual count, limit, and rbel fence label where present. All rbel fence kinds,
including definition, example, and test, are eligible. Bare rbel fences and
empty blocks are supported for linting. Other-language fences are skipped,
including any apparent rbel fences within them. Fence markers, indentation,
and closing widths use the command's existing Markdown fence helper.
Linting counts source without parsing or evaluating its rbel expressions.
Malformed/unclosed Markdown fences, unreadable files, invalid UTF-8/control
bytes, source bounds, and execution bounds are errors. Linting does not require
definition/example pairs or validate block attributes as a build does.

Findings are warnings and return 0; file/format/execution failures return 1;
invalid options return 2. A clean file prints nothing. The command reads one
explicit file and makes no edits. Rules, explanation, and 17 compiled examples
live in [the separate lint module](../../../literate/lint.rbel.md). C supplies
file input, fence extraction, physical-line counts, and diagnostic formatting;
the compiled rbel module compares counts with limits and selects findings.
No model, external interpreter, or build tools are required to run `lint`.

Run `python3 experiments/bel-rust-repl/wasm/command/verify-lint.py` after building
the command to exercise boundary cases, errors, relocation, and the real
libcwr workflow source. It records results under `target/lint-verification`.

## Reader and compiler boundaries

The reader implements whitespace, semicolon comments, proper/dotted lists,
apostrophe quotation, strings, and character notation. Symbol tokens use ASCII
letters, digits, and hyphens, plus the integer operators `+`, `-`, `=`, `<`,
`<=`, `>`, and `>=`. Signed decimal integer literals are supported; malformed
numeric tokens, fractions, decimal points, and exponent notation are rejected.
Bracket functions, backquote, unquote, and other abbreviations remain unsupported. This is the reader for the compiled subset, not every Bel
notation. Character names use ASCII letters and the names sp/tab/lf/cr. String
escapes take the next character literally, matching the existing reader's rule.

Integer values use Bel's existing `(lit num ...)` representation, made of pairs
and symbols. A literal evaluates to itself, including inside quoted lists and
records. Zero is distinct from `nil` and is true in a conditional. Numeric
comparison uses `=`, rather than pair identity with `id`.

Import [the integer library](../../../literate/integers.rbel.md), or supply it
with `--include`, to use arithmetic and comparisons. It provides signed addition,
subtraction, equality and ordering. These operations check their operands and
reject results outside the documented magnitude bound. `integer-text` converts
an integer to decimal text for a byte-list filter's output. The `read` command
continues to expose the underlying value tree as JSON; it does not evaluate or
pretty-print numeric values.

The compiler's fixed-arity functions, lexical closures, and remaining limits are
in [higher-order compilation](higher-order.md). `sym` is now a compiled primitive:
it interns a character list as a symbol, providing the reader's symbol boundary.
Unicode text can be transported and read, but target source literals remain
limited to the compiler's documented ASCII/tab/LF/CR coverage.

The C command handles files, Markdown block extraction, UTF-8 transport, native
processes, and output publication. The rbel reader constructs source values;
`frontend.bel` constructs compiled checks; `self-compiler.bel` makes compilation
decisions. Native WABT assembles Wasm and translates it to C. Apple's compiler
produces the executable. No C or Rust expression evaluator is used.

Each file is limited to 128 KiB, as are combined definition text and example-row
text separately. At most 32 files and 2048 block names are accepted. Each core
action uses a fresh bounded 64 MiB arena and a 4096-frame call allowance; source
within the byte bound can still exceed structural/stack limits. Text output is
limited to 8 MiB; debug JSON rendering has a 128-depth/32768-step bound. Produced
filters/checkers retain the native 1024-frame limit. Bound failures are errors.

## Build the command from the maintained project

The maintained source is in `randallb.com/Memos/code/randallb-com/projects/bel-wasm`.
The first bootstrap uses the existing Rust reader and Node transport once:

```sh
cargo build --locked -p bel-rust-repl --bins
python3 experiments/bel-rust-repl/wasm/verify-selfhost.py
python3 experiments/bel-rust-repl/wasm/command/setup-tools.py
python3 experiments/bel-rust-repl/wasm/command/build-command.py
python3 experiments/bel-rust-repl/wasm/command/verify-command.py
```

`setup-tools.py` builds native WABT from the existing checksum-pinned 1.0.39
archive, plus WABT's pinned PicoSHA2 submodule. It fixes upstream CMake argument
quoting for paths with spaces. If CMake is absent it installs CMake 3.31.6 into
ignored build output; it does not change the user's global Python environment.
Third-party licenses are included in the installed bundle.

Once a native command exists, it can seed another command build:

```sh
python3 experiments/bel-rust-repl/wasm/command/build-command.py \
  --seed target/rbel-command/bin/rbel
```

This packaging script still uses Python for build orchestration, but its seed
route invokes neither Rust nor Node to read or compile the new command's core.
The existing command is preserved until the replacement binary compiles.
The language core is generated from the maintained reader chapter, frontend,
and compiler. Generated source/AST/WAT/C stay under `target/rbel-command-build`.
The installed command and its normal build/check/emit/read operations use no
Python script. No new machine must have Rust merely to use this bundle.

## Verification

`verify-command.py` moves a copy of the bundle into a separate temporary directory,
with only copied sources supplied to the command. Rust, Cargo, Node, Python, and
Deno names on PATH are replaced with traps recording any attempted invocation.
All supported operations pass and no trap is invoked. The external tools in the
bundle are ARM64 Mach-O executables linked only to macOS system libraries.

The verifier checks 22 reader comparisons with the previous Rust reader as a
development reference, 27 malformed/unsupported/bounded input cases, 67 foundation
examples, 12 reader examples, both native uppercase builds, the 77-example combined
build, and byte fixtures including NUL, UTF-8, chunk boundaries, and over 1 MiB.
It covers malformed/duplicate/unclosed fences, example mismatch and traps,
source/output aliases, stale-product cleanup, missing tools, recovery, spaces,
metacharacter filenames, and complete debug rendering before output.

The native command reproduces both the compiler's WAT and its own complete core
WAT byte-for-byte. A native-seeded command rebuild also matches the first
bootstrap's core Wasm. The old Rust reader is used only as a test reference in
the comparison phase; it is not called by the native command or its children.
These are bootstrap consistency checks with shared compiler/runtime definitions,
not an independent proof of all language semantics.

`target/rbel-command-verification/results.json` records the test counts, library
links, and binary hash. The bundle's `MANIFEST.json` records source and file hashes.
Original foundation definitions and their source pins remain unchanged. No code
has been committed or published by this milestone.
