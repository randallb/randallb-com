# Looper

Looper takes a topic and notes and puts them into a simple brief. For example,
the topic “Launch” and the notes “Small first step.” produce:

> Brief: Launch
> Small first step.

Its one action, `draft--brief`, accepts two fields: `topic` must contain text,
and `notes` can contain text or be empty. The result has one field, `brief`.
Looper copies your words into this format; it does not summarize them or use AI.

This file contains the program and examples you can run to check its behavior.
It is written in rbel.

## Handling a request

The `looper` function below accepts a request. It uses the shared functions in
`resources.rbel.md` to check the request and run the requested action.

```rbel definition=looper-entry
(import "../resources.rbel.md")
(def looper (request) (resource-program (looper-definition) request))
```

## Defining Looper

The functions below describe what Looper accepts, what it returns, and how it
runs:

- `looper-input` and `looper-output` define the fields in a request and result.
- `looper-action` connects the name `draft--brief` to the function that does the work.
- `looper-role` groups the actions Looper offers. There is just one here.
- `looper-definition` gives Looper its name and ID. Its `bindings` entry tells
  the system to run the action using the rbel code in this program.
- `looper-draft` builds the brief from the supplied topic and notes.

The ID and short name (`slug`) are both `looper`. Editing or moving this file
keeps those names, so the system can continue to recognize the same Looper.

```rbel definition=looper-definition
(def looper-input ()
  '((type record) (fields ((topic nonempty-text) (notes text)))))
(def looper-output ()
  '((type record) (fields ((brief text)))))
(def looper-action ()
  (join '(type action)
    (join '(name draft--brief)
      (join (resource-entry 'input (looper-input))
        (join (resource-entry 'output (looper-output))
          (join (resource-entry 'handler looper-draft) nil))))))
(def looper-role ()
  (join '(type role)
    (join '(id looper-role)
      (join '(name "Looper")
        (join (resource-entry 'actions (join (looper-action) nil)) nil)))))
(def looper-definition ()
  (join '(type coworker)
    (join '(id looper)
      (join '(name "Looper")
        (join '(slug looper)
          (join (resource-entry 'roles (join (looper-role) nil))
            (join '(bindings (((type binding) (id loopback)
              (role looper-role) (adapter native-rbel)))) nil)))))))
(def looper-draft (input)
  (join (resource-entry 'brief
    (resource-append "Brief: "
      (resource-append (resource-get 'topic input)
        (join \lf (resource-get 'notes input))))) nil))
```

## Examples and checks

Each example shows a request followed by `=>` and the expected result. The
first two show successful briefs. The remaining checks cover invalid requests,
errors in the definition, and rules for keeping Looper’s identity stable.

```rbel example=looper-formats-brief
(looper '(invoke draft--brief ((topic "Launch") (notes "Small first step."))))
=>
(ok ((brief "Brief: Launch
Small first step.")))
```

```rbel example=looper-empty-notes
(looper '(invoke draft--brief ((topic "Launch") (notes ()))))
=>
(ok ((brief "Brief: Launch
")))
```

```rbel example=looper-rejects-empty-topic
(car (looper '(invoke draft--brief ((topic ()) (notes "Notes")))))
=>
invalid-input
```

```rbel example=looper-rejects-extra-field
(car (looper '(invoke draft--brief ((topic "A") (notes "B") (other "C")))))
=>
invalid-input
```

```rbel example=looper-rejects-unknown-action
(car (looper '(invoke undeclared ((topic "A") (notes "B")))))
=>
unknown-action
```

```rbel example=looper-describes-checked-resource
(car (resource-check (car (cdr (looper '(describe)))) nil))
=>
valid
```

```rbel example=looper-keeps-identity
(car (resource-check (resource-project (looper-definition))
  (resource-set 'id 'replacement (resource-project (looper-definition)))))
=>
invalid
```

```rbel example=looper-keeps-slug
(car (resource-check (resource-project (looper-definition))
  (resource-set 'slug 'replacement (resource-project (looper-definition)))))
=>
invalid
```

```rbel example=looper-rejects-duplicate-input
(car (looper '(invoke draft--brief ((topic "A") (notes "B") (topic "C")))))
=>
invalid-input
```

```rbel example=looper-rejects-scalar-input
(car (looper '(invoke draft--brief malformed)))
=>
invalid-input
```

```rbel example=looper-rejects-nontext-field
(car (looper '(invoke draft--brief ((topic "A") (notes symbolic-not-text)))))
=>
invalid-input
```

```rbel example=looper-rejects-malformed-request
(car (looper '(invoke draft--brief)))
=>
invalid-request
```

```rbel example=looper-rejects-scalar-handler
((fn (resource role action)
  (car (resource-program
    (resource-set 'roles
      (join (resource-set 'actions
        (join (resource-set 'handler 'not-a-function action) nil) role) nil)
      resource) '(describe))))
 (looper-definition) (looper-role) (looper-action))
=>
invalid-resource
```

```rbel example=looper-rejects-handler-output
((fn (resource role action)
  (car (resource-program
    (resource-set 'roles
      (join (resource-set 'actions
        (join (resource-set 'handler (fn (input) '((other "Wrong field"))) action) nil) role) nil)
      resource) '(invoke draft--brief ((topic "A") (notes "B"))))))
 (looper-definition) (looper-role) (looper-action))
=>
invalid-output
```

```rbel example=looper-rejects-unresolved-role
(car (resource-check
  (resource-set 'bindings
    '(((type binding) (id loopback) (role missing) (adapter native-rbel)))
    (resource-project (looper-definition))) nil))
=>
invalid
```

```rbel example=looper-rejects-unknown-adapter
(car (resource-check
  (resource-set 'bindings
    '(((type binding) (id loopback) (role looper-role) (adapter unknown)))
    (resource-project (looper-definition))) nil))
=>
invalid
```

```rbel example=looper-rejects-duplicate-action
((fn (resource role action)
  (car (resource-check
    (resource-set 'roles
      (join (resource-set 'actions (join action (join action nil)) role) nil)
      resource) nil)))
 (resource-project (looper-definition))
 (car (resource-get 'roles (resource-project (looper-definition))))
 (resource-action (resource-project (looper-definition)) 'draft--brief))
=>
invalid
```

```rbel example=looper-rejects-unbound-role
(car (resource-check
  (resource-set 'bindings nil (resource-project (looper-definition))) nil))
=>
invalid
```

```rbel example=looper-descriptor-has-no-handler
(resource-key 'handler
  (resource-action (car (cdr (looper '(describe)))) 'draft--brief))
=>
nil
```
