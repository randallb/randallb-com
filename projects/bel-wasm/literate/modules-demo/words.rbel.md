# A module with a private helper

```rbel definition=words-module
(module words)
(export finish)
(def helper (text) (join \! text))
(def finish (text) (helper text))
```

```rbel example=module-check
(finish "hello")
=>
"!hello"
```
