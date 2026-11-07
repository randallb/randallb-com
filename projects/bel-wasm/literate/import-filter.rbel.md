# Importing a source file

`import` loads another source file relative to this one. The imported definitions
and examples are available to this module. The main file's definitions come first,
so `import-uppercase` remains the program entry even with the import written first.

```rbel definition=import-uppercase
(import "unix-filter.rbel.md")
(def import-uppercase (xs) (uppercase xs))
```

The imported module explains the byte conversion. This example checks the
composition using that module's definitions; its own seven examples also run.

```rbel example=import-uppercase-greeting
(import-uppercase "Hello!")
=>
"HELLO!"
```
