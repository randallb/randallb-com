# Foundation: learning to read and follow rbel

A program is a set of written instructions. A programming language gives those
instructions precise meanings. **rbel**, pronounced “rebel,” is the language we
will use here. You do not need to know another programming language.

Our goal is to understand five small reusable instructions: `no`, `atom`, `all`,
`some`, and `reduce`. These names will gain meaning as we go. We will first learn
the pieces they use, then follow their work one step at a time.

## 1. Instructions and answers

A **value** is something a program can work with: for example a name used as data,
a character, or a collection of other values. An **expression** is written text
that rbel can interpret to produce a value. **Evaluation** means following the
language’s rules to find that value. The resulting value is the **result**.

The word `nil` is our first expression. Its value is `nil`. Later we will use
that value to mean an empty collection or a false answer.

Each example box below has three parts: an expression to evaluate, a line
containing `=>`, and the result we expect. The arrow belongs to the explanation;
it is not part of the expression. The result is displayed as data, not evaluated
again as another instruction.

```rbel example=nil-value
nil
=>
nil
```

The word `t` is another expression whose value is itself. We use it for a
true answer. These two words have built-in meanings; an arbitrary word does not
automatically evaluate to itself.

```rbel example=true-value
t
=>
t
```

A **character** is one written mark, such as the letter a. In rbel, `\a`
means the character a. The backslash introduces character notation. This is
different from a name spelled `a`.

```rbel example=character-value
\a
=>
\a
```

## 2. A name as data, and a name to look up

A **symbol** is a named value, such as `seed`. A symbol can serve as a name to
look up, or it can be data in its own right. These uses are different.

Writing `'seed` means “use the symbol seed as data.” The apostrophe is called
**quotation**: it asks rbel to take the following piece of written structure
without evaluating it. The result displays as `seed`, without the apostrophe.

```rbel example=quoted-symbol
'seed
=>
seed
```

Writing bare `seed` instead asks rbel to look up the value associated with
that name. An association between a name and a value is a **binding**. If there
is no binding, evaluation reports an error. An error means evaluation could not
produce a normal answer. It is different from returning `nil`.

`nil` and `t` are special: they evaluate to themselves. Most other symbols are
looked up. We will soon give names values inside functions.

## 3. Calling an operation

An **operation** takes values and does something with them. A **function** is an
operation we can call with inputs to obtain a result. Calling means asking it to
perform its work. The inputs supplied in a call are called **arguments**.

Parentheses group a call. In `(id 'seed 'seed)`, `id` names the function and the
two quoted symbols provide its two arguments. Spaces separate the pieces.

`id` is a starting operation: it answers `t` when its two inputs are identical,
and `nil` otherwise. For symbols, identical means the same name. For characters,
it means the same character. We will explain identity for collections shortly.

For an ordinary function call, rbel finds the function, evaluates the argument
expressions from left to right, and then calls the function with those values.
Here each quoted expression produces the symbol seed, so `id` receives the same
symbol twice.

```rbel example=same-symbol
(id 'seed 'seed)
=>
t
```

```rbel example=different-symbols
(id 'seed 'leaf)
=>
nil
```

```rbel example=different-characters
(id \a \b)
=>
nil
```

Parentheses can be nested: one call can supply an argument to another. To
find the result of `(id (id 'seed 'leaf) nil)`, first find the inner call’s result:
`(id 'seed 'leaf)` produces `nil`. The outer call therefore receives `nil` and
`nil`, which are identical.

```rbel example=nested-call
(id (id 'seed 'leaf) nil)
=>
t
```

Quotation also has a long spelling: `(quote seed)` means the same as `'seed`.
`quote` has its own rule: return its following written structure as data instead
of evaluating that structure. We will call rules of this kind special forms
when we reach conditionals.

```rbel example=long-quotation
(quote seed)
=>
seed
```

## 4. Pairs: putting two values together

A **pair** is a value with two positions. Its first position is called its
**head**; its second is its **tail**. Either position can contain any value,
including another pair.

`join` constructs a new pair from two arguments. rbel displays a pair containing
the symbols seed and leaf as `(seed . leaf)`. The dot separates the head from
the tail. Here the parentheses describe data in the result; they are not a
request to call seed.

```rbel example=make-pair
(join 'seed 'leaf)
=>
(seed . leaf)
```

`car` returns the head of a pair. `cdr` returns its tail. These historical
names are simply the operation names to remember: `car` selects the first
position and `cdr` selects the second. Neither changes the pair.

```rbel example=pair-head
(car (join 'seed 'leaf))
=>
seed
```

```rbel example=pair-tail
(cdr (join 'seed 'leaf))
=>
leaf
```

The inner `join` runs before `car` or `cdr` receives its argument. A pair’s
tail need not be a collection: the symbol leaf is a valid tail.

Two separately constructed pairs are distinct objects, even when their positions
contain identical values. For pairs, `id` tests whether they are the very same
pair, not whether their contents look alike.

```rbel example=distinct-pairs
(id (join 'seed 'leaf) (join 'seed 'leaf))
=>
nil
```

## 5. Lists: pairs ending in nil

A **list**, in the ordinary form used here, is either `nil` or a pair whose tail
is another list. It is also called a **proper list**. Following its tails
eventually reaches `nil`. Its **elements** are the heads encountered along the
way, in order. Thus `nil` is the empty list: it has no elements.

`(join 'seed nil)` is a one-element list. Its fully dotted spelling is
`(seed . nil)`, but rbel displays it more compactly as `(seed)`.

```rbel example=one-element-list
(join 'seed nil)
=>
(seed)
```

We can make a two-element list by using a one-element list as a new pair’s
tail. Read this construction from the inner call outward: make `(leaf)`, then
put seed before it.

```rbel example=two-element-list
(join 'seed (join 'leaf nil))
=>
(seed leaf)
```

The same list can be written as quoted data: `'(seed leaf)`. The apostrophe
protects the entire following structure, so neither seed nor leaf is looked up.
Without that apostrophe, `(seed leaf)` would be interpreted as a call.

```rbel example=quoted-list
'(seed leaf)
=>
(seed leaf)
```

```rbel example=list-head
(car '(seed leaf))
=>
seed
```

```rbel example=list-tail
(cdr '(seed leaf))
=>
(leaf)
```

```rbel example=last-tail
(cdr '(leaf))
=>
nil
```

A list can contain a list as an element. In `'((seed) leaf)`, the first
element is the one-element list `(seed)`, and the second is the symbol leaf.

```rbel example=nested-list-head
(car '((seed) leaf))
=>
(seed)
```

A **dotted list** has a final tail other than `nil`. `(seed leaf . twig)`
means `(seed . (leaf . twig))`. It contains pairs, but is not a proper list.
The traversing functions later in this chapter are explained for proper lists.

There is a useful boundary rule: `(car nil)` and `(cdr nil)` both return `nil`.
An empty list has no actual pair positions; this is the behavior these operations
define for it. Giving either operation a different non-pair value, such as the
symbol seed, is an error.

```rbel example=empty-head
(car nil)
=>
nil
```

```rbel example=empty-tail
(cdr nil)
=>
nil
```

## 6. Asking what kind of value we have

A value’s **type** describes its kind. The starting operation `type` returns a
symbol naming that kind. For the values introduced here, the answers are
`symbol`, `char`, and `pair`.

```rbel example=symbol-type
(type 'seed)
=>
symbol
```

```rbel example=nil-type
(type nil)
=>
symbol
```

```rbel example=character-type
(type \a)
=>
char
```

```rbel example=pair-type
(type '(seed))
=>
pair
```

So `nil` serves as the empty list but has type symbol, not pair. A nonempty
list has type pair because its outermost value is a pair. `type` describes that
outermost value, not every value inside it.

## 7. Choosing what to evaluate

A **condition** is an expression whose result decides what happens next. Only
`nil` counts as false. Every other value counts as true, including a symbol, a
character, or a pair. `t` is the conventional true answer, not the only true value.

`if` makes a choice. In `(if condition yes no)`, first evaluate condition. If its
result is true, evaluate yes and return that result. Otherwise evaluate no and
return its result. The unchosen expression is not evaluated.

This is a **special form**: a piece of syntax with its own evaluation rule.
Unlike an ordinary call, it does not evaluate all its arguments before choosing.

```rbel example=choose-true
(if t 'yes 'no)
=>
yes
```

```rbel example=choose-false
(if nil 'yes 'no)
=>
no
```

```rbel example=non-t-truth
(if 'seed 'yes 'no)
=>
yes
```

rbel’s `if` can also try several condition/result pairs, from left to right.
The first true condition selects its corresponding result. A final expression
without a matching result is the fallback. If nothing matches and there is no
fallback, the result is `nil`.

```rbel example=choose-later
(if nil 'first t 'second 'fallback)
=>
second
```

```rbel example=choose-fallback
(if nil 'first nil 'second 'fallback)
=>
fallback
```

```rbel example=no-fallback
(if nil 'first)
=>
nil
```

The starting operation `err` reports an error using its argument as the
message, rather than returning an ordinary value. We can use it to show that an
unchosen branch really is skipped: the following expression succeeds because
rbel never evaluates the expression containing `err`. Stopping evaluation as
soon as the answer is decided is called **short-circuiting**.

```rbel example=skip-error
(if t 'kept (err 'should-not-run))
=>
kept
```

## 8. Making a function and giving its inputs names

`fn` creates a function. In `(fn (x) x)`, `(x)` is the **parameter list**:
it names the input x. The final `x` is the **body**, the expression to evaluate
when the function is called. These positions are syntax, so rbel does not try to
call x or look up the body’s x while creating the function.

A **parameter** is a name used inside a function. An **argument** is the value
supplied by a caller. When this function is called, its parameter x is bound
to the supplied argument. Evaluating the body x then looks up that value.

A call need not begin with a simple name: it can begin with an expression that
produces a function. The following expression creates a function, then calls it
with the symbol seed.

```rbel example=identity-function
((fn (x) x) 'seed)
=>
seed
```

Follow the evaluation in order:

1. `(fn (x) x)` produces a function, keeping its body for later.
2. `'seed` produces the symbol seed.
3. The call gives x the value seed inside the function.
4. Evaluating the body x looks up that value and returns seed.

Quotation still means data inside a function. A body `'x` returns the symbol x
itself, regardless of what input the parameter x received.

```rbel example=parameter-versus-symbol
((fn (x) 'x) 'seed)
=>
x
```

Functions can take several inputs. The parameter names receive argument
values in the same order. The examples here supply exactly the declared inputs.

```rbel example=two-parameters
((fn (head tail) (join head tail)) 'seed 'leaf)
=>
(seed . leaf)
```

A function can receive another function as an input. In the next example,
f receives the function named car, x receives a list, and `(f x)` calls that
function on that list. Bare car means “look up the function”; `'car` would mean
the symbol car as data.

```rbel example=function-input
((fn (f x) (f x)) car '(seed leaf))
=>
seed
```

We can now check pair identity without constructing two pairs. The next
expression constructs one pair, binds p to it, and uses p twice. Both inputs to
`id` refer to that same pair.

```rbel example=same-pair
((fn (p) (id p p)) (join 'seed 'leaf))
=>
t
```

`def` creates a named function. The structure `(def name (inputs) body)`
associates name with a function having those parameters and body. Later calls
use the name to find it. Like `fn`, this is a special rule: the parameter list
and body are not evaluated as ordinary arguments when defining the function.

We now have the starting vocabulary for the five definitions. `join`, `car`,
`cdr`, `id`, `type`, and `err` are supplied operations. `quote` (usually written
with the apostrophe), `if`, `fn`, and `def` supply the syntax rules we have used.
These rules say what rbel does; they do not depend on which software runs it.

## 9. no: testing for absence

A **definition** gives a reusable name to behavior. Here is our first one.
Lines beginning with a semicolon are **comments**: explanatory text ignored
during evaluation. The comment below preserves the original definition’s source
header. Line breaks and indentation make the structure easier to read; the
parentheses determine the grouping.

```rbel definition=no
; Bel in Bel. 9 October 2019, 9:14 GMT


(def no (x)
  (id x nil))

```

Read it as: “Define no with one parameter x. When called, test whether x is
identical to nil.” It is a **predicate**, a function used to test a condition.

For `(no nil)`: bind x to `nil`, evaluate `(id x nil)` with those bindings,
and obtain `t`. For `(no 'a)`: quotation supplies the symbol a, bind x to a,
and `(id x nil)` returns `nil`. The letter a is just a sample symbol.

```rbel example=empty
(no nil)
=>
t
```

```rbel example=symbol
(no 'a)
=>
nil
```

```rbel example=pair
(no '(a))
=>
nil
```

A nonempty list is a pair, so it is different from `nil`. `no` tests absence;
it does not ask whether a list’s elements themselves contain empty values.

```rbel example=nonempty-containing-nil
(no '(nil))
=>
nil
```

## 10. atom: testing for a value that is not a pair

An **atom** is a value that is not a pair. Symbols and characters are atoms.
The empty list is an atom too, because its value is `nil`, whose type is symbol.

```rbel definition=atom
(def atom (x)
  (no (id (type x) 'pair)))

```

The body contains nested calls. For `(atom 'a)`:

1. Bind x to the symbol a.
2. `(type x)` returns the symbol symbol.
3. `(id (type x) 'pair)` compares symbol with pair and returns `nil`.
4. `(no nil)` returns `t`.

For `(atom '(a b))`, the type is pair, so the identity test returns `t`.
The final `(no t)` returns `nil`. In this definition, no reverses the answer to
“is it a pair?” We can check the two intermediate comparisons directly.

```rbel example=atom-symbol-step
(id (type 'a) 'pair)
=>
nil
```

```rbel example=atom-pair-step
(id (type '(a b)) 'pair)
=>
t
```

```rbel example=nil-atom
(atom nil)
=>
t
```

```rbel example=symbol-atom
(atom 'a)
=>
t
```

```rbel example=character-atom
(atom \a)
=>
t
```

```rbel example=list-pair
(atom '(a b))
=>
nil
```

```rbel example=dotted-pair
(atom '(a . b))
=>
nil
```

## 11. all: testing every element

To process a list, we can deal with its head, then use the same operation on
its tail. When a function calls itself, that is **recursion**. A **base case**
is an input that produces an answer without another self-call. Here, progressively
shorter proper lists eventually reach the empty list.

`all` receives a predicate f and a proper list xs. The name xs is just a
parameter name, chosen to suggest several values. It is not special syntax.
`all` answers `t` if every element passes the predicate and `nil` otherwise.

```rbel definition=all
(def all (f xs)
  (if (no xs)      t
      (f (car xs)) (all f (cdr xs))
                   nil))

```

This `if` has two condition/result pairs and a fallback:

| Condition to evaluate | Result expression if true |
| --- | --- |
| `(no xs)` | `t` |
| `(f (car xs))` | `(all f (cdr xs))` |

The fallback is `nil`. The empty list is the base case: there are no elements
left that could fail the test. Otherwise test the head. If it passes, keep the
same predicate f and test the tail. If it fails, return `nil` immediately.

Follow `(all atom '(a b))`:

1. xs is `(a b)`, so `(no xs)` is `nil`.
2. Its head a is an atom, so the second condition is true.
3. Its tail is `(b)`. Continue with `(all atom '(b))`.
4. b passes too. Continue with `(all atom nil)`.
5. The empty-list condition is true, so return `t`. Each waiting call returns
   that result to its caller.

No answers need to be combined on the way back: each successful head simply
passes along the answer from the remaining tail.

```rbel example=all-trace-two
(all atom '(a b))
=>
t
```

```rbel example=all-trace-tail
(all atom '(b))
=>
t
```

```rbel example=all-empty
(all atom nil)
=>
t
```

```rbel example=all-atoms
(all atom '(a b c))
=>
t
```

```rbel example=all-mixed
(all atom '(a (b) c))
=>
nil
```

For `(all atom '(a (b) c))`, a passes, then the element `(b)` fails because
it is a pair. all returns `nil` without testing c. The next example makes that
stopping behavior visible. Its predicate returns `nil` for the symbol stop and
would report an error for anything else. The symbol later is never tested.

```rbel example=all-stops
(all (fn (x) (if (id x 'stop) nil (err 'visited))) '(stop later))
=>
nil
```

## 12. some: finding a matching suffix

A **suffix** is the remaining part of a list from some point onward. The
suffixes of `(a b c)` are `(a b c)`, `(b c)`, `(c)`, and `nil`.

`some` receives a predicate f and a proper list xs. It finds the first element
that passes the predicate and returns the suffix beginning there. If no element
passes, it returns `nil`. A successful result is the remaining list, not just t.

```rbel definition=some
(def some (f xs)
  (if (no xs)      nil
      (f (car xs)) xs
                   (some f (cdr xs))))

```

Its first condition handles the empty list: no match remains, so return
`nil`. Otherwise test the head. If the head passes, return xs itself. If it does
not, call some on the tail.

Follow `(some atom '((a) b c))`:

1. xs is `((a) b c)`, so it is not empty.
2. The head is `(a)`, a pair. atom returns `nil` for it.
3. Continue with `(some atom '(b c))`.
4. That list is not empty. Its head b is an atom.
5. Return the current xs, `(b c)`. The earlier call returns that same result.

Nothing after b needs testing. If every element fails, repeated tail calls
reach the empty-list base case.

```rbel example=some-trace-tail
(some atom '(b c))
=>
(b c)
```

```rbel example=some-empty
(some atom nil)
=>
nil
```

```rbel example=some-match
(some atom '((a) b c))
=>
(b c)
```

```rbel example=some-none
(some atom '((a) (b)))
=>
nil
```

As with all, a predicate can make skipped work observable: this one accepts
hit and would report an error for any other element. some returns immediately
at hit, leaving later untested.

```rbel example=some-stops
(some (fn (x) (if (id x 'hit) t (err 'visited))) '(hit later))
=>
(hit later)
```

Because some returns the existing suffix, not a newly constructed copy, we
can check its identity. Here xs receives the whole list, `(cdr xs)` selects its
existing tail, and some returns that same tail after the first element fails.

```rbel example=same-suffix
((fn (xs) (id (some atom xs) (cdr xs))) '((a) b c))
=>
t
```

## 13. reduce: combining a list into one result

`reduce` receives a function f and a proper list xs. It uses f to combine the
head with the result of reducing the tail. A list with one element returns that
element directly; there is no second element to combine with it.

```rbel definition=reduce
(def reduce (f xs)
  (if (no (cdr xs))
      (car xs)
      (f (car xs) (reduce f (cdr xs)))))

```

The condition is `(no (cdr xs))`. When the tail is empty, return the head.
Otherwise evaluate `(f (car xs) (reduce f (cdr xs)))`.

Follow `(reduce join '(a b c))`. Unlike all and some, these recursive calls
have combining work waiting for the result:

```text
(reduce join '(a b c))
  needs join of a with the result of (reduce join '(b c))
  which needs join of b with the result of (reduce join '(c))
  which returns c directly
  so join b with c -> (b . c)
  then join a with (b . c) -> (a b . c)
```

In each call, f is bound to the join function. `(car xs)` supplies the head;
the recursive call supplies the other argument after finishing its own work.
The final tail is c, not nil. Thus `(a b . c)` is a dotted list, not the proper
list `(a b c)`.

This is a **right fold**, meaning the grouping is `join(a, join(b, c))` rather
than `join(join(a, b), c)`. That notation in this sentence explains grouping;
it is not rbel syntax.

```rbel example=reduce-trace-tail
(reduce join '(b c))
=>
(b . c)
```

```rbel example=reduce-trace-combine
(join 'a (join 'b 'c))
=>
(a b . c)
```

```rbel example=reduce-one
(reduce join '(a))
=>
a
```

```rbel example=reduce-right
(reduce join '(a b c))
=>
(a b . c)
```

For an empty input, `(cdr nil)` is `nil`, so the condition is true. The
chosen result is `(car nil)`, also `nil`. That explains the empty-input result
without inventing an extra starting value.

```rbel example=reduce-empty
(reduce join nil)
=>
nil
```

The single-element base case does not call f at all. Even a function that
would report an error is harmless when no combination is needed.

```rbel example=reduce-skips-function
(reduce (fn (left right) (err 'should-not-run)) '(a))
=>
a
```

## 14. What we have built

We began with values, quotation, calls, pairs, lists, types, choices, and function
bindings. Using those rules, the five definitions add reusable behavior:

| Function | Inputs | Result |
| --- | --- | --- |
| no | one value | t exactly when the value is nil |
| atom | one value | t exactly when the value is not a pair |
| all | predicate and proper list | t if every element passes; nil otherwise |
| some | predicate and proper list | suffix at the first passing element; nil if none |
| reduce | combining function and proper list | right-associated combination; nil for empty input |

To read another definition, locate its parameters, identify its body, and follow
the evaluation rules. For recursion, find the base case and what gets smaller.
For each if, identify which expressions can be skipped. When looking at a result,
remember that displayed data is not automatically evaluated again.

## Source and further reading

Chapter explanation by Assistant for Randall Bennett, November 5, 2026, revised
following Randall’s direction to assume no programming knowledge and explain rbel
independently of its current implementation. The five definition blocks preserve
Paul Graham’s published Bel verbatim, including the October 9, 2019 source header.
See [source attribution](../NOTICE.txt) and [snapshot origin](../ORIGIN.json).

The [bootstrap and checking guide](bootstrap.md) explains how to run the examples,
what supplies the starting operations today, and how the original definitions are
kept unchanged. It is separate from the language rules learned in this chapter.

