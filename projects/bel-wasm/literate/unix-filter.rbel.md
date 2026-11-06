# From a literate program to a Unix command

The [foundation](foundation.rbel.md) explains values, pairs, choices, functions,
and recursion. Here we use those pieces to make a command that reads text and
writes a transformed result. A **command** is a program the operating system
can start. **Standard input** (stdin) supplies bytes to the program; **standard
output** (stdout) carries its result. A **byte** is a value from 0 through 255
used to represent stored or transmitted data.

Our command changes the ASCII lowercase letters a through z to A through Z.
**ASCII** assigns byte values to basic letters and punctuation. Every other byte
passes through unchanged. This is not a complete Unicode case-conversion tool;
for example an accented letter keeps its original bytes.

## The program boundary

The runner presents a chunk of input as a proper list of characters, one per
byte, and calls the first function below. The function returns a proper list
of byte characters to write. The runner may split input into chunks of up to
256 bytes. Our transformation works on each character independently, so this
splitting does not change the answer. An empty input stream produces no output.

`uppercase` walks the list just as the foundation's recursive functions did.
If xs is nonempty, transform its head, process its tail, and join the results.
If xs is empty, return nil. `upper-byte` uses repeated choices to select a
capital letter. Its final fallback is the original character c. Calls may name
a function defined later: uppercase can call upper-byte.

This example uses direct named calls. The compiler now also supports functions
as values and fixed-arity closures; the
[foundation-based filter](foundation-filter.rbel.md) builds on this transformation
to exercise all, some, and reduce through that extended route.

## Executable definitions

These are the maintained definitions for the literate native example. The build
extracts them in this order; the first definition is the program's entry point.
The lower-level native guide retains the earlier standalone example as a baseline.

```rbel definition=uppercase-filter
; Byte-list stdin/stdout filter. Only ASCII lowercase bytes are changed.
(def uppercase (xs)
  (if xs (join (upper-byte (car xs)) (uppercase (cdr xs))) nil))

(def upper-byte (c)
 
    (if (id c \a) \A
    (if (id c \b) \B
    (if (id c \c) \C
    (if (id c \d) \D
    (if (id c \e) \E
    (if (id c \f) \F
    (if (id c \g) \G
    (if (id c \h) \H
    (if (id c \i) \I
    (if (id c \j) \J
    (if (id c \k) \K
    (if (id c \l) \L
    (if (id c \m) \M
    (if (id c \n) \N
    (if (id c \o) \O
    (if (id c \p) \P
    (if (id c \q) \Q
    (if (id c \r) \R
    (if (id c \s) \S
    (if (id c \t) \T
    (if (id c \u) \U
    (if (id c \v) \V
    (if (id c \w) \W
    (if (id c \x) \X
    (if (id c \y) \Y
    (if (id c \z) \Z c)))))))))))))))))))))))))))
```

## Examples

A quoted list of characters supplies input directly. A **string** is convenient
notation for a list of characters, written between double quotation marks.
For example, "abc" is the same character list as the quoted list containing
backslash-a, backslash-b, and backslash-c. Expected strings below are data.

```rbel example=empty-input
(uppercase nil)
=>
nil
```

```rbel example=lowercase-character
(upper-byte \a)
=>
\A
```

```rbel example=uppercase-character
(upper-byte \Z)
=>
\Z
```

```rbel example=punctuation
(upper-byte \!)
=>
\!
```

```rbel example=character-list
(uppercase '(\h \i))
=>
"HI"
```

```rbel example=text
(uppercase "hello, Bel!")
=>
"HELLO, BEL!"
```

```rbel example=alphabet
(uppercase "abcdefghijklmnopqrstuvwxyz")
=>
"ABCDEFGHIJKLMNOPQRSTUVWXYZ"
```

## Build with the native command

With the [native rbel command](../experiments/bel-rust-repl/wasm/native-command.md)
available, run from the project root:

```sh
rbel build literate/unix-filter.rbel.md -o target/hello
```

This checks the literate examples before producing the executable. The earlier
project bootstrap route is retained below.

## Build and use

The [native build guide](../experiments/bel-rust-repl/wasm/native/readme.md)
records bootstrap setup, build dependencies, the byte interface, and error limits.
After its compiler and tools are prepared, run from the project root:

```sh
python3 literate/build-native.py
printf 'hello, rbel!\n' | target/bel-native/rbel-uppercase
```

The expected output is `HELLO, RBEL!` followed by a newline. `--help` describes
the command; success returns exit status 0, unsupported arguments return 2,
and processing failures return 1. Diagnostics go to standard error, a separate
output channel used for errors.

The build extracts this chapter, checks these seven examples, and produces a
native executable for the build machine. The current verified target is macOS
ARM64. The resulting command needs system libraries, but none of the bootstrap
build tools at execution time. It can be copied and run as a normal Unix program.

Explanation by Assistant for Randall Bennett, November 5, 2026. The recovered
Bel transformation is from the earlier native experiment in this project's
source export; [native provenance](../NATIVE-ORIGIN.json) records the imported
files. The original foundation definitions and source pins are unchanged.
