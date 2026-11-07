# Functions, local values, recovery and Unicode

The compiled core supports fixed and rest arguments, first-class `apply`, lexical
closures, local `let`, left-to-right `do`, and catchable language failures. Only
`nil` is false. An empty `do` is nil; function bodies may contain several
expressions and return the final result. These forms work on Wasm and native
wasm2c output with the same source semantics.

```rbel definition=core-functions
(def core-identity (value) value)
(def core-gather arguments arguments)
(def core-first (first . remaining) first)
(def core-capture (value) (fn () value))
(def core-multiple-body (value)
  (core-identity value)
  'last)
(def core-tail (values)
  (if values (core-tail (cdr values)) 'done))
```

```rbel example=core-rest-and-apply
(apply core-gather '(a b c))
=>
(a b c)
```

```rbel example=core-rest-tail-identity
(let values '(a b c)
  (id (apply core-gather values) values))
=>
t
```

```rbel example=core-required-rest-prefix
(core-first 'a 'b 'c)
=>
a
```

```rbel example=core-lexical-initializer
(let value 'outside
  (let value (join value nil)
    (car value)))
=>
outside
```

```rbel example=core-escaping-capture
(let answer (core-capture 'retained)
  (answer))
=>
retained
```

```rbel example=core-empty-sequence
(do)
=>
nil
```

```rbel example=core-multiple-function-body
(core-multiple-body 'first)
=>
last
```

```rbel example=core-error-stops-sequence
(try (do (err 'first) (err 'second))
  (catch problem problem))
=>
first
```

```rbel example=core-nested-recovery
(try (try (err 'first) (catch problem (err 'second)))
  (catch problem problem))
=>
second
```

```rbel example=core-dynamic-arity-recovery
(let operation core-identity
  (try (operation 'a 'b) (catch problem problem)))
=>
arity-error
```

```rbel example=core-primitive-shadow
(let apply core-gather
  (apply 'a 'b 'c))
=>
(a b c)
```

```rbel example=core-tail-traversal
(core-tail '(a b c d))
=>
done
```

```rbel example=core-unicode-character
(char-code \é)
=>
233
```

```rbel example=core-supplementary-character
(code-char 128521)
=>
\😉
```

```rbel example=core-quoted-scalar-boundaries
'(\u{0} \u{D7FF} \u{E000} \u{10FFFF})
=>
(\u{0} \u{D7FF} \u{E000} \u{10FFFF})
```

A known function called with the wrong number of arguments is a compile error,
even in an unchosen branch. Computed calls check arity at execution and can be
caught. A catch handles language failures, not invariant-breaking runtime traps.
Catching a failure performs no rollback or external-resource cleanup.

Tail calls transfer to the next function through a dispatcher loop, including
mutual recursion, closure calls and `apply`. The collector reclaims unreachable
pairs, captures and symbol names. Ordinary rbel code needs no `free`. Host-held
values use explicit retain/release tokens. Host resources still require explicit
close/cancel actions; GC runs no resource finalizers.

Unicode characters and byte values are distinct interfaces. Use
[UTF-8 conversion](utf8.rbel.md) when crossing the native byte-filter boundary.
See [the design](language-core-design.md) for ABI2, roots, error semantics,
resource ceilings, namespace rules and verification scope.
