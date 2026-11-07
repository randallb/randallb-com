# Signed integers

Integers count whole amounts, including zero and negative amounts. A die face
can be `14`, a bonus can be `2`, and a penalty can be `-3`. An integer evaluates
to itself. Only `nil` means false; the integer `0` still means true.

Load this file with `--include` to use the arithmetic functions. `+` and `-` each take two
integers. `=` compares their numeric values; `<`, `<=`, `>`, and `>=` compare
their order. `integer` tests whether a value is a supported integer.
`integer-text` returns decimal text for display.

The first implementation supports magnitudes through 256. Operations reject
other kinds of input and results outside that range. Fractions, decimals, and
complex numbers are outside this chapter. Numbers use Bel's existing literal
pairs, so they can travel inside records without changing the value transport.

## Recognizing integer values

The reader builds `(lit num real imaginary)`. A real component contains a sign,
a numerator represented by a list of `t` values, and the denominator `(t)`.
The imaginary component is positive zero. Zero always has a positive sign.
These details are checked before arithmetic uses an input.

```rbel definition=integer-values
(def int-one (x)
  (if (id (type x) 'pair) (if (cdr x) nil t) nil))
(def int-two (x)
  (if (id (type x) 'pair) (int-one (cdr x)) nil))
(def int-three (x)
  (if (id (type x) 'pair) (int-two (cdr x)) nil))
(def int-four (x)
  (if (id (type x) 'pair) (int-three (cdr x)) nil))
(def int-second (x) (car (cdr x)))
(def int-third (x) (car (cdr (cdr x))))
(def int-fourth (x) (car (cdr (cdr (cdr x)))))
(def int-copy-onto (a b)
  (if a (join t (int-copy-onto (cdr a) b)) b))
(def int-double (x) (int-copy-onto x x))
(def int-limit ()
  (int-double (int-double (int-double (int-double
    (int-double (int-double (int-double (int-double '(t))))))))))
(def int-unary (x budget)
  (if x
    (if budget (if (id (type x) 'pair)
      (if (id (car x) t) (int-unary (cdr x) (cdr budget)) nil) nil) nil) t))
(def int-denominator (x) (if (int-one x) (id (car x) t) nil))
(def int-zero-component (x)
  (if (int-three x)
    (if (id (car x) '+)
      (if (id (int-second x) nil) (int-denominator (int-third x)) nil) nil) nil))
(def int-real-component (x budget)
  (if (int-three x)
    (if (int-denominator (int-third x))
      (if (id (car x) '+) (int-unary (int-second x) budget)
        (if (id (car x) '-)
          (if (int-second x) (int-unary (int-second x) budget) nil) nil)) nil) nil))
(def integer (x)
  (if (int-four x)
    (if (id (car x) 'lit)
      (if (id (int-second x) 'num)
        (if (int-zero-component (int-fourth x))
          (int-real-component (int-third x) (int-limit)) nil) nil) nil) nil))
(def int-require (x)
  (if (integer x) x (err 'expected-bounded-integer)))
(def int-sign (x) (car (int-third x)))
(def int-magnitude (x) (int-second (int-third x)))
(def int-build (sign magnitude)
  (if (int-unary magnitude (int-limit))
    (join 'lit (join 'num
      (join (join (if magnitude sign '+)
        (join magnitude (join '(t) nil)))
        (join '(+ nil (t)) nil))))
    (err 'integer-result-out-of-range)))
```

## Addition and subtraction

For equal signs, addition combines the magnitudes. For opposite signs it
cancels one unit from each magnitude until one is empty. Subtraction reverses
the second sign and then adds.

```rbel definition=integer-addition
(def int-cancel (a b sign-a sign-b)
  (if a (if b (int-cancel (cdr a) (cdr b) sign-a sign-b)
    (int-build sign-a a)) (int-build sign-b b)))
(def int-add (a b)
  (if (id (int-sign a) (int-sign b))
    (int-build (int-sign a)
      (int-copy-onto (int-magnitude a) (int-magnitude b)))
    (int-cancel (int-magnitude a) (int-magnitude b) (int-sign a) (int-sign b))))
(def + (a b) (int-add (int-require a) (int-require b)))
(def int-negative (x)
  (int-build (if (id (int-sign x) '+) '- '+) (int-magnitude x)))
(def - (a b) (int-add (int-require a) (int-negative (int-require b))))
```

## Equality and ordering

`id` continues to compare pair identity. Use `=` for integer equality, including
numbers constructed by separate calculations.

```rbel definition=integer-comparison
(def int-same-magnitude (a b)
  (if a (if b (int-same-magnitude (cdr a) (cdr b)) nil) (if b nil t)))
(def int-smaller-magnitude (a b)
  (if a (if b (int-smaller-magnitude (cdr a) (cdr b)) nil) (if b t nil)))
(def int-equal (a b)
  (if (id (int-sign a) (int-sign b))
    (int-same-magnitude (int-magnitude a) (int-magnitude b)) nil))
(def = (a b) (int-equal (int-require a) (int-require b)))
(def int-less (a b)
  (if (id (int-sign a) (int-sign b))
    (if (id (int-sign a) '-)
      (int-smaller-magnitude (int-magnitude b) (int-magnitude a))
      (int-smaller-magnitude (int-magnitude a) (int-magnitude b)))
    (id (int-sign a) '-)))
(def < (a b) (int-less (int-require a) (int-require b)))
(def <= (a b) (if (< b a) nil t))
(def > (a b) (< b a))
(def >= (a b) (if (< a b) nil t))
```

## Decimal text

The printer starts at zero and advances one decimal digit at a time for each
unit in the magnitude. Its working digits are reversed so a carry touches the
least significant digit first.

```rbel definition=integer-display
(def int-reverse (xs result)
  (if xs (int-reverse (cdr xs) (join (car xs) result)) result))
(def int-next-digit (c)
  (if (id c \0) \1 (if (id c \1) \2 (if (id c \2) \3
    (if (id c \3) \4 (if (id c \4) \5 (if (id c \5) \6
      (if (id c \6) \7 (if (id c \7) \8 \9)))))))))
(def int-increment-digits (digits)
  (if digits
    (if (id (car digits) \9)
      (join \0 (int-increment-digits (cdr digits)))
      (join (int-next-digit (car digits)) (cdr digits))) '(\1)))
(def int-magnitude-digits (magnitude digits)
  (if magnitude
    (int-magnitude-digits (cdr magnitude) (int-increment-digits digits))
    (int-reverse digits nil)))
(def int-display (x)
  ((fn (digits) (if (id (int-sign x) '-) (join \- digits) digits))
   (int-magnitude-digits (int-magnitude x) '(\0))))
(def integer-text (x) (int-display (int-require x)))
```

## Examples

```rbel example=integer-dice-total
(+ 14 2)
=>
16
```

```rbel example=integer-dice-success
(>= (+ 14 2) 10)
=>
t
```

```rbel example=integer-negative-result
(+ 2 -3)
=>
-1
```

```rbel example=integer-subtraction
(- -3 -5)
=>
2
```

```rbel example=integer-zero-truth
(if 0 'yes 'no)
=>
yes
```

```rbel example=integer-value-equality
(= (+ 1 1) 2)
=>
t
```

```rbel example=integer-rejects-text-as-number
(integer "2")
=>
nil
```

```rbel example=integer-display-negative
(integer-text (- 2 16))
=>
"-14"
```

```rbel example=integer-record-values
'((face 14) (bonus -2))
=>
((face 14) (bonus -2))
```
