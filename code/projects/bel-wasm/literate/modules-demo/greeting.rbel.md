# Composing a namespaced dependency

`helper` is private to this module and can coexist with the other module's helper.

```rbel definition=greeting-module
(module greeting)
(import "words.rbel.md" as words)
(export greet)
(def helper (text) text)
(def greet (text) (words/finish (helper text)))
```

```rbel example=module-check
(greet "hello")
=>
"!hello"
```
