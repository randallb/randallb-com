# A reloadable uppercase coworker

This coworker imports the existing uppercase example. Its first definition is
its entry: a list of bytes goes in, a list of bytes comes out. The native chat
host handles the message, retained conversation and reply relationship.

```rbel definition=coworker-uppercase
(import "../../literate/unix-filter.rbel.md")
(def coworker-uppercase (xs) (uppercase xs))
```

```rbel example=coworker-uppercase-greeting
(coworker-uppercase "Hello, coworker!")
=>
"HELLO, COWORKER!"
```

Run chat with `--source` pointing here and `--rbel` pointing to the native
compiler. After editing, `/reload` checks this example and the imported examples,
then replaces the worker only if the build succeeds. The shell stays open and
history stays retained. Transformations run in independent 256-byte chunks.
