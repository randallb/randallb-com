# Ordinary resource programs

Resources are ordinary rbel values, constructed by functions and checked by
ordinary rbel functions. This module is the shared interface for a first native
coworker program. It does not read TOML or JSON Schema. Loading a definition
neither grants authority nor starts an action: the owning runtime still controls
admission and execution.

Records are proper lists of unique `(key value)` pairs. IDs and action names are
symbols; names and data text are character lists. A descriptor is a projection
of the executable resource with its handler functions omitted. It carries one
or more roles, each with actions, and bindings connecting roles to `native-rbel`.
Only this adapter is executable in this initial interface. All action names in
a coworker must be distinct, including across roles.

## Safe values

```rbel definition=resource-values
(def resource-get (key record)
  (if record (if (id key (car (car record))) (car (cdr (car record)))
    (resource-get key (cdr record))) nil))
(def resource-list (value)
  (if value (if (id (type value) 'pair) (resource-list (cdr value)) nil) t))
(def resource-symbol (value)
  (if value (id (type value) 'symbol) nil))
(def resource-member (value values)
  (if values (if (id value (car values)) t
    (resource-member value (cdr values))) nil))
(def resource-record (value)
  (if value
    (if (id (type value) 'pair)
      ((fn (entry)
        (if (id (type entry) 'pair)
          (if (id (type (cdr entry)) 'pair)
            (if (cdr (cdr entry)) nil
              (if (resource-symbol (car entry))
                (if (resource-record (cdr value))
                  (if (resource-key (car entry) (cdr value)) nil t) nil) nil)) nil) nil))
       (car value)) nil) t))
(def resource-key (key record)
  (if record (if (id key (car (car record))) t
    (resource-key key (cdr record))) nil))
(def resource-keys (record keys)
  (if record
    (if keys (if (resource-member (car (car record)) keys)
      (resource-keys (cdr record) (resource-remove (car (car record)) keys)) nil) nil)
    (if keys nil t)))
(def resource-remove (value values)
  (if values (if (id value (car values)) (cdr values)
    (join (car values) (resource-remove value (cdr values)))) nil))
(def resource-shape (value keys)
  (if (resource-record value) (resource-keys value keys) nil))
(def resource-text (value)
  (if value (if (id (type value) 'pair)
    (if (id (type (car value)) 'char) (resource-text (cdr value)) nil) nil) t))
(def resource-named (value)
  (if value (resource-text value) nil))
(def resource-result (code value) (join code (join value nil)))
(def resource-entry (key value) (join key (join value nil)))
```

## Data interfaces

An interface is itself a value: `((type record) (fields ((topic nonempty-text)
(notes text))))`. Fields are exact: missing, duplicate and unknown fields fail.
This first interface supports `text` and `nonempty-text`; extending these
functions will extend the native interface without introducing a second
configuration language.

```rbel definition=resource-interfaces
(def resource-interface (interface)
  (if (resource-shape interface '(type fields))
    (if (id (resource-get 'type interface) 'record)
      ((fn (fields)
        (if fields (if (resource-record fields)
          (resource-field-types fields) nil) nil))
       (resource-get 'fields interface)) nil) nil))
(def resource-field-types (fields)
  (if fields
    (if (resource-member (car (cdr (car fields))) '(text nonempty-text))
      (resource-field-types (cdr fields)) nil) t))
(def resource-fields-match (fields value)
  (if fields
    ((fn (key kind)
      (if (resource-key key value)
        (if (id kind 'nonempty-text)
          (if (resource-named (resource-get key value))
            (resource-fields-match (cdr fields) value) nil)
          (if (resource-text (resource-get key value))
            (resource-fields-match (cdr fields) value) nil)) nil))
     (car (car fields)) (car (cdr (car fields)))) t))
(def resource-field-keys (fields)
  (if fields (join (car (car fields)) (resource-field-keys (cdr fields))) nil))
(def resource-value-valid (interface value)
  (if (resource-interface interface)
    (if (resource-shape value (resource-field-keys (resource-get 'fields interface)))
      (resource-fields-match (resource-get 'fields interface) value) nil) nil))
```

## Descriptors and stable identity

The public `resource-check` validates structure and graph references before the
host registers anything. A previous descriptor makes replacement checks retain
both the logical ID and slug. Source paths and build versions are deliberately
absent: moving or editing a program cannot silently replace its identity.

```rbel definition=resource-descriptors
(def resource-action-valid (action)
  (if (resource-shape action '(type name input output))
    (if (id (resource-get 'type action) 'action)
      (if (resource-symbol (resource-get 'name action))
        (if (resource-interface (resource-get 'input action))
          (resource-interface (resource-get 'output action)) nil) nil) nil) nil))
(def resource-actions-valid (actions)
  (if actions (if (id (type actions) 'pair)
    (if (resource-action-valid (car actions))
      (resource-actions-valid (cdr actions)) nil) nil) t))
(def resource-role-valid (role)
  (if (resource-shape role '(type id name actions))
    (if (id (resource-get 'type role) 'role)
      (if (resource-symbol (resource-get 'id role))
        (if (resource-named (resource-get 'name role))
          (if (resource-get 'actions role)
            (resource-actions-valid (resource-get 'actions role)) nil) nil) nil) nil) nil))
(def resource-roles-valid (roles)
  (if roles (if (id (type roles) 'pair)
    (if (resource-role-valid (car roles))
      (resource-roles-valid (cdr roles)) nil) nil) t))
(def resource-find (key value records)
  (if records (if (id value (resource-get key (car records))) (car records)
    (resource-find key value (cdr records))) nil))
(def resource-binding-valid (binding roles)
  (if (resource-shape binding '(type id role adapter))
    (if (id (resource-get 'type binding) 'binding)
      (if (resource-symbol (resource-get 'id binding))
        (if (id (resource-get 'adapter binding) 'native-rbel)
          (if (resource-find 'id (resource-get 'role binding) roles) t nil) nil) nil) nil) nil))
(def resource-bindings-valid (bindings roles)
  (if bindings (if (id (type bindings) 'pair)
    (if (resource-binding-valid (car bindings) roles)
      (resource-bindings-valid (cdr bindings) roles) nil) nil) t))
(def resource-unique (key records seen)
  (if records
    (if (resource-member (resource-get key (car records)) seen) nil
      (resource-unique key (cdr records) (join (resource-get key (car records)) seen))) t))
(def resource-all-actions (roles)
  (if roles (resource-append (resource-get 'actions (car roles))
    (resource-all-actions (cdr roles))) nil))
(def resource-append (prefix suffix)
  (if prefix (join (car prefix) (resource-append (cdr prefix) suffix)) suffix))
(def resource-roles-bound (roles bindings)
  (if roles
    (if (resource-find 'role (resource-get 'id (car roles)) bindings)
      (resource-roles-bound (cdr roles) bindings) nil) t))
(def resource-descriptor-valid (descriptor)
  (if (resource-shape descriptor '(type id name slug roles bindings))
    (if (id (resource-get 'type descriptor) 'coworker)
      (if (resource-symbol (resource-get 'id descriptor))
        (if (resource-symbol (resource-get 'slug descriptor))
          (if (resource-named (resource-get 'name descriptor))
            (if (resource-get 'roles descriptor)
              (if (resource-roles-valid (resource-get 'roles descriptor))
                (if (resource-get 'bindings descriptor)
                  (if (resource-bindings-valid (resource-get 'bindings descriptor) (resource-get 'roles descriptor))
                    (if (resource-unique 'id (resource-get 'roles descriptor) nil)
                      (if (resource-unique 'id (resource-get 'bindings descriptor) nil)
                        (if (resource-unique 'name (resource-all-actions (resource-get 'roles descriptor)) nil)
                          (resource-roles-bound (resource-get 'roles descriptor) (resource-get 'bindings descriptor)) nil) nil) nil) nil) nil) nil) nil) nil) nil) nil) nil) nil))
(def resource-check (descriptor previous)
  (if (resource-descriptor-valid descriptor)
    (if previous
      (if (resource-descriptor-valid previous)
        (if (id (resource-get 'id descriptor) (resource-get 'id previous))
          (if (id (resource-get 'slug descriptor) (resource-get 'slug previous))
            (resource-result 'valid descriptor)
            (resource-result 'invalid "coworker slug changed"))
          (resource-result 'invalid "coworker identity changed"))
        (resource-result 'invalid "previous descriptor is malformed"))
      (resource-result 'valid descriptor))
    (resource-result 'invalid "malformed coworker, role, action, interface or binding descriptor")))
```

The runtime uses the same declared interfaces for nonterminal callers and
terminal requests. Action lookup supplies no authority; it only resolves the
contract already present in a checked descriptor.

```rbel definition=resource-action-interfaces
(def resource-action (descriptor name)
  (resource-find 'name name (resource-all-actions (resource-get 'roles descriptor))))
(def resource-input-valid (descriptor name value)
  ((fn (action)
    (if action (resource-value-valid (resource-get 'input action) value) nil))
   (resource-action descriptor name)))
(def resource-output-valid (descriptor name value)
  ((fn (action)
    (if action (resource-value-valid (resource-get 'output action) value) nil))
   (resource-action descriptor name)))
```

## Executable resources

A resource program stores a one-argument function in each action's
`handler` field. Projection removes the function, rather than naming an external
script or routing through a TOML loader. Invocation checks the declared input
before calling the function and the output before returning success.

The current compiler represents closures as pairs with a private marker.
`resource-handler-form` rejects scalar handlers but does not prove that a pair
is callable. Actual application is checked by the compiler runtime; a forged
pair fails execution and the host retains a failed action. The adjacent Looper
examples exercise actual successful calls, not just this representation check.

```rbel definition=resource-program
(def resource-handler-form (value) (id (type value) 'pair))
(def resource-without (key record)
  (if record (if (id key (car (car record))) (cdr record)
    (join (car record) (resource-without key (cdr record)))) nil))
(def resource-set (key value record)
  (if record (if (id key (car (car record)))
    (join (resource-entry key value) (cdr record))
    (join (car record) (resource-set key value (cdr record))))
    (join (resource-entry key value) nil)))
(def resource-project-actions (actions)
  (if actions (join (resource-without 'handler (car actions))
    (resource-project-actions (cdr actions))) nil))
(def resource-project-roles (roles)
  (if roles (join (resource-set 'actions
    (resource-project-actions (resource-get 'actions (car roles))) (car roles))
    (resource-project-roles (cdr roles))) nil))
(def resource-project (resource)
  (resource-set 'roles (resource-project-roles (resource-get 'roles resource)) resource))
(def resource-handlers-valid (actions)
  (if actions (if (id (type actions) 'pair)
    (if (resource-shape (car actions) '(type name input output handler))
      (if (resource-handler-form (resource-get 'handler (car actions)))
        (resource-handlers-valid (cdr actions)) nil) nil) nil) t))
(def resource-executable-roles (roles)
  (if roles (if (id (type roles) 'pair)
    (if (resource-shape (car roles) '(type id name actions))
      (if (resource-handlers-valid (resource-get 'actions (car roles)))
        (resource-executable-roles (cdr roles)) nil) nil) nil) t))
(def resource-executable-valid (resource)
  (if (resource-shape resource '(type id name slug roles bindings))
    (if (resource-executable-roles (resource-get 'roles resource))
      (resource-descriptor-valid (resource-project resource)) nil) nil))
(def resource-program (resource request)
  (if (resource-executable-valid resource)
    (if (resource-list request)
      (if (id (car request) 'describe)
        (if (cdr request) (resource-result 'invalid-request "describe takes no arguments")
          (resource-result 'ok (resource-project resource)))
        (if (id (car request) 'invoke)
          (if (cdr request)
            (if (cdr (cdr request))
              (if (cdr (cdr (cdr request)))
                (resource-result 'invalid-request "invoke requires action and input")
                (resource-invoke resource (car (cdr request)) (car (cdr (cdr request)))))
              (resource-result 'invalid-request "invoke requires action and input"))
            (resource-result 'invalid-request "invoke requires action and input"))
          (resource-result 'invalid-request "expected describe or invoke")))
      (resource-result 'invalid-request "request must be a proper list"))
    (resource-result 'invalid-resource "resource must supply valid descriptors and handler forms")))
(def resource-invoke (resource name input)
  ((fn (action)
    (if action
      (if (resource-value-valid (resource-get 'input action) input)
        ((fn (output)
          (if (resource-value-valid (resource-get 'output action) output)
            (resource-result 'ok output)
            (resource-result 'invalid-output "handler result does not match action output interface")))
         ((resource-get 'handler action) input))
        (resource-result 'invalid-input "input fields do not match action input interface"))
      (resource-result 'unknown-action "action is not declared by this coworker")))
   (resource-action resource name)))
```

```rbel example=resource-check-rejects-missing-fields
(car (resource-check '((type coworker) (id incomplete)) nil))
=>
invalid
```

```rbel example=resource-interface-checks-values
(resource-value-valid '((type record) (fields ((topic nonempty-text))))
  '((topic "A topic")))
=>
t
```

```rbel example=resource-interface-rejects-duplicate-field
(resource-value-valid '((type record) (fields ((topic nonempty-text))))
  '((topic "A") (topic "B")))
=>
nil
```
