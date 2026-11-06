# Find oversized files and rbel blocks

This module owns the line-limit decisions for the local `rbel lint` command.
It is independent of libcwr's protocol engine and example coworkers. A warning
asks an author to inspect a boundary; length alone does not establish a defect.

The command adapter reads a file, identifies rbel fences using the existing
Markdown fence rules, and supplies the resulting physical-line counts. Blank lines and
comments count. A final unterminated line counts once; a trailing newline does
not create another line. CRLF represents one line ending. Whole-file counts
include fences and prose. Block counts include only content between the fences.

The request is a proper list `(file-count file-limit block-limit blocks)`.
Counts and limits are canonical nonnegative decimal character lists: `"0"`,
`"80"`, or `"500"`, for example. There are no leading zeroes except in `"0"`. Each block is
`(location count)`, with an opaque location supplied by the adapter. The adapter
uses a text index to look up the file's opening-fence line and optional name.
The adapter validates input and option bounds before calling this trusted ABI.

The result is a list of findings in source order, beginning with a whole-file
finding when appropriate. A file finding is `(file count limit)`. A block
finding is `(block location count limit)`. The adapter formats counts and
locations for the reader. No finding means an empty list. A count exactly equal
to its limit produces no warning.

## Compare a count with its limit

Compare the number of decimal digits first. A longer canonical decimal value
is greater. When widths match, compare corresponding digits from left to right,
using their order in `"0123456789"`. Equal values do not exceed the limit.
This uses recursion proportional to decimal width rather than the line count;
a 131072-line file does not require 131072 stack frames or unary markers.

```rbel definition=lint-line-comparison
(def rl-over-limit (count limit)
  ((fn (width)
     (if (id width 'longer) t
       (id width 'shorter) nil
       (rl-digits-over count limit)))
    (rl-width-order count limit)))
(def rl-width-order (count limit)
  (if count
    (if limit (rl-width-order (cdr count) (cdr limit)) 'longer)
    (if limit 'shorter 'same)))
(def rl-digits-over (count limit)
  (if count
    (if (id (car count) (car limit))
      (rl-digits-over (cdr count) (cdr limit))
      (rl-digit-over (car count) (car limit) "0123456789"))
    nil))
(def rl-digit-over (count limit digits)
  (if (id (car digits) count) nil
    (id (car digits) limit) t
    (rl-digit-over count limit (cdr digits))))
```

```rbel example=lint-zero-count
(rl-over-limit "0" "2")
=>
nil
```

```rbel example=lint-exact-limit
(rl-over-limit "80" "80")
=>
nil
```

```rbel example=lint-one-over-limit
(rl-over-limit "81" "80")
=>
t
```

```rbel example=lint-below-limit
(rl-over-limit "79" "80")
=>
nil
```

```rbel example=lint-zero-limit
(rl-over-limit "1" "0")
=>
t
```

```rbel example=lint-longer-count
(rl-over-limit "100" "99")
=>
t
```

```rbel example=lint-shorter-count
(rl-over-limit "99" "100")
=>
nil
```

```rbel example=lint-large-count
(rl-over-limit "131072" "131071")
=>
t
```

```rbel example=lint-large-custom-limit
(rl-over-limit "131072" "2147483647")
=>
nil
```

## Select findings

`rl-lint` takes the file count, two independent budgets, and the blocks. Each
block is inspected using the block budget. Keeping the original count and limit
in the finding lets the adapter report the actual count without duplicating the
threshold decision. The locations and order are preserved exactly as supplied.

```rbel definition=lint-findings
(def rl-lint (request)
  ((fn (count file-limit block-limit blocks)
     ((fn (findings)
        (if (rl-over-limit count file-limit)
          (join (join 'file (join count (join file-limit nil))) findings)
          findings))
       (rl-block-findings blocks block-limit)))
    (car request) (car (cdr request)) (car (cdr (cdr request)))
    (car (cdr (cdr (cdr request))))))
(def rl-block-findings (blocks limit)
  (if blocks
    (if (rl-over-limit (car (cdr (car blocks))) limit)
      (join (join 'block
              (join (car (car blocks))
                (join (car (cdr (car blocks))) (join limit nil))))
        (rl-block-findings (cdr blocks) limit))
      (rl-block-findings (cdr blocks) limit))
    nil))
```

```rbel example=lint-empty-file
(rl-lint '("0" "2" "1" nil))
=>
nil
```

```rbel example=lint-file-at-limit
(rl-lint '("2" "2" "1" nil))
=>
nil
```

```rbel example=lint-file-warning
(rl-lint '("3" "2" "1" nil))
=>
((file "3" "2"))
```

```rbel example=lint-block-at-limit
(rl-lint '("2" "3" "2" (("first" "2"))))
=>
nil
```

```rbel example=lint-block-warning
(rl-lint '("2" "3" "1" (("first" "2"))))
=>
((block "first" "2" "1"))
```

```rbel example=lint-combined-warnings
(rl-lint '("3" "2" "1" (("first" "2") ("second" "1") ("third" "3"))))
=>
((file "3" "2") (block "first" "2" "1") (block "third" "3" "1"))
```

```rbel example=lint-independent-custom-limits
(rl-lint '("3" "3" "2" (("first" "3") ("second" "0"))))
=>
((block "first" "3" "2"))
```

```rbel example=lint-zero-budgets
(rl-lint '("1" "0" "0" (("empty" "0") ("one" "1"))))
=>
((file "1" "0") (block "one" "1" "0"))
```

These compiled examples test the actual lint rules with small, readable budgets.
The command-level fixtures separately check default limits of 500 and 80,
physical-line transport, fence recognition, error handling, and formatted source
locations against real files.
