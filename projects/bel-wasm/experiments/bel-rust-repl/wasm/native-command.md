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
```

`--help` and `--version` return 0. Usage errors return 2; source, example, tool,
or resource failures return 1. `--quiet` suppresses successful build/check messages.

`.md` inputs use the existing `rbel definition=NAME` and `rbel example=NAME`
fences. Every literate input requires both kinds. Definition fence names must be unique within each input. Example labels are
local to their source unit; imported modules may reuse them. Example bodies contain one expression, an entire line `=>`, and one
expected value. Backtick and tilde fences, up to three leading spaces, CRLF,
and explanatory fences follow the extractor's supported rules. Unknown rbel
attributes, duplicate names, unclosed fences, and malformed examples fail.
Raw `.rbel`/`.bel` inputs contain definitions directly; a raw build may have no
examples, but `check` requires examples. There is no implicit foundation import.

`check` compiles a separate example module and runs each named check in a fresh
native process. Expected values are read as data, never evaluated. The structural
comparison helper is generated in Bel. Results and language errors identify the example,
source file, and opening-fence line. The prefix `rbel-internal-` is reserved for
these generated checks. There is no expected-error fence notation yet.

`emit` checks any examples before writing Wasm text. It is useful for compiler
bootstrapping. `read` reports parsed definitions as a JSON value tree, using
`{"pair":[head,tail]}` for pairs, `{"char":"x"}` for characters, strings for
symbols, and null for nil. It does not evaluate source. Rendering completes in
memory before stdout is written; a bounded-render failure prints no partial JSON.

Output/source aliases, hard links to inputs, and bundle-resource output paths
are rejected before cleanup. After that validation, a previous output is removed
before reading/compiling, so failures do not leave a stale runnable product.
Successful products are renamed from a private directory beside the output.
Filenames are passed as process arguments, never interpolated into shell commands.

## Reader and compiler boundaries

The reader implements whitespace, semicolon comments, proper/dotted lists,
apostrophe quotation, strings, and character notation. Symbol tokens use ASCII
letters, digits, and hyphens, plus arithmetic operators `+`, `-`, `*`, `/`,
`=`, `<`, `<=`, `>`, and `>=`. Exact integer, fraction, decimal and exponent
literals are supported; `f` suffixes request finite IEEE binary64 floats.
Bracket functions, backquote, unquote, and other abbreviations remain unsupported.
Character names use ASCII letters and sp/tab/lf/cr. String escapes take the next
character literally, matching the existing reader's rule.

Exact values use `(lit num sign numerator denominator)` with decimal character
lists; floats use `(lit float bits)` with a 64-bit binary character list. These
replace the old unary integer representation; old raw numeric value trees are
not compatible. Rebuild generated modules and migrate persisted raw numeric
values before using them with the new library. Literals evaluate to themselves,
including in quoted data. Zero is true. Use `=` for numeric equality; `id`
continues to compare pair identity.

Include [exact arithmetic](../../../literate/integers.rbel.md) for `+`, `-`, `*`,
`/`, equality, ordering, and exact text output. Include the
[float chapter](../../../literate/floats.rbel.md) too for `float`, `exact`,
`float-text`, and `number-text`. Mixed exactness is an error. Float overflow and
nonfinite results are errors; underflow rounds to signed zero. The reader chapter
itself depends on both libraries when compiled as an example suite:

```sh
bin/rbel check examples/reader.rbel.md --include examples/integers.rbel.md --include examples/floats.rbel.md
bin/rbel check examples/floats.rbel.md --include examples/integers.rbel.md
```

Run `python3 experiments/bel-rust-repl/wasm/command/verify-numbers.py` after
building the command. It checks independent exact and binary64 expected values,
boundary/failure cases and an executable that prints a large integer. The `read`
command exposes value trees as JSON; it does not evaluate or pretty-print them.
See the [numeric contract](../../../literate/numbers.md).

The [ABI2 language core](../../../language-core/readme.md) supports proper tail
calls, GC, recoverable errors, rest arguments and `apply`, local binding,
sequencing, namespace modules, and Unicode scalar literals. That guide records
host rooting, explicit external resource lifetimes and migration requirements.
`(module name)`, `(export names...)` and `(import "relative/path" as alias)`
provide private definitions and qualified references. Bare legacy imports combine
legacy units without introducing module visibility rules.

The C command handles files, Markdown extraction, UTF-8 transport, native
processes and output publication. The rbel reader constructs source values;
`frontend.bel` constructs checks; `modules.bel` resolves and renames bindings;
`self-compiler.bel` makes compilation decisions. Native WABT assembles Wasm and
translates it to C. Apple's compiler produces the executable.

Source inputs have a combined 512 KiB transport bound. At most 32 files and 2048
block names are accepted. Compiler instances use four million pair cells, plus
roots and collector metadata, below the 64 MiB total memory maximum. Text output
is limited to 32 MiB. Non-tail calls have an explicit maximum of 512; tail calls
use bounded stack. Native filters reuse an initialized collecting instance across
input chunks. Source within the byte bound can still exceed configured limits;
these failures report language errors.

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
