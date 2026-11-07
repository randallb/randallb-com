# A reader written in rbel

The [foundation](foundation.rbel.md) explains values and evaluation. Before an
expression can be evaluated or compiled, its written characters must be read
into values. This chapter implements that reader in rbel. Its language rules
are independent of Rust; the [native command guide](../experiments/bel-rust-repl/wasm/native-command.md)
explains the bootstrap and command that runs it.

## Reading text as values

The reader takes a proper list of characters and returns a proper list of
all expressions in that text. Reading constructs values; it never evaluates
those expressions. For example, reading a written call to err produces a list
that describes that call. It does not report that error.

Within the reader, a single parsed result is a pair: its head is the value just
read, and its tail is the unused input characters. Keeping the unused input lets
the next routine continue exactly where the previous one stopped. A separate
failure operation stops malformed or unsupported input.

```rbel definition=rn-read-all
(def rn-read-all (source) (rn-many (rn-skip (rn-check-source source))))
(def rn-check-source (source)
  (if (rn-source-valid source) source (err 'invalid-source-control)))
(def rn-source-valid (source)
  (if source
    (if (id (type source) 'pair)
      (if (id (type (car source)) 'char)
        (if (n-less (n-numerator (char-code (car source))) "32")
          (if (rn-member (car source) '(\tab \lf \cr))
            (rn-source-valid (cdr source)) nil)
          (rn-source-valid (cdr source))) nil) nil) t))
(def rn-fail () (err 'invalid-or-unsupported-reader-input))
```

## Finding boundaries

Spaces, tabs, line feeds, and carriage returns separate expressions. A
semicolon starts a comment that ends at a line feed or the end of input. The
skip routine advances past these characters without creating expressions.
Delimiter characters end a symbol token. These routines inspect characters,
not the meanings of any eventual expressions.

```rbel definition=rn-member
(def rn-member (x xs)
  (if xs (if (id x (car xs)) t (rn-member x (cdr xs))) nil))
(def rn-white (c) (rn-member c '(\sp \tab \lf \cr)))
(def rn-letter (c)
  (rn-member c "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ"))
(def rn-delimiter (c)
  (if (rn-white c) t (rn-member c '(\( \) \' \" \; \\))))
(def rn-skip (xs)
  (if xs
    (if (rn-white (car xs)) (rn-skip (cdr xs))
      (if (id (car xs) \;) (rn-skip (rn-comment (cdr xs))) xs)) nil))
(def rn-comment (xs)
  (if xs (if (id (car xs) \lf) (cdr xs) (rn-comment (cdr xs))) nil))
```

## Reading expressions in order

The many routine reads one expression, saves its value, and then reads
the remaining expressions. The dispatcher chooses a rule from the first
non-whitespace character: opening parenthesis, quotation mark, apostrophe,
backslash, or a symbol token. An unexpected closing parenthesis is an error.
An apostrophe constructs the two-element list whose first element is the symbol
quote and whose second is the value that follows.

```rbel definition=rn-many
(def rn-many (xs)
  (if xs (rn-many-after (rn-one xs)) nil))
(def rn-many-after (parsed)
  (join (car parsed) (rn-many (rn-skip (cdr parsed)))))
(def rn-one (input)
  (rn-dispatch (rn-skip input)))
(def rn-dispatch (xs)
  (if xs
    (if (id (car xs) \() (rn-list (cdr xs))
      (if (id (car xs) \)) (rn-fail)
        (if (id (car xs) \') (rn-quoted (rn-one (cdr xs)))
          (if (id (car xs) \") (rn-string (cdr xs))
            (if (id (car xs) \\) (rn-character (cdr xs))
              (rn-symbol (rn-token xs))))))) (rn-fail)))
(def rn-quoted (parsed)
  (join (join 'quote (join (car parsed) nil)) (cdr parsed)))
```

## Proper and dotted lists

An opening parenthesis starts list reading. A closing parenthesis ends
it. Each ordinary element is read with the same one-expression routine used at
the top level. A separate dot after an element means that the next value is the
final tail; after that value, a closing parenthesis is required. A leading dot,
a missing tail, or extra values after a dotted tail is an error.

```rbel definition=rn-list
(def rn-list (input)
  (rn-list-start (rn-skip input)))
(def rn-list-start (xs)
  (if xs
    (if (id (car xs) \)) (join nil (cdr xs))
      (rn-list-head (rn-one xs))) (rn-fail)))
(def rn-list-head (head)
  (rn-list-tail (car head) (rn-skip (cdr head))))
(def rn-dot (xs)
  (if (id (car xs) \.)
    (if (cdr xs) (rn-delimiter (car (cdr xs))) t) nil))
(def rn-list-tail (head xs)
  (if (rn-dot xs)
    (rn-dotted-tail head (rn-one (cdr xs)))
    (rn-list-join head (rn-list xs))))
(def rn-list-join (head tail)
  (join (join head (car tail)) (cdr tail)))
(def rn-dotted-tail (head parsed)
  (rn-dotted-close head (car parsed) (rn-skip (cdr parsed))))
(def rn-dotted-close (head tail xs)
  (if (id (car xs) \)) (join (join head tail) (cdr xs)) (rn-fail)))
```

## Symbols and numeric notation

A token collects characters until a boundary. Symbol names use ASCII letters,
hyphens, and digits. The arithmetic and comparison names `+`, `-`, `*`, `/`, `=`,
`<`, `<=`, `>`, and `>=` are also symbols. The `sym` operation turns a character
list into a symbol, giving the same symbol for the same name. A qualified
reference such as `words/finish`, `math/+`, or `math//` is one symbol token; module linking
resolves it when it appears as a reference. In `math//`, the first slash is
the namespace separator and the second is the division operator. Quoting it retains ordinary symbol
data. Numeric-looking tokens are read first, so `1/3` remains a fraction.

Numeric literals are exact unless an `f` suffix requests an approximate binary64
float. Integers have no fixed magnitude limit. Fractions such as `1/3`, decimals
such as `0.1`, and exponent notation such as `1.25e3` all produce exact values.
Decimal notation requires digits before and after its point. An exponent may
have a leading sign. A denominator must be a positive integer, and zero
denominators fail. A leading sign applies to the complete value.

Leading zeroes are allowed. Exact zero always has positive sign and denominator
one. Equivalent exact fractions are reduced to the same numerator and denominator.
The [numeric library](integers.rbel.md) owns that normalization; the reader calls
it rather than defining a second arithmetic implementation. An exact value has
five fields: `(lit num sign numerator denominator)`, where the last two fields
are lists of decimal digit characters. Floats use `(lit float bits)`, with 64
binary digit characters containing their IEEE 754 representation.

`0.1f` requests a binary64 approximation to the exact decimal `0.1`. The reader
first constructs the exact value and then calls the explicit float conversion,
so literal conversion and program conversion obey the same rounding rule. A
float literal also retains its written sign when its value rounds to zero.
Reading a number does not execute an authored expression, and quotation still
quotes the number itself.

Source, memory, and stack budgets still apply. This reader additionally bounds
decimal digit expansion to 4096 characters, including exponent expansion; that
is a resource budget, rather than a maximum numeric magnitude. Malformed numeric
tokens fail instead of silently becoming symbols. Bracket functions, backquote,
unquote, and other unsupported abbreviations remain errors.

```rbel definition=rn-token
(def rn-token (xs)
  (if xs (if (rn-delimiter (car xs)) (join nil xs)
    (rn-token-join (car xs) (rn-token (cdr xs)))) (join nil nil)))
(def rn-token-join (head parsed)
  (join (join head (car parsed)) (cdr parsed)))
(def rn-legal-token (xs)
  (if xs
    (if (rn-member (car xs) "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ-0123456789")
      (rn-legal-token (cdr xs)) nil) t))
(def rn-operator (xs)
  (if (rn-text-equal xs "+") t
    (if (rn-text-equal xs "*") t
      (if (rn-text-equal xs "/") t
        (if (rn-text-equal xs "=") t
          (if (rn-text-equal xs "<") t
            (if (rn-text-equal xs "<=") t
              (if (rn-text-equal xs ">") t (rn-text-equal xs ">=")))))))))
(def rn-qualified-token (xs)
  (rn-qualified-left xs nil))
(def rn-qualified-left (xs left)
  (if xs
    (if (id (car xs) \/)
      (if left
        (if (cdr xs)
          (if (rn-text-equal (cdr xs) "/") t
            (if (rn-member \/ (cdr xs)) nil
              (if (rn-operator (cdr xs)) t (rn-legal-token (cdr xs))))) nil) nil)
      (if (rn-member (car xs) "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ-0123456789")
        (rn-qualified-left (cdr xs) t) nil)) nil))
(def rn-numeric-start (xs)
  (if (rn-member (car xs) "0123456789") t
    (if (rn-member (car xs) '(\+ \-))
      (rn-member (car (cdr xs)) "0123456789") nil)))
(def rn-symbol (parsed)
  (if (car parsed)
    (if (rn-numeric-start (car parsed))
      (join (rn-number (car parsed)) (cdr parsed))
      (if (rn-operator (car parsed))
        (join (sym (car parsed)) (cdr parsed))
        (if (if (rn-qualified-token (car parsed)) t (rn-legal-token (car parsed)))
          (join (sym (car parsed)) (cdr parsed)) (rn-fail)))) (rn-fail)))
```

The parser separates the leading sign, whole digits, fractional digits, and
exponent. It keeps digit characters throughout; it never allocates one list
entry per unit of magnitude. A decimal point contributes a power-of-ten
denominator. A positive exponent scales the numerator, and a negative exponent
scales the denominator. The numeric library then reduces the result.

```rbel definition=rn-number
(def rn-reverse (xs tail)
  (if xs (rn-reverse (cdr xs) (join (car xs) tail)) tail))
(def rn-append (a b)
  (if a (join (car a) (rn-append (cdr a) b)) b))
(def rn-number (token)
  (rn-number-suffix token (rn-reverse token nil)))
(def rn-number-suffix (token reversed)
  (if (id (car reversed) \f)
    (if (cdr reversed)
      (f-literal (if (id (car token) \-) (sym "-") (sym "+"))
        (rn-exact (rn-reverse (cdr reversed) nil))) (rn-fail))
    (rn-exact token)))
(def rn-exact (token)
  (if (id (car token) \-)
    (rn-exact-sign (sym "-") (cdr token))
    (if (id (car token) \+)
      (rn-exact-sign (sym "+") (cdr token))
      (rn-exact-sign (sym "+") token))))
(def rn-digit (c) (rn-member c "0123456789"))
(def rn-digits (xs)
  (if xs (if (rn-digit (car xs)) (rn-digits (cdr xs)) nil) t))
(def rn-take-digits (xs)
  (if xs (if (rn-digit (car xs))
    (rn-token-join (car xs) (rn-take-digits (cdr xs)))
    (join nil xs)) (join nil nil)))
(def rn-exact-sign (sign token)
  (rn-exact-whole sign (rn-take-digits token)))
(def rn-exact-whole (sign parsed)
  (if (car parsed)
    (rn-after-whole sign (car parsed) (cdr parsed)) (rn-fail)))
(def rn-after-whole (sign whole tail)
  (if tail
    (if (id (car tail) \/) (rn-fraction sign whole (cdr tail))
      (if (id (car tail) \.)
        (rn-fractional-digits sign whole (rn-take-digits (cdr tail)))
        (rn-exponent sign whole "1" tail)))
    (rn-build sign whole "1")))
(def rn-fraction (sign whole denominator)
  (if denominator
    (if (rn-digits denominator) (rn-build sign whole denominator) (rn-fail))
    (rn-fail)))
(def rn-fractional-digits (sign whole parsed)
  (if (car parsed)
    (rn-exponent sign (rn-append whole (car parsed))
      (join \1 (rn-digit-zeroes (car parsed))) (cdr parsed)) (rn-fail)))
(def rn-digit-zeroes (digits)
  (if digits (join \0 (rn-digit-zeroes (cdr digits))) nil))
(def rn-exponent (sign numerator denominator tail)
  (if tail
    (if (rn-member (car tail) '(\e \E))
      (rn-exponent-sign sign numerator denominator (cdr tail)) (rn-fail))
    (rn-build sign numerator denominator)))
(def rn-exponent-sign (sign numerator denominator exponent)
  (if (id (car exponent) \-)
    (rn-exponent-digits sign numerator denominator t (cdr exponent))
    (if (id (car exponent) \+)
      (rn-exponent-digits sign numerator denominator nil (cdr exponent))
      (rn-exponent-digits sign numerator denominator nil exponent))))
(def rn-exponent-digits (sign numerator denominator negative exponent)
  (if exponent
    (if (rn-digits exponent)
      (if (n-less "4096" (n-trim exponent)) (rn-resource-fail)
        (rn-exponent-scale sign numerator denominator negative
          (rn-exponent-zeroes (n-trim exponent) nil))) (rn-fail))
    (rn-fail)))
(def rn-exponent-zeroes (digits result)
  (if digits
    (rn-exponent-zeroes (cdr digits)
      (rn-ten-copies '(t t t t t t t t t t) result
        (rn-small-zeroes (car digits) "0123456789"))) result))
(def rn-small-zeroes (digit remaining)
  (if (id digit (car remaining)) nil
    (join \0 (rn-small-zeroes digit (cdr remaining)))))
(def rn-ten-copies (copies value tail)
  (if copies (rn-copy value (rn-ten-copies (cdr copies) value tail)) tail))
(def rn-exponent-scale (sign numerator denominator negative zeroes)
  (if negative
    (rn-build sign numerator (rn-append denominator zeroes))
    (rn-build sign (rn-append numerator zeroes) denominator)))
(def rn-copy (a b)
  (if a (join (car a) (rn-copy (cdr a) b)) b))
(def rn-budget () '(t t t t t t t t t t t t t t t t))
(def rn-resource-fail () (err 'numeric-reader-resource-limit))
(def rn-drop-sixteen (digits count)
  (if digits (if count (rn-drop-sixteen (cdr digits) (cdr count)) digits) nil))
(def rn-drop-block (digits count)
  (if digits
    (if count
      (rn-drop-block (rn-drop-sixteen digits (rn-budget)) (cdr count)) digits) nil))
(def rn-bounded (digits budget)
  (if digits
    (if budget
      (rn-bounded (rn-drop-block digits (rn-budget)) (cdr budget)) (rn-resource-fail)) t))
(def rn-build (sign numerator denominator)
  (if (rn-bounded numerator (rn-budget))
    (if (rn-bounded denominator (rn-budget))
      (n-build sign numerator denominator) (rn-resource-fail)) (rn-resource-fail)))
```

## Strings

Double quotation marks surround a string, which is a list of characters.
A backslash takes the next character literally. Thus an escaped quotation mark
is part of the string rather than its end. This matches the existing reader's
escape rule: backslash-n means the letter n, not a line feed. An actual line
feed may appear inside a string; an unescaped tab or carriage return is rejected.
A missing closing quote or an unfinished escape is an error.

```rbel definition=rn-string
(def rn-string (xs)
  (if xs
    (if (id (car xs) \") (join nil (cdr xs))
      (if (id (car xs) \\)
        (if (cdr xs) (rn-token-join (car (cdr xs)) (rn-string (cdr (cdr xs)))) (rn-fail))
        (if (rn-member (car xs) '(\tab \cr)) (rn-fail)
          (rn-token-join (car xs) (rn-string (cdr xs)))))) (rn-fail)))
```

## Character notation

A backslash outside a string starts a character. A punctuation character
is taken directly; ASCII alphabetic text is read as one character name. The names sp,
tab, lf, and cr mean space, tab, line feed, and carriage return. Any other name
must contain exactly one character. An actual non-ASCII scalar is one character
as well: `\é` and `\🦉` are supported. Explicit scalar notation `\u{HEX}` accepts
one through six hexadecimal digits and rejects surrogate values and values above
U+10FFFF. It can express controls, including `\u{0}`. This escape applies outside
strings; inside a string the existing backslash rule still only quotes the next
character. Text equality here compares character lists
by their contents rather than pair identity.

```rbel definition=rn-text-equal
(def rn-text-equal (a b)
  (if a (if b (if (id (car a) (car b)) (rn-text-equal (cdr a) (cdr b)) nil) nil)
    (if b nil t)))
(def rn-character (xs)
  (if xs
    (if (if (id (car xs) \u) (id (car (cdr xs)) \{) nil)
      (rn-scalar-escape (cdr (cdr xs)) "0" nil '(t t t t t t))
      (if (rn-letter (car xs)) (rn-named-character (rn-token xs))
        (if (rn-white (car xs)) (rn-fail) (join (car xs) (cdr xs))))) (rn-fail)))
(def rn-hex-value (c chars values)
  (if chars
    (if (id c (car chars)) (car values) (rn-hex-value c (cdr chars) (cdr values))) nil))
(def rn-scalar-escape (xs magnitude seen budget)
  (if xs
    (if (id (car xs) \})
      (if seen (join (code-char (n-build '+ magnitude "1")) (cdr xs)) (rn-fail))
      (if budget
        ((fn (digit)
           (if digit
             (rn-scalar-escape (cdr xs) (n-add (n-mul magnitude "16") digit) t (cdr budget))
             (rn-fail)))
         (rn-hex-value (car xs) "0123456789abcdefABCDEF"
           '("0" "1" "2" "3" "4" "5" "6" "7" "8" "9" "10" "11" "12" "13" "14" "15" "10" "11" "12" "13" "14" "15")))
        (rn-fail))) (rn-fail)))
(def rn-named-character (parsed)
  (if (rn-text-equal (car parsed) "sp") (join \sp (cdr parsed))
    (if (rn-text-equal (car parsed) "tab") (join \tab (cdr parsed))
      (if (rn-text-equal (car parsed) "lf") (join \lf (cdr parsed))
        (if (rn-text-equal (car parsed) "cr") (join \cr (cdr parsed))
          (if (cdr (car parsed)) (rn-fail) (join (car (car parsed)) (cdr parsed))))))))
```

## Executable examples

Each input below is a string containing source text. The expected answer is
the list of read values, not the result of evaluating that source.

```rbel example=reader-empty
(rn-read-all "")
=>
nil
```

```rbel example=reader-symbols
(rn-read-all "nil t seed")
=>
(nil t seed)
```

```rbel example=reader-quote
(rn-read-all "'seed")
=>
((quote seed))
```

```rbel example=reader-list
(rn-read-all "(a b)")
=>
((a b))
```

```rbel example=reader-dotted
(rn-read-all "(a . b)")
=>
((a . b))
```

```rbel example=reader-comment
(rn-read-all "; ignored
(a (b) nil)")
=>
((a (b) nil))
```

```rbel example=reader-chars
(rn-read-all "\\a \\tab")
=>
(\a \tab)
```

```rbel example=reader-string
(rn-read-all "\"hello\"")
=>
("hello")
```

```rbel example=reader-escaped-quote
(rn-read-all "\"\\\"\"")
=>
("\"")
```

```rbel example=reader-backslash
(rn-read-all "\\\\")
=>
(\\)
```

```rbel example=reader-dot-character
(rn-read-all "(\\. . seed)")
=>
((\. . seed))
```

```rbel example=reader-no-evaluation
(rn-read-all "(err 'not-executed)")
=>
((err (quote not-executed)))
```

```rbel example=reader-integer-magnitude
(car (cdr (cdr (cdr (car (rn-read-all "+002"))))))
=>
"2"
```

```rbel example=reader-integer-negative
(id (car (cdr (cdr (car (rn-read-all "-3"))))) (sym "-"))
=>
t
```

```rbel example=reader-integer-zero-normalized
(id (car (cdr (cdr (car (rn-read-all "-000"))))) (sym "+"))
=>
t
```

```rbel example=reader-integer-quoted
(car (car (rn-read-all "'14")))
=>
quote
```

```rbel example=reader-arithmetic-operators
(rn-text-equal (nom (car (rn-read-all ">="))) ">=")
=>
t
```

```rbel example=reader-large-integer
(car (rn-read-all "123456789012345678901234567890"))
=>
123456789012345678901234567890
```

```rbel example=reader-fraction-reduced
(car (rn-read-all "12/18"))
=>
2/3
```

```rbel example=reader-decimal-exact
(car (rn-read-all "0.125"))
=>
1/8
```

```rbel example=reader-positive-exponent
(car (rn-read-all "1.25e3"))
=>
1250
```

```rbel example=reader-negative-exponent
(car (rn-read-all "-1.25E-2"))
=>
-1/80
```

```rbel example=reader-explicit-float
(car (rn-read-all "0.5f"))
=>
0.5f
```

```rbel example=reader-qualified-symbol
(rn-text-equal (nom (car (rn-read-all "math/+"))) "math/+")
=>
t
```

```rbel example=reader-scalar-escape
(char-code (car (rn-read-all "\\u{1F989}")))
=>
129417
```

```rbel example=reader-scalar-nul
(char-code (car (rn-read-all "\\u{0}")))
=>
0
```

```rbel example=reader-raw-source-control-rejected
(try (rn-read-all (join (code-char 0) nil)) (catch problem problem))
=>
invalid-source-control
```

## Scope and verification

This is a reader for the current compiled subset, not every notation supported
by the larger Bel reference. The native command bounds input and stack work;
exceeding a limit fails the operation. Malformed-input and cross-reader tests
are recorded in the native command guide. Reader definitions, explanation, and
examples are maintained together here. Generated extracted source is build output.

Written by Assistant for Randall Bennett for the native compiler-command
milestone. The original foundation definitions and source pins are unchanged.

With the native rbel command available, these examples run through compiled code:

```sh
rbel check literate/reader.rbel.md
```
