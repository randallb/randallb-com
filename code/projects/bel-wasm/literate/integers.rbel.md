# Exact numbers

Whole numbers and fractions are exact. Their magnitudes use decimal digits rather
than one list cell per unit, so there is no fixed numeric bound such as 256.
Available memory and execution resources still limit calculations. Zero is true;
only `nil` is false.

Load this chapter with `--include`. `+`, `-`, `*`, and `/` take two numbers;
`=`, `<`, `<=`, `>`, and `>=` compare them. Division by zero is an error.
`integer` recognizes exact whole numbers and `exact-number` recognizes exact
rationals. `integer-text` and `exact-text` return text that the reader can read
back. Nonintegral results print as reduced fractions.

The representation is `(lit num sign numerator denominator)`. The numerator and
denominator are canonical, big-endian decimal character lists. Denominators are
positive and nonzero; zero has positive sign and denominator one. Constructors
reduce fractions by their greatest common divisor. These definitions contain no
numeric literals, so a previous compiler can bootstrap the new reader.

## Decimal magnitude operations

```rbel definition=number-magnitudes
(def n-reverse (xs out)
  (if xs (n-reverse (cdr xs) (join (car xs) out)) out))
(def n-append (a b)
  (if a (join (car a) (n-append (cdr a) b)) b))
(def n-trim (x)
  (if x (if (id (car x) \0) (n-trim (cdr x)) x) "0"))
(def n-equal (a b)
  (if a (if b (if (id (car a) (car b)) (n-equal (cdr a) (cdr b)) nil) nil)
    (if b nil t)))
(def n-digit-less (a b ds)
  (if (id a (car ds)) (if (id a b) nil t)
    (if (id b (car ds)) nil (n-digit-less a b (cdr ds)))))
(def n-lex-less (a b)
  (if a (if (id (car a) (car b)) (n-lex-less (cdr a) (cdr b))
    (n-digit-less (car a) (car b) "0123456789")) nil))
(def n-shorter (a b)
  (if a (if b (n-shorter (cdr a) (cdr b)) nil) (if b t nil)))
(def n-less (a b)
  (if (n-shorter a b) t (if (n-shorter b a) nil (n-lex-less a b))))
(def n-after-digit (d digits)
  (if (id d (car digits)) (cdr digits) (n-after-digit d (cdr digits))))
(def n-digit-unary (d digits)
  (if (id d (car digits)) nil (join t (n-digit-unary d (cdr digits)))))
(def n-units (d) (n-digit-unary d "0123456789"))
(def n-digit (units digits)
  (if units (n-digit (cdr units) (cdr digits)) (car digits)))
(def n-add-digit-loop (a units carry)
  (if units
    (if (id a \9) (n-add-digit-loop \0 (cdr units) t)
      (n-add-digit-loop (car (n-after-digit a "0123456789")) (cdr units) carry))
    (join a carry)))
(def n-add-digit (a b carry)
  (n-add-digit-loop a (if carry (join t (n-units b)) (n-units b)) nil))
(def n-add-reversed (a b carry out)
  (if a
    (n-add-reversed-step (cdr a) (if b (cdr b) nil) out
      (n-add-digit (car a) (if b (car b) \0) carry))
    (if b
      (n-add-reversed-step nil (cdr b) out (n-add-digit \0 (car b) carry))
      (if carry (join \1 out) out))))
(def n-add-reversed-step (a b out digit)
  (n-add-reversed a b (cdr digit) (join (car digit) out)))
(def n-add (a b)
  (n-add-reversed (n-reverse a nil) (n-reverse b nil) nil nil))
(def n-before-digit (d digits)
  (if (id d (car (cdr digits))) (car digits) (n-before-digit d (cdr digits))))
(def n-sub-digit-loop (a units borrow)
  (if units
    (if (id a \0) (n-sub-digit-loop \9 (cdr units) t)
      (n-sub-digit-loop (n-before-digit a "0123456789") (cdr units) borrow))
    (join a borrow)))
(def n-sub-digit (a b borrow)
  (n-sub-digit-loop a (if borrow (join t (n-units b)) (n-units b)) nil))
(def n-sub-reversed (a b borrow out)
  (if a
    (n-sub-reversed-step (cdr a) (if b (cdr b) nil) out
      (n-sub-digit (car a) (if b (car b) \0) borrow))
    (if borrow (err 'negative-magnitude) (n-trim out))))
(def n-sub-reversed-step (a b out digit)
  (n-sub-reversed a b (cdr digit) (join (car digit) out)))
(def n-sub (a b)
  (if (n-less a b) (err 'negative-magnitude)
    (n-sub-reversed (n-reverse a nil) (n-reverse b nil) nil nil)))
(def n-mul-digit-loop (a units out)
  (if units (n-mul-digit-loop a (cdr units) (n-add a out)) out))
(def n-mul-loop (a b out)
  (if b (n-mul-loop a (cdr b)
    (n-add (n-trim (n-append out "0")) (n-mul-digit-loop a (n-units (car b)) "0"))) out))
(def n-mul (a b) (n-mul-loop a b "0"))
(def n-div-digit (remainder divisor digit)
  (if (n-less remainder divisor) (join digit remainder)
    (n-div-digit (n-sub remainder divisor) divisor
      (car (n-after-digit digit "0123456789")))))
(def n-div-loop (digits divisor quotient remainder)
  (if digits
    (n-div-step (cdr digits) divisor quotient
      (n-div-digit (n-trim (n-append remainder (join (car digits) nil))) divisor \0))
    (join (n-trim (n-reverse quotient nil)) (join remainder nil))))
(def n-div-step (digits divisor quotient result)
  (n-div-loop digits divisor (join (car result) quotient) (cdr result)))
(def n-divmod (a b)
  (if (n-equal b "0") (err 'division-by-zero) (n-div-loop a b nil "0")))
(def n-quotient (a b) (car (n-divmod a b)))
(def n-remainder (a b) (car (cdr (n-divmod a b))))
(def n-gcd (a b)
  (if (n-equal b "0") a (n-gcd b (n-remainder a b))))
```

## Canonical values and predicates

```rbel definition=number-values
(def n-one (x) (if (id (type x) 'pair) (if (cdr x) nil t) nil))
(def n-two (x) (if (id (type x) 'pair) (n-one (cdr x)) nil))
(def n-three (x) (if (id (type x) 'pair) (n-two (cdr x)) nil))
(def n-four (x) (if (id (type x) 'pair) (n-three (cdr x)) nil))
(def n-five (x) (if (id (type x) 'pair) (n-four (cdr x)) nil))
(def n-second (x) (car (cdr x)))
(def n-third (x) (car (cdr (cdr x))))
(def n-fourth (x) (car (cdr (cdr (cdr x)))))
(def n-fifth (x) (car (cdr (cdr (cdr (cdr x))))))
(def n-sign (x) (n-third x))
(def n-numerator (x) (n-fourth x))
(def n-denominator (x) (n-fifth x))
(def n-digit-p (c digits)
  (if digits (if (id c (car digits)) t (n-digit-p c (cdr digits))) nil))
(def n-digits (xs)
  (if xs (if (id (type xs) 'pair)
    (if (n-digit-p (car xs) "0123456789") (n-digits (cdr xs)) nil) nil) t))
(def n-magnitude-p (xs)
  (if (id (type xs) 'pair)
    (if (n-digits xs)
      (if (id (car xs) \0) (if (cdr xs) nil t) t) nil) nil))
(def n-valid-ratio (x)
  (if (n-magnitude-p (n-numerator x))
    (if (n-magnitude-p (n-denominator x))
      (if (n-equal (n-denominator x) "0") nil
        (if (n-equal (n-numerator x) "0")
          (if (id (n-sign x) '+) (n-equal (n-denominator x) "1") nil)
          (n-equal (n-gcd (n-numerator x) (n-denominator x)) "1"))) nil) nil))
(def exact-number (x)
  (if (n-five x) (if (id (car x) 'lit) (if (id (n-second x) 'num)
    (if (if (id (n-sign x) '+) t (id (n-sign x) '-)) (n-valid-ratio x) nil) nil) nil) nil))
(def integer (x)
  (if (exact-number x) (n-equal (n-denominator x) "1") nil))
(def n-raw (sign numerator denominator)
  (join 'lit (join 'num (join sign (join numerator (join denominator nil))))))
(def n-reduced (sign numerator denominator gcd)
  (n-raw sign (n-quotient numerator gcd) (n-quotient denominator gcd)))
(def n-build-trimmed (sign numerator denominator)
  (if (n-equal denominator "0") (err 'division-by-zero)
    (if (n-equal numerator "0") (n-raw '+ "0" "1")
      (n-reduced sign numerator denominator (n-gcd numerator denominator)))))
(def n-build (sign numerator denominator)
  (if (if (id sign '+) t (id sign '-))
    (if (if numerator (n-digits numerator) nil)
      (if (if denominator (n-digits denominator) nil)
        (n-build-trimmed sign (n-trim numerator) (n-trim denominator))
        (err 'expected-decimal-denominator)) (err 'expected-decimal-numerator))
    (err 'expected-number-sign)))
(def n-require (x) (if (exact-number x) x (err 'expected-exact-number)))
(def n-negative (x)
  (n-raw (if (n-equal (n-numerator x) "0") '+
    (if (id (n-sign x) '+) '- '+)) (n-numerator x) (n-denominator x)))
```

## Exact arithmetic

Equal signs add magnitudes; opposite signs subtract the smaller magnitude from
the larger. Fractions use a common denominator. Products multiply numerators and
denominators; division multiplies by the reciprocal.

```rbel definition=number-arithmetic
(def n-signed-sum (sign-a a sign-b b denominator)
  (if (id sign-a sign-b) (n-build sign-a (n-add a b) denominator)
    (if (n-less a b) (n-build sign-b (n-sub b a) denominator)
      (n-build sign-a (n-sub a b) denominator))))
(def n-exact-add (a b)
  (n-signed-sum (n-sign a) (n-mul (n-numerator a) (n-denominator b))
    (n-sign b) (n-mul (n-numerator b) (n-denominator a))
    (n-mul (n-denominator a) (n-denominator b))))
(def n-product-sign (a b) (if (id (n-sign a) (n-sign b)) '+ '-))
(def n-exact-mul (a b)
  (n-build (n-product-sign a b) (n-mul (n-numerator a) (n-numerator b))
    (n-mul (n-denominator a) (n-denominator b))))
(def n-exact-div (a b)
  (n-build (n-product-sign a b) (n-mul (n-numerator a) (n-denominator b))
    (n-mul (n-denominator a) (n-numerator b))))
(def n-exact-equal (a b)
  (if (id (n-sign a) (n-sign b))
    (if (n-equal (n-numerator a) (n-numerator b))
      (n-equal (n-denominator a) (n-denominator b)) nil) nil))
(def n-exact-less (a b)
  (if (id (n-sign a) (n-sign b))
    (if (id (n-sign a) '-)
      (n-less (n-mul (n-numerator b) (n-denominator a))
        (n-mul (n-numerator a) (n-denominator b)))
      (n-less (n-mul (n-numerator a) (n-denominator b))
        (n-mul (n-numerator b) (n-denominator a))))
    (id (n-sign a) '-)))
```

## Choosing exact or floating arithmetic

Both operands must have the same exactness. Explicit conversion is required
before mixing exact and floating values. This initial floating interface accepts
only finite values; NaN, infinities, and arithmetic overflow are errors.

```rbel definition=number-dispatch
(def n-float-bits (xs budget)
  (if budget
    (if (id (type xs) 'pair)
      (if (if (id (car xs) \0) t (id (car xs) \1))
        (n-float-bits (cdr xs) (cdr budget)) nil) nil)
    (id xs nil)))
(def n-all-ones (xs slots)
  (if slots
    (if (id (car xs) \1) (n-all-ones (cdr xs) (cdr slots)) nil) t))
(def n-finite-bits (bits)
  (if (n-float-bits bits
      "1111111111111111111111111111111111111111111111111111111111111111")
    (if (n-all-ones (cdr bits) "11111111111") nil t) nil))
(def float-number (x)
  (if (n-three x) (if (id (car x) 'lit) (if (id (n-second x) 'float)
    (n-finite-bits (n-third x)) nil) nil) nil))
(def number (x) (if (exact-number x) t (float-number x)))
(def n-both-exact (a b) (if (exact-number a) (exact-number b) nil))
(def n-both-float (a b) (if (float-number a) (float-number b) nil))
(def + (a b)
  (if (n-both-exact a b) (n-exact-add a b)
    (if (n-both-float a b) (fp-add a b) (err 'expected-same-exactness))))
(def - (a b)
  (if (n-both-exact a b) (n-exact-add a (n-negative b))
    (if (n-both-float a b) (fp-sub a b) (err 'expected-same-exactness))))
(def * (a b)
  (if (n-both-exact a b) (n-exact-mul a b)
    (if (n-both-float a b) (fp-mul a b) (err 'expected-same-exactness))))
(def / (a b)
  (if (n-both-exact a b) (n-exact-div a b)
    (if (n-both-float a b) (fp-div a b) (err 'expected-same-exactness))))
(def = (a b)
  (if (n-both-exact a b) (n-exact-equal a b)
    (if (n-both-float a b) (fp-eq a b) (err 'expected-same-exactness))))
(def < (a b)
  (if (n-both-exact a b) (n-exact-less a b)
    (if (n-both-float a b) (fp-lt a b) (err 'expected-same-exactness))))
(def <= (a b) (if (< a b) t (= a b)))
(def > (a b) (< b a))
(def >= (a b) (if (> a b) t (= a b)))
```

## Decimal and fraction text

```rbel definition=number-text
(def n-signed-text (sign digits)
  (if (id sign '-) (join \- digits) digits))
(def n-exact-text (x)
  (n-signed-text (n-sign x)
    (if (n-equal (n-denominator x) "1") (n-numerator x)
      (n-append (n-numerator x) (join \/ (n-denominator x))))))
(def exact-text (x) (n-exact-text (n-require x)))
(def integer-text (x)
  (if (integer x) (n-signed-text (n-sign x) (n-numerator x))
    (err 'expected-integer)))
```

## Executable examples

```rbel example=large-integer-carry
(+ 999999999999999999999 1)
=>
1000000000000000000000
```

```rbel example=large-integer-borrow
(- 1000000000000000000000 1)
=>
999999999999999999999
```

```rbel example=integer-product
(* 123456789 987654321)
=>
121932631112635269
```

```rbel example=exact-decimal-sum
(+ 0.1 0.2)
=>
0.3
```

```rbel example=exact-fraction-sum
(+ 1/3 1/6)
=>
1/2
```

```rbel example=exact-fraction-product
(* -2/3 9/10)
=>
-3/5
```

```rbel example=exact-division
(/ 5 2)
=>
5/2
```

```rbel example=integer-quotient-recognition
(integer (/ 1000000000000000000000 10))
=>
t
```

```rbel example=fraction-not-integer
(integer 1/3)
=>
nil
```

```rbel example=exact-zero-truth
(if 0 'yes 'no)
=>
yes
```

```rbel example=negative-fraction-order
(< -1/3 -1/4)
=>
t
```

```rbel example=reduced-fraction-text
(exact-text (+ 1/3 1/6))
=>
"1/2"
```

```rbel example=large-integer-text
(integer-text (+ 999999999999999999999 1))
=>
"1000000000000000000000"
```
