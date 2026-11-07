# Register ordinary resource programs

This trusted runtime boundary validates the executable program's public descriptor
before registering its targets. The platform supplies canonical references; rbel
constructs the action catalog and controls installation authority. Loading is not
an action invocation. Existing rows, availability and grants survive reopening,
so a source edit cannot restore revoked authority.

This first resource-program slice keeps role, action, interface and binding
metadata fixed after installation. Handler and display-name edits can reload.
Changing the callable contract requires an explicit replacement resource.

```rbel definition=resource-host-rules
(def resource-host-compatible (descriptor previous)
  (if previous
    (if (cw-equal (cw-get 'roles descriptor) (cw-get 'roles previous))
      (cw-equal (cw-get 'bindings descriptor) (cw-get 'bindings previous)) nil) t))
(def resource-host-check (descriptor previous)
  ((fn (checked)
    (if (id (car checked) 'valid)
      (if (resource-host-compatible descriptor previous) checked
        (resource-result 'invalid "installed role, action, interface or binding changed")) checked))
    (resource-check descriptor previous)))
(def resource-host-ref (key identities) (cw-get key identities))
(def resource-host-rows (descriptor roles identities base)
  (if roles
    (resource-append
      (resource-host-actions descriptor (car roles) (cw-get 'actions (car roles)) identities base)
      (resource-host-rows descriptor (cdr roles) identities base)) nil))
(def resource-host-actions (descriptor role actions identities base)
  (if actions
    (join (resource-host-row descriptor role (car actions) identities base)
      (resource-host-actions descriptor role (cdr actions) identities base)) nil))
(def resource-host-row (descriptor role action identities base)
  ((fn (role-ref binding-ref contract-ref route-ref)
    (cw-set 'name (cw-get 'name action)
      (cw-set 'resource (cw-get 'id descriptor)
        (cw-set 'slug (cw-get 'slug descriptor)
          (cw-set 'display-name (cw-get 'name descriptor)
            (cw-set 'description "Ordinary rbel resource action"
              (cw-set 'target
                (join (cw-get 'runtime identities)
                  (join (cw-get 'coworker identities)
                    (join role-ref (join (cw-get 'name action)
                      (join binding-ref (join contract-ref nil))))))
                (cw-set 'input-interface (cw-get 'input action)
                  (cw-set 'output-interface (cw-get 'output action)
                    (cw-set 'input-schema (cw-get 'input action)
                      (cw-set 'route route-ref
                        (cw-set 'required nil (cw-set 'grants nil (cw-set 'capabilities nil
                          (cw-set 'repeat-safe t nil)))))))))))))))
    (cw-get (cw-get 'id role) (cw-get 'roles identities))
    (cw-get (cw-get 'id role) (cw-get 'bindings identities))
    (cw-get (cw-get 'name action) (cw-get 'contracts identities))
    (cw-get (cw-get 'name action) (cw-get 'routes identities))))
(def resource-host-collision (rows catalog)
  (if rows
    (if (resource-host-row-collision (car rows) catalog) t
      (resource-host-collision (cdr rows) catalog)) nil))
(def resource-host-row-collision (row catalog)
  (if catalog
    (if (id (cw-get 'name row) (cw-get 'name (car catalog))) t
      (if (id (cw-get 'slug row) (cw-get 'name (car catalog))) t
        (if (id (cw-target-worker (cw-get 'target row))
                (cw-target-worker (cw-get 'target (car catalog)))) t
          (resource-host-row-collision row (cdr catalog))))) nil))
(def resource-host-base (catalog)
  (if catalog
    (if (id (cw-target-action (cw-get 'target (car catalog))) 'uppercase--text)
      (car catalog) (resource-host-base (cdr catalog))) nil))
(def resource-host-targets (rows)
  (if rows (join (cw-get 'target (car rows)) (resource-host-targets (cdr rows))) nil))
(def resource-host-register (descriptor identities environment)
  ((fn (previous checked)
    (if (id (car checked) 'valid)
      (if previous
        (resource-result 'registered (cw-set 'resource-descriptor descriptor environment))
        ((fn (base rows)
          (if base
            (if (resource-host-collision rows (cw-get 'workspace-actions environment))
              (resource-result 'invalid "resource action or coworker alias conflicts with catalog")
              (resource-result 'registered
                (cw-set 'resource-descriptor descriptor
                  (cw-set 'workspace-actions
                    (resource-append (cw-get 'workspace-actions environment) rows)
                    (cw-set 'allow
                      (if (cw-authorized (cw-get 'target base) (cw-set 'binding base environment))
                        (resource-append (cw-get 'allow environment) (resource-host-targets rows))
                        (cw-get 'allow environment))
                      (cw-set 'available
                        (if (cw-member (cw-target-worker (cw-get 'target base))
                              (cw-get 'available environment))
                          (join (cw-get 'coworker identities) (cw-get 'available environment))
                          (cw-get 'available environment)) environment))))))
            (resource-result 'invalid "workspace has no installation binding")))
          (resource-host-base (cw-get 'workspace-actions environment))
          (resource-host-rows descriptor (cw-get 'roles descriptor) identities nil))) checked))
    (cw-get 'resource-descriptor environment)
    (resource-host-check descriptor (cw-get 'resource-descriptor environment))))
(def resource-host-entry (state event environment)
  ((fn (answer) (cw-reply state (car answer) (car (cdr answer))))
    (if (id (cw-get 'op event) 'resource-check)
      (resource-host-check (cw-get 'descriptor event) (cw-get 'previous event))
      (id (cw-get 'op event) 'resource-output-check)
        (resource-result
          (if (cw-get 'output-interface (cw-get 'invocation event))
            (if (cw-interface-valid (cw-get 'output-interface (cw-get 'invocation event))
                  (cw-get 'result event)) 'valid 'invalid) 'invalid) nil)
      (resource-host-register (cw-get 'descriptor event) (cw-get 'identities event) environment))))
```
