# Numeric contract

Accepted direction: [Randall, November 6, 2026](cwp://local.randalls-macbook-pro/runtimes/e8609410f60cdd3cd357fad57f9dda5c027e77ca5410198e36811fbad7939e04/messages/e29b497d74a64f58bd79c304f76266cf),
in the originating rbel conversation.
This implementation extends the bounded signed-integer milestone on PR #10.

| Spelling | Value |
| --- | --- |
| `42`, `-42` | Exact integer, decimal digits rather than unary magnitude |
| `1/3` | Reduced exact rational; denominator must be positive |
| `0.1` | Exact `1/10` |
| `1.25e3`, `12.5e-3` | Exact `1250`, `1/80` |
| `0.1f`, `1/3f`, `1.25e3f` | Finite IEEE754 binary64, nearest with ties to even |

Decimal points require digits on both sides. Exponents use `e` or `E` and an
optional sign. Leading zeroes and a leading sign are accepted. Zero exact values
normalize to positive integer zero; float negative zero retains its sign bit.
Only `nil` is false. Numeric comparison uses `=`, not pair identity `id`.

`+`, `-`, `*`, `/`, `=`, `<`, `<=`, `>`, `>=` take two operands with the same
exactness. `(float value)` explicitly approximates an exact number; `(exact value)`
recovers the exact rational represented by a float. Both are identities for
values already in the requested domain. Division by zero, malformed values,
mixed exactness, float overflow, and nonfinite float results fail. NaN/infinity
are outside this first finite interface. Gradual underflow and signed zero work.

`integer-text` prints exact integers; `exact-text` prints integers or reduced
fractions. `float-text` prints a value-preserving fraction with `f`, or `0f`/
`-0f`. `number-text` selects either printer. Printed text reads back with the same
value and exactness; spelling need not match the input. Compact shortest-decimal
float printing is not implemented.

## Representation and limits

Exact values are `(lit num sign numerator denominator)`, with canonical decimal
character lists, positive denominator, reduced fraction, and unique exact zero.
Float values are `(lit float bits)`, with 64 MSB-first binary characters. Ordinary
pair transport and persistence preserve these values without host JSON numeric
rounding. These shapes replace the earlier unary numeric literal trees. Rebuild
compiled modules; migrate any persisted old numeric trees explicitly. Existing
symbol/character/pair transport is unchanged.

Integers have no fixed magnitude limit. The reader guards 4096 decimal digits and
exponent magnitude 4096; expanded exact decimal components must fit 4096 digits.
These are resource ceilings, not promises every input of that size executes.
The existing 128 KiB source limit, bounded arena (64 MiB), recursion/step budgets,
and process timeouts also apply. Native compiler and emitted programs share a
4096-call depth budget. Exhaustion reports a failure rather than silently
changing a number or leaving a successful executable.

Arithmetic, normalization, reading and exact-to-float rounding are implemented
in rbel. Float operations use Wasm `f64` instructions, including native products
translated through wasm2c. Rust, Python and Node participate only in bootstrap,
verification or transport; native installed command/program execution needs none.

## Verification

From the project root, after building the command:

```sh
python3 experiments/bel-rust-repl/wasm/command/verify-numbers.py
python3 experiments/bel-rust-repl/wasm/command/verify-command.py
python3 libcwr/verify.py
```

The numeric verifier uses independently generated Python int/Fraction/struct
expected data. Rbel reads and executes all expressions. Cases cover large
arithmetic, normalization, exact decimal equality, closures/quoted records,
binary64 bit patterns, ties, subnormals, signed zero, boundaries, and failures.
`--portable --rbel target/rbel-wasm-bootstrap/bin/rbel` runs the same cases on
Wasm and explicitly omits standalone native executable acceptance. Hosted Linux
CI bootstraps the actual rbel reader/compiler and runs that portable suite.
