# Greeter: a small software coworker

Greeter replies with `Hello, ` followed by the text you send it. It is a local
rbel function, with no model or network service. Its first function is the
coworker's entry: a list of bytes comes in and a list of bytes goes out.

```rbel definition=greeter
(def greeter (text) (greeter-append "Hello, " text))
(def greeter-append (prefix text)
  (if prefix
    (join (car prefix) (greeter-append (cdr prefix) text))
    text))
```

The helper copies the greeting one character at a time, then attaches the
original text. The executable examples below explain the expected replies.

```rbel example=greeter-person
(greeter "Randall")
=>
"Hello, Randall"
```

```rbel example=greeter-empty-message
(greeter "")
=>
"Hello, "
```

Launch with `workspace --coworker greeter --source` pointing to this file and
`--rbel` pointing to the native compiler. Then open a conversation and send
`@greeter Randall`. The builtin `@uppercase` coworker remains available separately.
The host retains the real author and reply link for each response.

After editing, `/check` runs these examples without changing the active worker.
`/reload` checks again and replaces Greeter only when the build succeeds. A
failed example or build leaves the last working Greeter active. This source
interface transforms independent chunks of up to 256 bytes, so long messages
can receive the greeting once per chunk.
