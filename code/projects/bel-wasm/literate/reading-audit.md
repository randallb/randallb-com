# Foundation reading audit

Reviewed by Assistant, November 5, 2026. Scope: the revised
[foundation chapter](foundation.rbel.md), its worked traces, and its executable
examples. This is an editorial prerequisite audit, not a novice user study.

## Prerequisite order

| First introduced | Concepts and notation | Used afterward |
| --- | --- | --- |
| Opening and section 1 | program, language, value, expression, evaluation, result, nil, t, example separator, character/backslash | All example boxes distinguish input from displayed output. |
| Section 2 | symbol, quotation/apostrophe, lookup, binding, error versus nil | Symbols used as data are quoted; parameter lookup comes later. |
| Section 3 | operation, function, call, argument, grouping parentheses, ordinary evaluation order, id, nested calls, long quote form | Pair construction and later function bodies use these rules. |
| Section 4 | pair, head, tail, join, dotted pair, car, cdr, pair identity | Lists and later traversal use these operations. |
| Section 5 | proper list, elements, nil termination, list display, whole-structure quotation, nested/dotted lists, empty car/cdr behavior | Later functions explicitly receive proper lists. |
| Section 6 | type, symbol/char/pair answers, nil's type | atom uses type and id. |
| Section 7 | condition, truth beyond t, if, special form, fallback, err, short-circuiting | all and some use multiple conditions and skipped branches. |
| Section 8 | fn, parameter, body, binding on call, functions as inputs, def | All five definitions can be read using prior syntax and operations. |
| Section 9 | definition layout, comments, predicate, no | atom negates a pair test; all/some/reduce test emptiness. |
| Section 10 | atom, nested evaluation trace | Predicates used in traversal examples. |
| Section 11 | recursion, base case, shrinking tail, xs as ordinary parameter name, all | Later self-calls and stopping behavior. |
| Section 12 | suffix, some, return of existing suffix | Identity example distinguishes shared suffix from a copy. |
| Section 13 | reduce, pending combination, dotted result, right fold | Final summary connects the five behaviors. |

The opening names the five functions as the destination without relying on their
meanings. Early examples use only supplied starting operations and syntax, not
unexplained library functions. The checker installs the five definitions before
every example; this audit separately checks the pedagogical dependency order.

## Worked traces and executable evidence

- Ordinary nested call: `nested-call` checks inner-result use by the outer call.
- Parameter binding and quotation: `identity-function`, `parameter-versus-symbol`,
  `two-parameters`, and `function-input` exercise the operations described in the
  function walkthrough. `same-pair` checks identity after one construction.
- no: `empty`, `symbol`, `pair`, and `nonempty-containing-nil` check its body and
  distinguish an empty list from a list containing nil.
- atom: `atom-symbol-step` and `atom-pair-step` check intermediate identity tests;
  atom examples check final results for nil, symbols, characters, and pairs.
- all: `all-trace-two`, `all-trace-tail`, and `all-empty` check successive recursive
  inputs. `all-mixed` checks failure; `all-stops` detects evaluation past a failure.
- some: `some-match`, `some-trace-tail`, and `some-empty` check the described search
  and base case. `some-none` checks exhaustion; `some-stops` checks skipped work;
  `same-suffix` checks that the existing tail is returned.
- reduce: `reduce-right`, `reduce-trace-tail`, `reduce-one`, and
  `reduce-trace-combine` check the recursive inputs and combination result.
  `reduce-empty` checks the car/cdr nil boundary. `reduce-skips-function` checks
  that the singleton base case never calls its function argument.

Trace prose was checked against the definitions and the evaluation order. The
traces are explanatory walkthroughs, not captured debugger traces. The code
examples verify their results and selected intermediate expressions.

## Verification result

- All 67 executable chapter examples passed on the current host.
- All four extraction/source-drift tests passed.
- The concatenated definition bytes still equal the pinned foundation reference;
  the foundation and complete 31-fragment snapshot hashes remain unchanged.
- Chapter, bootstrap guide, audit, README, and planning links were checked locally.
- Rust runtime and extraction behavior were not changed by this revision. The
  full 86-test Rust suite was not rerun; its previous pass remains historical.

## Remaining reader evidence

No reader with no programming experience has yet completed a comprehension
review of this revision. A useful next review is to read it in order, mark the
first unexplained term, and explain a fresh expression using the stated rules.
Actual feedback may require changes to pace, examples, or prerequisite order.

Follow-up: Randall subsequently read the chapter and said it “looks great.”
That is recorded reader feedback, not a claim that he represents a novice.
The native integration reran all 67 examples and five extraction checks, plus
89 release-mode Rust tests. The original definitions and their hashes remain
unchanged; see the separate native chapter for the first-order compiler boundary.
