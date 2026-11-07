# An entry using a module

Imports are static relative source dependencies. Only explicitly exported names
are accessible through an alias. The first definition in this root remains the
entry. Example labels are local to their source unit; each module can use the
same readable test label.

```rbel definition=module-entry
(import "modules-demo/greeting.rbel.md" as greeting)
(def module-example (text) (greeting/greet text))
```

```rbel example=module-check
(module-example "hello")
=>
"!hello"
```

`rbel check module-example.rbel.md` checks this entry and both dependencies.
Building it creates a byte filter that prepends `!` to each input chunk.
