# Explicit floating-point numbers

Unsuffixed integers, fractions, decimals and exponents are exact. Add `f` for
finite IEEE 754 binary64 arithmetic: `0.1f` is approximate. Use `(float value)`
to convert an exact value, and `(exact value)` to recover the exact rational
value represented by a float. Arithmetic rejects mixed exact/float operands.
Include this chapter and `integers.rbel.md` for these conversion functions.

Conversion rounds once to nearest, with ties to even, including subnormals.
Negative zero is retained for float literals and arithmetic. Overflow and
nonfinite arithmetic results are errors in this first finite-float interface;
NaN and infinity have no literal spelling. Underflow rounds to signed zero.
The arithmetic instructions themselves execute in Wasm, including native builds.

The representation is `(lit float bits)`, where `bits` is a 64-character binary
string containing the sign, exponent and fraction in IEEE order. Ordinary pair
transport therefore preserves every bit without a host JSON number conversion.

## Binary conversion

The exact library supplies decimal magnitude arithmetic. No host parser or
floating-point approximation is used to decide how an exact input rounds.

```rbel definition=float-conversion
(def f-zero-exponent () "00000000000")
(def f-first-exponent () "00000000001")
(def f-last-exponent () "11111111111")
(def f-bias () "01111111111")
(def f-fraction-slots () "0000000000000000000000000000000000000000000000000000")
(def f-same (a b)
  (if a (if b (if (id (car a) (car b)) (f-same (cdr a) (cdr b)) nil) nil)
    (if b nil t)))
(def f-increment-reversed (bits)
  (if bits
    (if (id (car bits) \0) (join nil (join \1 (cdr bits)))
      ((fn (answer) (join (car answer) (join \0 (cdr answer))))
       (f-increment-reversed (cdr bits))))
    (join t nil)))
(def f-increment (bits)
  ((fn (answer)
     (if (car answer) (err 'float-overflow)
       (n-reverse (cdr answer) nil)))
   (f-increment-reversed (n-reverse bits nil))))
(def f-decrement-reversed (bits)
  (if bits
    (if (id (car bits) \1) (join \0 (cdr bits))
      (join \1 (f-decrement-reversed (cdr bits))))
    (err 'float-exponent-underflow)))
(def f-decrement (bits)
  (n-reverse (f-decrement-reversed (n-reverse bits nil)) nil))
(def f-raw (sign exponent fraction)
  (join 'lit (join 'float
    (join (join (if (id sign '-) \1 \0)
      (n-append exponent fraction)) nil))))
(def f-finish (sign exponent reversed)
  (if (f-same exponent (f-last-exponent)) (err 'float-overflow)
    (f-raw sign exponent (n-reverse reversed nil))))
(def f-rounded (sign exponent reversed)
  ((fn (answer)
     (f-finish sign (if (car answer) (f-increment exponent) exponent)
       (cdr answer)))
   (f-increment-reversed reversed)))
(def f-round (sign exponent reversed remainder denominator)
  ((fn (twice)
     (if (n-less denominator twice) (f-rounded sign exponent reversed)
       (if (n-equal twice denominator)
         (if (id (car reversed) \1) (f-rounded sign exponent reversed)
           (f-finish sign exponent reversed))
         (f-finish sign exponent reversed))))
   (n-add remainder remainder)))
(def f-fraction (sign exponent numerator denominator slots reversed)
  (if slots
    ((fn (twice)
       (if (n-less twice denominator)
         (f-fraction sign exponent twice denominator (cdr slots) (join \0 reversed))
         (f-fraction sign exponent (n-sub twice denominator) denominator
           (cdr slots) (join \1 reversed))))
     (n-add numerator numerator))
    (f-round sign exponent reversed numerator denominator)))
(def f-double (x) (n-add x x))
(def f-double-four (x) (f-double (f-double (f-double (f-double x)))))
(def f-double-sixteen (x)
  (f-double-four (f-double-four (f-double-four (f-double-four x)))))
(def f-exponent-sixteen (exponent up)
  (n-append (if up (f-increment (f-take exponent "0000000"))
    (f-decrement (f-take exponent "0000000"))) (f-drop exponent "0000000")))
(def f-normalize (sign numerator denominator exponent)
  ((fn (magnitude)
     (if (n-less numerator denominator)
       (if (n-less "16" magnitude)
         ((fn (scaled)
            (if (n-less denominator scaled)
              (f-normalize-one sign numerator denominator exponent)
              (f-normalize sign scaled denominator (f-exponent-sixteen exponent nil))))
          (f-double-sixteen numerator))
         (f-normalize-one sign numerator denominator exponent))
       (if (n-less magnitude "2031")
         ((fn (scaled)
            (if (n-less numerator scaled)
              (f-normalize-one sign numerator denominator exponent)
              (f-normalize sign numerator scaled (f-exponent-sixteen exponent t))))
          (f-double-sixteen denominator))
         (f-normalize-one sign numerator denominator exponent))))
   (f-binary-magnitude exponent "0")))
(def f-normalize-one (sign numerator denominator exponent)
  (if (n-less numerator denominator)
    (if (f-same exponent (f-first-exponent))
      (f-fraction sign (f-zero-exponent) numerator denominator (f-fraction-slots) nil)
      (f-normalize sign (n-add numerator numerator) denominator (f-decrement exponent)))
    (if (n-less numerator (n-add denominator denominator))
      (f-fraction sign exponent (n-sub numerator denominator) denominator
        (f-fraction-slots) nil)
      ((fn (next)
         (if (f-same next (f-last-exponent)) (err 'float-overflow)
           (f-normalize sign numerator (n-add denominator denominator) next)))
       (f-increment exponent)))))
(def f-from-exact (value)
  (if (n-equal (n-numerator value) "0")
    (f-raw (n-sign value) (f-zero-exponent) (f-fraction-slots))
    (f-normalize (n-sign value) (n-numerator value) (n-denominator value) (f-bias))))
(def float (value)
  (if (float-number value) value
    (if (exact-number value) (f-from-exact value) (err 'expected-number))))
(def f-literal (sign value)
  ((fn (answer)
     (join 'lit (join 'float
       (join (join (if (id sign '-) \1 \0) (cdr (car (cdr (cdr answer))))) nil))))
   (float value)))
```

## Exact recovery and printing

The printer emits an exact fraction followed by `f`. This can be longer than
the original decimal spelling, but reading it recovers precisely the same float.
For example, `0.5f` prints as `1/2f`; `0.1f` prints as
`3602879701896397/36028797018963968f`. This avoids a second approximation during
printing. A compact shortest-decimal printer is a separate future improvement.

```rbel definition=float-printing
(def f-take (xs slots)
  (if slots (join (car xs) (f-take (cdr xs) (cdr slots))) nil))
(def f-drop (xs slots)
  (if slots (f-drop (cdr xs) (cdr slots)) xs))
(def f-binary-magnitude (bits value)
  (if bits
    (f-binary-magnitude (cdr bits)
      ((fn (twice) (if (id (car bits) \1) (n-add twice "1") twice))
       (n-add value value))) value))
(def f-scale-two (value count)
  (if (n-equal count "0") value
    (if (n-less count "16")
      (f-scale-two (n-add value value) (n-sub count "1"))
      (f-scale-two (f-double-sixteen value) (n-sub count "16")))))
(def f-recover (sign exponent fraction)
  ((fn (effective magnitude)
     (if (n-equal magnitude "0") (n-raw '+ "0" "1")
       (if (n-less effective "1075")
         (n-build sign magnitude (f-scale-two "1" (n-sub "1075" effective)))
         (n-build sign (f-scale-two magnitude (n-sub effective "1075")) "1"))))
   (if (n-equal exponent "0") "1" exponent)
   (f-binary-magnitude (if (n-equal exponent "0") fraction (join \1 fraction)) "0")))
(def f-exact (value)
  ((fn (bits)
     (f-recover (if (id (car bits) \1) '- '+)
       (f-binary-magnitude (f-take (cdr bits) (f-zero-exponent)) "0")
       (f-drop (cdr bits) (f-zero-exponent))))
   (car (cdr (cdr value)))))
(def exact (value)
  (if (exact-number value) value
    (if (float-number value) (f-exact value) (err 'expected-number))))
(def float-text (value)
  (if (float-number value)
    ((fn (recovered)
       (if (n-equal (n-numerator recovered) "0")
         (if (id (car (car (cdr (cdr value)))) \1) "-0f" "0f")
         (n-append (exact-text recovered) "f")))
     (f-exact value))
    (err 'expected-float)))
(def number-text (value)
  (if (float-number value) (float-text value) (exact-text value)))
```

## Examples

```rbel example=float-explicit-conversion
(= (float 1/2) 0.5f)
=>
t
```

```rbel example=float-approximation-visible
(= (+ 0.1f 0.2f) 0.3f)
=>
nil
```

```rbel example=float-exact-recovery
(exact 0.5f)
=>
1/2
```

```rbel example=float-roundtrip-text
(float-text 0.5f)
=>
"1/2f"
```

```rbel example=float-negative-zero-text
(float-text -0f)
=>
"-0f"
```
