# Native Unix executables from Bel

The `uppercase.bel` example builds into a real native Unix executable. It reads
stdin, uppercases ASCII lowercase bytes in Bel, and writes stdout. The build
uses the self-hosted compiler:

```text
Bel source -> reader data -> Wasm Bel compiler -> Wasm -> wasm2c -> C -> executable
```

The C adapter implements byte input/output and value transport. The Bel source
implements the transformation. WABT's C support is linked into the executable;
running it requires system libraries but no Node, Rust, Python, WABT installation,
separate Wasm file, or separately installed Wasm engine.

The first implementation was verified on macOS arm64 as a Mach-O executable.
Its only dynamically linked library is the system's `libSystem.B.dylib`. The
builder also targets ordinary Unix C toolchains, but Linux execution has not yet
been verified; run the verification script on each intended target.

## Literate authoring

The maintained literate example is [unix-filter.rbel.md](../../../../literate/unix-filter.rbel.md).
After the setup below, `python3 literate/build-native.py` extracts its definitions,
checks its seven examples, and builds `target/bel-native/rbel-uppercase`.
The earlier `uppercase.bel` remains the baseline used by the native regression suite.
The literate example uses the same transformation through the same compiler/adapter.
On macOS the builder defaults to `/usr/bin/clang` to avoid non-system libraries
introduced by package-manager C compiler wrappers. Override with `--cc` when needed.

## Build and run

From this standalone source distribution's root, with Cargo/Rust, Python 3,
Node/npm, and a C99 compiler available:

```sh
cargo build --locked -p bel-rust-repl --bins
npm install --prefix target/bel-wasm-tools --no-save --ignore-scripts wabt@1.0.39
python3 experiments/bel-rust-repl/wasm/verify-selfhost.py
python3 experiments/bel-rust-repl/wasm/native/setup-runtime.py
python3 experiments/bel-rust-repl/wasm/native/build-native.py \
  experiments/bel-rust-repl/wasm/native/uppercase.bel \
  target/bel-native/bel-uppercase \
  --artifacts target/bel-native/uppercase-build
printf 'hello, Bel!\n' | target/bel-native/bel-uppercase
```

Expected output:

```text
HELLO, BEL!
```

`verify-selfhost.py` bootstraps and verifies the compiler. If the current
`target/bel-selfhost/stage1.wasm` already exists, a native filter build can reuse
it. `build-native.py` accepts `--compiler`, `--reader`, `--tools`, `--runtime`,
`--cc`, and `--artifacts` to select other build inputs and retain intermediate
JSON, WAT, Wasm, C, and header files. All build inputs must be trusted project
artifacts; this is an experimental native toolchain.

`setup-runtime.py` downloads the official WABT 1.0.39 source archive, verifies
SHA256 `8aaafc02afdb8832bd51140b833816512a834601a65348e1f433cece6bbf2db7`, and
extracts only top-level C runtime files and the license into
`target/wabt-native-1.0.39`. The builder requires the matching wasm2c version.
The third-party source remains in build output; the transformation needs no
network access after the tools and runtime source are installed.

## Program interface

The first definition must take exactly one argument and return one value. Its
argument is a proper list of Bel characters representing bytes 0 through 255;
its result must also be a proper byte-character list. Other function definitions
may have any arity supported by the [self-hosting core](../selfhost.md).

The adapter calls that function separately on each input chunk of up to 256
bytes. It creates a fresh module instance for each chunk, reclaiming the arena
before the next chunk. Chunk lengths follow stdin reads; there is no invocation
for an empty stream. Programs must be designed for independent chunks. For
example, uppercase mapping composes across chunks; whole-input reversal does not.
This is a byte filter, so bytes forming non-ASCII UTF-8 sequences pass through
the uppercase example unchanged. It does not perform Unicode case conversion.

Each result is checked completely before that chunk is written. Results are
limited to 1 MiB per chunk. Later failures may occur after earlier chunks were
written. The runtime retains the module's 64 MiB arena ceiling and checks native
call depth at 1024. Traps and invalid output fail with stderr diagnostics and
exit status 1. Input/output failures also return 1; unsupported command-line
arguments return 2; `--help` and successful transformations return 0. SIGPIPE is
handled as an ordinary output error.

The builder removes an old executable before attempting a new build, and
publishes a replacement only after compilation succeeds. Optional retained
intermediates are updated on successful builds; an older artifact directory may
remain after a failed build. Source files and compiler inputs are protected
against output-path collisions.

## Verification

```sh
python3 experiments/bel-rust-repl/wasm/native/verify-native.py
```

The verifier rebuilds programs and checks:

- Native executable format and execution in an otherwise empty temporary
  directory with an empty PATH.
- Empty stdin, ordinary text, every byte value, embedded NUL, UTF-8, the
  255/256/257-byte boundaries, and a stream larger than 1 MiB.
- A changed Bel program producing different native behavior through the same
  adapter.
- Explicit errors, infinite recursion reaching a trap, and invalid output
  atoms, character values, and list tails.
- Real stdin read errors, closed stdout pipes, help, and usage exit statuses.
- Invalid source, malformed reader input, wrong entry arity, missing C compiler,
  stale-executable cleanup, source preservation, and successful recovery.

`target/bel-native/native-results.json` records the platform, native binary
hash, size, and build path. Redirect stdout/stderr to retain the verification
summary. The proof covers the native filter path; no Rust evaluator or compiler
semantics were changed for this step.

The next larger interfaces would be a native compiler command that accepts Bel
source files, and language-level Unix I/O for general applications. The existing
compiler still emits Wasm, which this build pipeline translates to native code.
