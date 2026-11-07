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
(def rn-read-all (source) (rn-many (rn-skip source)))
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

## Symbols and integer notation

A token collects characters until a boundary. Symbol names use ASCII letters,
hyphens, and digits. The arithmetic and comparison names `+`, `-`, `=`, `<`,
`<=`, `>`, and `>=` are also symbols. The `sym` operation turns a character
list into a symbol, giving the same symbol for the same name.

A decimal integer can have a leading `+` or `-`. Leading zeroes are allowed;
`0`, `-0`, and `+000` all produce the same positive zero. A number is read as
Bel's ordinary `(lit num real imaginary)` data, with a signed unary numerator,
a denominator of one, and a zero imaginary part. Reading a number does not
calculate with it, and quotation still quotes the number itself.

This first integer implementation accepts magnitudes through 256. The reader
checks the decimal spelling before allocating the unary value. Larger integers,
fractions, decimal points, exponent notation, and numeric tokens with trailing
letters fail rather than becoming symbols. Bracket functions, backquote,
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
    (if (rn-text-equal xs "=") t
      (if (rn-text-equal xs "<") t
        (if (rn-text-equal xs "<=") t
          (if (rn-text-equal xs ">") t (rn-text-equal xs ">=")))))))
(def rn-numeric-start (xs)
  (if (rn-member (car xs) "0123456789") t
    (if (rn-member (car xs) '(\+ \-))
      (rn-member (car (cdr xs)) "0123456789") nil)))
(def rn-symbol (parsed)
  (if (car parsed)
    (if (rn-numeric-start (car parsed))
      (join (rn-integer (car parsed)) (cdr parsed))
      (if (rn-operator (car parsed))
        (join (sym (car parsed)) (cdr parsed))
        (if (rn-legal-token (car parsed))
          (join (sym (car parsed)) (cdr parsed)) (rn-fail)))) (rn-fail)))
```

The parser validates the digits, removes leading zeroes, and compares the
remaining decimal text with the limit. Only then does it build the number.
The magnitude is a list of `t` values: for example, two is `(t t)`. A decimal
step makes ten copies of the previous magnitude and adds the next digit.

```rbel definition=rn-integer
(def rn-integer (token)
  (if (id (car token) \-)
    (rn-integer-digits (sym "-") (cdr token))
    (if (id (car token) \+)
      (rn-integer-digits (sym "+") (cdr token))
      (rn-integer-digits (sym "+") token))))
(def rn-integer-digits (sign digits)
  (if digits
    (if (rn-digits digits)
      (rn-integer-bounded sign (rn-trim-zeroes digits)) (rn-fail)) (rn-fail)))
(def rn-digits (xs)
  (if xs (if (rn-member (car xs) "0123456789")
    (rn-digits (cdr xs)) nil) t))
(def rn-trim-zeroes (xs)
  (if xs (if (id (car xs) \0) (rn-trim-zeroes (cdr xs)) xs) nil))
(def rn-integer-bounded (sign digits)
  (if (rn-decimal-within digits "256")
    (rn-integer-value sign (rn-decimal-unary digits nil)) (rn-fail)))
(def rn-decimal-within (a b)
  (rn-decimal-width a b a b))
(def rn-decimal-width (a b original-a original-b)
  (if a
    (if b (rn-decimal-width (cdr a) (cdr b) original-a original-b) nil)
    (if b t (rn-decimal-order original-a original-b))))
(def rn-decimal-order (a b)
  (if a
    (if (id (car a) (car b)) (rn-decimal-order (cdr a) (cdr b))
      (rn-digit-before (car a) (car b) "0123456789")) t))
(def rn-digit-before (a b digits)
  (if digits
    (if (id a (car digits)) t
      (if (id b (car digits)) nil (rn-digit-before a b (cdr digits)))) nil))
(def rn-decimal-unary (digits value)
  (if digits
    (rn-decimal-unary (cdr digits)
      (rn-unary-copies '(t t t t t t t t t t) value
        (rn-digit-unary (car digits) "0123456789"))) value))
(def rn-unary-copies (copies value tail)
  (if copies
    (rn-unary-append value (rn-unary-copies (cdr copies) value tail)) tail))
(def rn-unary-append (a b)
  (if a
    (if (cdr a)
      (if (cdr (cdr a))
        (if (cdr (cdr (cdr a)))
          (join t (join t (join t (join t
            (rn-unary-append (cdr (cdr (cdr (cdr a)))) b)))))
          (join t (join t (join t b))))
        (join t (join t b)))
      (join t b)) b))
(def rn-digit-unary (digit remaining)
  (if (id digit (car remaining)) nil
    (join t (rn-digit-unary digit (cdr remaining)))))
(def rn-integer-value (sign magnitude)
  (join 'lit (join 'num
    (join
      (join (if magnitude sign (sym "+"))
        (join magnitude (join '(t) nil)))
      (join (join (sym "+") '(nil (t))) nil)))))
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
must contain exactly one character. Text equality here compares character lists
by their contents rather than pair identity.

```rbel definition=rn-text-equal
(def rn-text-equal (a b)
  (if a (if b (if (id (car a) (car b)) (rn-text-equal (cdr a) (cdr b)) nil) nil)
    (if b nil t)))
(def rn-character (xs)
  (if xs (if (rn-letter (car xs)) (rn-named-character (rn-token xs))
    (if (rn-white (car xs)) (rn-fail) (join (car xs) (cdr xs)))) (rn-fail)))
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
(car (cdr (car (cdr (cdr (car (rn-read-all "+002")))))))
=>
(t t)
```

```rbel example=reader-integer-negative
(id (car (car (cdr (cdr (car (rn-read-all "-3")))))) (sym "-"))
=>
t
```

```rbel example=reader-integer-zero-normalized
(id (car (car (cdr (cdr (car (rn-read-all "-000")))))) (sym "+"))
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

```rbel example=reader-decimal-limit
(rn-decimal-within "256" "256")
=>
t
```

```rbel example=reader-decimal-over-limit
(rn-decimal-within "257" "256")
=>
nil
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
