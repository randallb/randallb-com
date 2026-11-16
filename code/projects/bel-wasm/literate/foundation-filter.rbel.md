# A native filter built from the foundation

The [foundation chapter](foundation.rbel.md) defines no, atom, all, some, and
reduce. This program uses those exact definitions to uppercase a byte stream.
It uses [the earlier byte transformation](unix-filter.rbel.md) for upper-byte;
there is only one maintained copy of that letter mapping.

## Functions as inputs

The entry function receives a proper list of byte characters, xs. It creates a
function with parameter transform, capturing xs from the surrounding call, then
calls that function with upper-byte. Inside it:

- all checks that every input element is an atom. Byte characters satisfy this.
- some returns the suffix at the first atom. For a nonempty byte list this is
  the whole input; for empty input it is nil.
- byte-pieces calls transform for each character and puts each answer in its
  own one-element list.
- reduce combines those pieces into a proper output list.

The last element produced by byte-pieces is an empty list. That gives the right
fold its final tail: combining a one-element piece with the remaining output
means joining the piece's head to that output. Even empty input reduces to nil.
This explicit final empty piece prevents reduce from making a dotted byte list.

The combining function passed to reduce is another fn value. The compiled
program therefore exercises calls through function values and captured bindings,
as well as the foundation's unchanged recursive functions.

```rbel definition=foundation-uppercase
(def foundation-uppercase (xs)
  ((fn (transform)
     (if (all atom xs)
         (reduce (fn (piece rest) (join (car piece) rest))
                 (byte-pieces transform (some atom xs)))
         (err 'non-atomic-input)))
   upper-byte))

(def byte-pieces (transform xs)
  (if xs
      (join (join (transform (car xs)) nil)
            (byte-pieces transform (cdr xs)))
      '(nil)))
```

## Examples

```rbel example=foundation-empty
(foundation-uppercase nil)
=>
nil
```

```rbel example=foundation-text
(foundation-uppercase "hello, rbel!")
=>
"HELLO, RBEL!"
```

```rbel example=foundation-mixed
(foundation-uppercase "aZ !")
=>
"AZ !"
```

## Build with the native command

With the [native rbel command](../experiments/bel-rust-repl/wasm/native-command.md)
available, run from the project root:

```sh
rbel build literate/foundation-filter.rbel.md --include literate/foundation.rbel.md --include literate/unix-filter.rbel.md -o target/foundation-uppercase
```

This checks the literate examples before producing the executable. The earlier
project bootstrap route is retained below.

## Build and run

After the [bootstrap and native setup](../experiments/bel-rust-repl/wasm/native/readme.md),
run from the project root:

```sh
python3 literate/build-foundation-native.py
python3 literate/verify-foundation-filter.py
printf 'hello, rbel!\n' | target/bel-native/rbel-foundation-uppercase
```

The result is `HELLO, RBEL!` and a newline. The build checks the original
foundation pins, combines the three literate sources, checks this chapter's
examples, and builds native code. The entire foundation's 67 examples also
have a [native verification command](../experiments/bel-rust-repl/wasm/higher-order.md).

As in the earlier filter, the adapter reads independent chunks of up to 256
bytes. Every byte other than ASCII lowercase passes through unchanged, so chunk
boundaries do not change the transformation. The executable needs only system
libraries on macOS; the bootstrap tools are build dependencies.

Explanation and program by Assistant for Randall Bennett. The five foundation
definitions retain their original Bel attribution and bytes. See the separate
[higher-order compiler guide](../experiments/bel-rust-repl/wasm/higher-order.md)
for supported syntax and remaining language boundaries.
