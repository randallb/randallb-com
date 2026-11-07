# Workspace conversations and software-coworker controls

This module extends the first libcwr slice. The REPL is a software coworker:
a slash command names a real exact-target action. A trusted host supplies the
resolved binding and its current grants; the same event is usable without a
terminal. The language coworker's reload action uses a host effect because
compilation and process replacement are platform mechanisms.

The workspace home is not a special conversation. Control calls have canonical
invocation records, but do not invent a conversation or add the REPL/language
coworker to a conversation. Ordinary publication is a separate message operation;
it creates no invocation and activates nobody. A targeted request uses the
existing admission operation, which joins the available recipient and records
queued work. Execution is claimed separately so terminal input can remain usable.

These are newly authored rules from Randall's November 6 workspace/activation
HLQ and software-coworker clarification. The existing CWR contracts were read-only
reference; no existing implementation or tests are reused. This is a local
workspace slice, not a claim of complete Coworker Protocol coverage.

## Safe records and action visibility

```rbel definition=workspace-records
(def ws-symbolp (value)
  (if value (if (id (type value) 'pair) nil
    (if (id (type value) 'char) nil t)) nil))
(def ws-key-member (key record)
  (if record (if (id key (car (car record))) t
    (ws-key-member key (cdr record))) nil))
(def ws-recordp (record)
  (if record
    (if (id (type record) 'pair)
      ((fn (entry)
        (if (id (type entry) 'pair)
          (if (id (type (cdr entry)) 'pair)
            (if (cdr (cdr entry)) nil
              (if (ws-symbolp (car entry))
                (if (ws-recordp (cdr record))
                  (if (ws-key-member (car entry) (cdr record)) nil t) nil) nil)) nil) nil))
        (car record)) nil) t))
(def ws-control-visible (invocation environment)
  (if (cw-get 'control invocation)
    (if (id (cw-get 'owner invocation) (cw-get 'context environment))
      (if (id (cw-get 'author invocation) (cw-get 'caller environment))
        (cw-member (cw-get 'caller environment) (cw-get 'available environment)) nil) nil) nil))
(def ws-visible (state invocation environment)
  (if (cw-get 'control invocation) (ws-control-visible invocation environment)
    (cw-visible state invocation environment)))
(def ws-visible-records (state records environment)
  (if records
    (if (ws-visible state (car records) environment)
      (join (car records) (ws-visible-records state (cdr records) environment))
      (ws-visible-records state (cdr records) environment)) nil))
(def ws-remove-value (value values)
  (if values (if (cw-equal value (car values)) (ws-remove-value value (cdr values))
    (join (car values) (ws-remove-value value (cdr values)))) nil))
```

```rbel example=workspace-record-shape
(ws-recordp '((conversation room) (text (\h \i))))
=>
t
```

```rbel example=workspace-duplicate-field-rejected
(ws-recordp '((conversation room) (conversation other)))
=>
nil
```

## Publication and participation

A shared message carries its original author and no addressed coworker. It is
readable according to the conversation's current membership and authority.
Inviting changes membership only. Neither operation creates worker execution.

```rbel definition=workspace-publication
(def ws-publish (state input environment)
  ((fn (conversation)
    (if (if (cw-member (cw-get 'caller environment) (cw-get 'available environment))
          (cw-allowed-reader conversation environment) nil)
      (if (cw-textp (cw-get 'text input))
        (if (ws-symbolp (cw-get 'message input))
          (if (cw-used-ref state (cw-get 'message input)) (cw-reply state 'conflict nil)
            ((fn (message)
              (cw-reply (cw-save 'conversations
                (cw-set 'messages (join message (cw-get 'messages conversation)) conversation) state)
                'published message))
              (cw-set 'ref (cw-get 'message input)
                (cw-set 'author (cw-get 'caller environment)
                  (cw-set 'ingress (cw-get 'ingress environment)
                    (cw-set 'addressed nil (cw-set 'text (cw-get 'text input) nil)))))))
          (cw-reply state 'invalid nil)) (cw-reply state 'invalid nil))
      (cw-reply state 'unavailable nil)))
    (cw-conversation state (cw-get 'conversation input))))
(def ws-invite (state input environment)
  ((fn (conversation coworker)
    (if (cw-allowed-reader conversation environment)
      (if (cw-member coworker (cw-get 'available environment))
        ((fn (updated) (cw-reply (cw-save 'conversations updated state) 'invited updated))
          (cw-set 'participants
            (if (cw-member coworker (cw-get 'participants conversation))
              (cw-get 'participants conversation)
              (join coworker (cw-get 'participants conversation))) conversation))
        (cw-reply state 'unavailable nil)) (cw-reply state 'unavailable nil)))
    (cw-conversation state (cw-get 'conversation input)) (cw-get 'coworker input)))
(def ws-open (state input environment)
  ((fn (conversation) (if (cw-allowed-reader conversation environment)
    (cw-reply state 'found conversation) (cw-reply state 'unavailable nil)))
    (cw-conversation state (cw-get 'conversation input))))
```

## Removal and stop settlement

Removing a coworker prevents new delivery and marks admitted work cancelled.
An active attempt becomes cancelling; the host requests its process stop and
settles it using the same retained claim. The result is not erased or published
after removal. Work with an uncertain outcome remains uncertain. Re-invitation
never clears cancellation or resumes old requests. Descendants follow their
retained parent chain, so a different child target does not escape the stop.

```rbel definition=workspace-removal
(def ws-stop-tree (state invocation conversation coworker)
  (if invocation
    (if (id (cw-get 'conversation invocation) conversation)
      (if (id (cw-target-worker (cw-get 'target invocation)) coworker) t
        (ws-stop-parent state invocation conversation coworker))
      (ws-stop-parent state invocation conversation coworker)) nil))
(def ws-stop-parent (state invocation conversation coworker)
  ((fn (parent)
    (if parent (ws-stop-tree state (cw-invocation state parent) conversation coworker) nil))
    (cw-get 'parent (cw-get 'request invocation))))
(def ws-stop-record (invocation)
  (if (id (cw-get 'phase invocation) 'admitted)
    (cw-set 'phase 'cancelled (cw-set 'stop-requested t invocation))
    (if (id (cw-get 'phase invocation) 'running)
      (cw-set 'phase (if (cw-get 'attempt invocation) 'cancelling 'cancelled)
        (cw-set 'stop-requested t invocation)) invocation)))
(def ws-stop-records (state records conversation coworker)
  (if records (join
    (if (ws-stop-tree state (car records) conversation coworker)
      (ws-stop-record (car records)) (car records))
    (ws-stop-records state (cdr records) conversation coworker)) nil))
(def ws-cancelling (records conversation)
  (if records
    (if (id (cw-get 'conversation (car records)) conversation)
      (if (id (cw-get 'phase (car records)) 'cancelling)
        (join (car records) (ws-cancelling (cdr records) conversation))
        (ws-cancelling (cdr records) conversation))
      (ws-cancelling (cdr records) conversation)) nil))
(def ws-remove (state input environment)
  ((fn (conversation coworker)
    (if (cw-allowed-reader conversation environment)
      (if (cw-member coworker (cw-get 'participants conversation))
        ((fn (updated stopped)
          (cw-reply (cw-save 'conversations updated
            (cw-set 'invocations stopped state)) 'removed
            (cw-set 'conversation updated
              (cw-set 'cancelling (ws-cancelling stopped (cw-get 'ref conversation)) nil))))
          (cw-set 'participants (ws-remove-value coworker (cw-get 'participants conversation)) conversation)
          (ws-stop-records state (cw-get 'invocations state) (cw-get 'ref conversation) coworker))
        (cw-reply state 'unavailable nil)) (cw-reply state 'unavailable nil)))
    (cw-conversation state (cw-get 'conversation input)) (cw-get 'coworker input)))
```

## Actual REPL coworker actions

The exact target and input are retained in each control invocation. Pure controls
finish in one atomic transition together with their state change and result.
Language check and reload are admitted as running with a host claim and an explicit
platform effect. Check validates configured source and examples without activating
a build; reload checks before replacing the last working executable.
Mutation retries recover the retained result without repeating their state change.
Read controls retain a small `current-view` result reference and reproject the
currently authorized view on retry; they do not claim to preserve an old snapshot.
Their caller, exact target, input and completed invocation remain retained. Status
views contain invocation summaries, never recursively nested prior results. No
control audit record copies complete earlier conversation-history prefixes.
Changed input or reused identity fails.
Permission denial changes neither state nor membership.

```rbel definition=workspace-actions
(def ws-operation (state event environment)
  ((fn (action input)
    (if (id action 'repl--do--new)
      (cw-create state (cw-set 'op 'create
        (cw-set 'conversation (cw-get 'conversation input)
          (cw-set 'owner (cw-get 'context environment) nil))) environment)
      (id action 'repl--do--home) (ws-home state environment)
      (id action 'repl--do--list) (cw-list state '((kind conversation)) environment)
      (id action 'repl--do--open) (ws-open state input environment)
      (id action 'repl--do--history) (ws-open state input environment)
      (id action 'repl--do--publish) (ws-publish state input environment)
      (id action 'repl--do--invite) (ws-invite state input environment)
      (id action 'repl--do--remove) (ws-remove state input environment)
      (id action 'repl--do--status) (cw-reply state 'found (ws-summaries (ws-visible-records state (cw-get 'invocations state) environment)))
      (id action 'repl--do--actions) (cw-reply state 'found (cw-get 'workspace-actions environment))
      (id action 'repl--do--coworkers) (cw-reply state 'found (cw-get 'available environment))
      (id action 'repl--do--help) (cw-reply state 'found (cw-get 'workspace-actions environment))
      (id action 'repl--do--exit) (cw-reply state 'exited nil)
      (id action 'language--do--reload) (cw-reply state 'reload nil)
      (id action 'language--do--check) (cw-reply state 'check nil)
      (cw-reply state 'unsupported nil)))
    (cw-target-action (cw-get 'target event)) (cw-get 'input event)))
(def ws-home (state environment)
  (cw-reply state 'found
    (cw-set 'conversations (cw-visible-records state (cw-get 'conversations state) environment t)
      (cw-set 'available (cw-get 'available environment) nil))))
(def ws-control-record (event environment phase)
  (cw-set 'ref (cw-get 'ref event)
    (cw-set 'origin (cw-origin event environment)
      (cw-set 'request event
        (cw-set 'author (cw-get 'caller environment)
          (cw-set 'ingress (cw-get 'ingress environment)
            (cw-set 'owner (cw-get 'context environment)
              (cw-set 'control t (cw-set 'target (cw-get 'target event)
                (cw-set 'input (cw-get 'input event)
                  (cw-set 'route (cw-get 'route (cw-get 'binding environment))
                    (cw-set 'phase phase nil))))))))))))
(def ws-read-actionp (action)
  (cw-member action '(repl--do--home repl--do--list repl--do--open
    repl--do--history repl--do--status repl--do--actions repl--do--coworkers repl--do--help)))
(def ws-summary (invocation)
  (cw-set 'ref (cw-get 'ref invocation)
    (cw-set 'author (cw-get 'author invocation)
      (cw-set 'conversation (cw-get 'conversation invocation)
        (cw-set 'target (cw-get 'target invocation)
          (cw-set 'phase (cw-get 'phase invocation)
            (cw-set 'attempt (cw-get 'attempt invocation)
              (cw-set 'uncertain (cw-get 'uncertain invocation)
                (cw-set 'control (cw-get 'control invocation) nil)))))))))
(def ws-summaries (invocations)
  (if invocations (join (ws-summary (car invocations)) (ws-summaries (cdr invocations))) nil))
(def ws-control-response (event response)
  ((fn (action code value)
    (if (if (id action 'repl--do--new) t (id action 'repl--do--invite))
      (if (if (id code 'created) t (if (id code 'existing) t (id code 'invited)))
        (cw-two code (cw-set 'messages nil value)) response)
      (if (id action 'repl--do--remove)
        (if (id code 'removed)
          (cw-two code (cw-set 'conversation
            (cw-set 'messages nil (cw-get 'conversation value))
            (cw-set 'cancelling (ws-summaries (cw-get 'cancelling value)) nil))) response)
        response)))
    (cw-target-action (cw-get 'target event)) (car response) (car (cdr response))))
(def ws-control-finish (event environment answer)
  ((fn (response projection)
    (cw-three
      (cw-save 'invocations
        (cw-set 'projection projection
          (cw-set 'result (if projection (cw-two (car response) 'current-view) response)
            (ws-control-record event environment 'completed)))
        (car answer)) response nil))
    (ws-control-response event (car (cdr answer)))
    (ws-read-actionp (cw-target-action (cw-get 'target event)))))
(def ws-control-input-conflict (event)
  ((fn (action input reference)
    (if (id action 'repl--do--new) (id reference (cw-get 'conversation input))
      (if (id action 'repl--do--publish) (id reference (cw-get 'message input)) nil)))
    (cw-target-action (cw-get 'target event)) (cw-get 'input event) (cw-get 'ref event)))
(def ws-control-new (state event environment)
  (if (if (cw-used-ref state (cw-get 'ref event)) t (ws-control-input-conflict event)) (cw-reply state 'conflict nil)
    (if (cw-member (cw-target-action (cw-get 'target event)) '(language--do--reload language--do--check))
      ((fn (invocation)
        (cw-three (cw-save 'invocations invocation state)
          (cw-two 'running invocation)
          (cw-two (if (id (cw-target-action (cw-get 'target event)) 'language--do--check) 'check 'reload) invocation)))
        (cw-set 'claim (cw-get 'claim environment)
          (cw-set 'attempt t (ws-control-record event environment 'running))))
      (ws-control-finish event environment (ws-operation state event environment)))))
(def ws-control (state event environment)
  (if (if (ws-recordp event) (ws-recordp (cw-get 'input event)) nil)
    (if (ws-symbolp (cw-get 'ref event))
      (if (ws-symbolp (cw-get 'origin event))
        (if (cw-authorized (cw-get 'target event) environment)
          ((fn (old)
            (if old
              (if (cw-get 'control old)
                (if (cw-equal event (cw-get 'request old))
                  (if (cw-get 'projection old) (ws-operation state event environment)
                    (if (cw-get 'result old) (cw-three state (cw-get 'result old) nil)
                      (cw-reply state (cw-get 'phase old) old)))
                  (cw-reply state 'conflict nil)) (cw-reply state 'conflict nil))
              (ws-control-new state event environment)))
            (cw-find-origin (cw-origin event environment) (cw-get 'invocations state)))
          (cw-reply state 'denied nil)) (cw-reply state 'invalid nil))
      (cw-reply state 'invalid nil)) (cw-reply state 'invalid nil)))
```

## Authenticated asynchronous settlement

```rbel definition=workspace-settlement
(def ws-claim-valid (invocation event environment)
  (if (cw-get 'executor environment)
    (if (cw-equal (cw-get 'executor environment) (cw-get 'target invocation))
      (id (cw-get 'claim event) (cw-get 'claim invocation)) nil) nil))
(def ws-settle-control (state event environment)
  ((fn (invocation)
    (if (cw-get 'control invocation)
      (if (ws-claim-valid invocation event environment)
        (if (cw-get 'attempt invocation)
          ((fn (settled) (cw-reply (cw-save 'invocations settled state)
            (cw-get 'phase settled) settled))
            (cw-set 'result (cw-two (if (cw-get 'failed event) 'failed 'completed) (cw-get 'result event))
              (cw-set 'phase (if (cw-get 'failed event) 'failed 'completed)
                (cw-set 'attempt nil invocation))))
          (cw-reply state (cw-get 'phase invocation) invocation))
        (cw-reply state 'denied nil)) (cw-reply state 'unavailable nil)))
    (cw-invocation state (cw-get 'ref event))))
(def ws-settle-stop (state event environment)
  ((fn (invocation)
    (if (if (id (cw-get 'phase invocation) 'cancelling) t
          (id (cw-get 'phase invocation) 'cancelled))
      (if (ws-claim-valid invocation event environment)
        (if (id (cw-get 'phase invocation) 'cancelled) (cw-reply state 'cancelled invocation)
          ((fn (settled) (cw-reply (cw-save 'invocations settled state) 'cancelled settled))
            (cw-set 'phase 'cancelled (cw-set 'attempt nil
              (cw-set 'uncertain (cw-get 'uncertain event)
                (cw-set 'result (cw-get 'result event) invocation))))))
        (cw-reply state 'denied nil)) (cw-reply state 'unavailable nil)))
    (cw-invocation state (cw-get 'ref event))))
(def ws-complete (state event environment)
  ((fn (invocation)
    (if (cw-get 'control invocation) (ws-settle-control state event environment)
      (if (if (id (cw-get 'phase invocation) 'cancelling) t
            (id (cw-get 'phase invocation) 'cancelled))
        (ws-settle-stop state event environment) (cw-complete state event environment))))
    (cw-invocation state (cw-get 'ref event))))
(def ws-run (state event environment)
  ((fn (invocation)
    (if (cw-visible state invocation environment)
      (if (cw-get 'control invocation) (cw-reply state 'unsupported nil)
        (if (id (cw-get 'phase invocation) 'cancelled) (cw-reply state 'cancelled invocation)
          (if (id (cw-get 'phase invocation) 'cancelling) (cw-reply state 'cancelling invocation)
            (cw-run state event environment))))
      (cw-reply state 'unavailable nil)))
    (cw-invocation state (cw-get 'ref event))))
(def ws-failed (state event environment)
  ((fn (invocation)
    (if (ws-claim-valid invocation event environment)
      (if (id (cw-get 'phase invocation) 'cancelling)
        (ws-settle-stop state (cw-set 'uncertain t event) environment)
        (if (id (cw-get 'phase invocation) 'running)
          ((fn (uncertain) (cw-reply (cw-save 'invocations uncertain state)
            'reconciliation-required uncertain))
            (cw-set 'attempt nil (cw-set 'uncertain t (cw-set 'checked nil
              (cw-set 'error (cw-get 'result event) invocation)))))
          (cw-reply state 'unavailable nil)))
      (cw-reply state 'denied nil)))
    (cw-invocation state (cw-get 'ref event))))
(def ws-read (state event environment)
  (if (id (cw-get 'kind event) 'invocation)
    ((fn (invocation) (if (cw-get 'control invocation)
      (if (ws-control-visible invocation environment) (cw-reply state 'found invocation)
        (cw-reply state 'unavailable nil)) (cw-read state event environment)))
      (cw-invocation state (cw-get 'ref event)))
    (cw-read state event environment)))
```

```rbel definition=workspace-entry
(def ws-step (state event environment)
  (if (id (cw-get 'op event) 'workspace-call) (ws-control state event environment)
    (id (cw-get 'op event) 'workspace-publish)
      (if (ws-recordp event) (ws-publish state event environment) (cw-reply state 'invalid nil))
    (id (cw-get 'op event) 'workspace-complete) (ws-settle-control state event environment)
    (id (cw-get 'op event) 'workspace-stopped) (ws-settle-stop state event environment)
    (id (cw-get 'op event) 'workspace-failed) (ws-failed state event environment)
    (id (cw-get 'op event) 'complete) (ws-complete state event environment)
    (id (cw-get 'op event) 'run) (ws-run state event environment)
    (id (cw-get 'op event) 'read) (ws-read state event environment)
    (id (cw-get 'op event) 'list)
      (if (id (cw-get 'kind event) 'invocation)
        (cw-reply state 'found (ws-visible-records state (cw-get 'invocations state) environment))
        (cw-list state event environment))
    (cw-step state event environment)))
(def ws-engine (request)
  (ws-step (car request) (car (cdr request)) (car (cdr (cdr request)))))
```

```rbel example=workspace-removal-keeps-other-values
(ws-remove-value 'lint '(root uppercase lint))
=>
(root uppercase)
```

## Adjacent examples of control admission and cancellation

These small pure examples construct fictional state directly. They execute the
same workspace rules, without a C implementation of membership or cancellation.
The fixture block is excluded from the shipped core.

```rbel definition=workspace-fixtures
(def ws-fixture-target (action)
  (join 'runtime (join (if (id action 'uppercase--text) 'uppercase
    (if (cw-member action '(language--do--reload language--do--check)) 'language 'repl))
    (join 'role (join action (join 'binding (join 'contract nil)))))))
(def ws-fixture-environment (action)
  ((fn (target)
    (cw-set 'runtime 'runtime (cw-set 'caller 'person (cw-set 'context 'workspace
      (cw-set 'ingress 'terminal (cw-set 'claim 'claim
        (cw-set 'available '(person other repl language uppercase)
          (cw-set 'allow (join target nil)
            (cw-set 'binding (cw-set 'target target (cw-set 'route 'route nil)) nil)))))))))
    (ws-fixture-target action)))
(def ws-fixture-call (state action input reference)
  (ws-step state (cw-set 'op 'workspace-call
    (cw-set 'ref reference (cw-set 'origin reference
      (cw-set 'target (ws-fixture-target action) (cw-set 'input input nil)))))
    (ws-fixture-environment action)))
(def ws-fixture-new ()
  (ws-fixture-call nil 'repl--do--new '((conversation room)) 'new-call))
(def ws-fixture-code (answer) (car (car (cdr answer))))
(def ws-fixture-mention (state)
  (ws-step state (cw-set 'target (ws-fixture-target 'uppercase--text)
    '((op admit) (ref work) (origin request) (conversation room)
      (message source) (response-message reply) (input (\h \i)) (text (\h \i))))
    (ws-fixture-environment 'uppercase--text)))
(def ws-fixture-queued () (car (ws-fixture-mention (car (ws-fixture-new)))))
(def ws-fixture-running ()
  (car (ws-step (ws-fixture-queued) '((op run) (ref work))
    (ws-fixture-environment 'uppercase--text))))
(def ws-fixture-remove (state)
  (ws-fixture-call state 'repl--do--remove
    '((conversation room) (coworker uppercase)) 'remove-call))
(def ws-fixture-executor ()
  (cw-set 'executor (ws-fixture-target 'uppercase--text)
    (ws-fixture-environment 'uppercase--text)))
```

```rbel example=workspace-new-is-a-real-control
(cw-get 'control (car (cw-get 'invocations (car (ws-fixture-new)))))
=>
t
```

```rbel example=workspace-controls-do-not-join-the-room
(cw-get 'participants (cw-conversation (car (ws-fixture-new)) 'room))
=>
(person)
```

```rbel example=workspace-message-does-not-invoke
((fn (state)
  (cw-equal (cw-get 'invocations state)
    (cw-get 'invocations (car (ws-step state
      '((op workspace-publish) (conversation room) (message note) (text (\h \i)))
      (ws-fixture-environment 'repl--do--publish))))))
  (car (ws-fixture-new)))
=>
t
```

```rbel example=workspace-denied-control-has-no-state
(car (ws-step nil
  (cw-set 'op 'workspace-call (cw-set 'ref 'denied (cw-set 'origin 'denied
    (cw-set 'target (ws-fixture-target 'repl--do--new)
      (cw-set 'input '((conversation room)) nil)))))
  (cw-set 'allow nil (ws-fixture-environment 'repl--do--new))))
=>
nil
```

```rbel example=workspace-new-ref-collision-rejected
(ws-fixture-code (ws-fixture-call nil 'repl--do--new
  '((conversation same)) 'same))
=>
conflict
```

```rbel example=workspace-publication-ref-collision-rejected
(ws-fixture-code (ws-fixture-call (car (ws-fixture-new)) 'repl--do--publish
  '((conversation room) (message same) (text (\h \i))) 'same))
=>
conflict
```

```rbel example=workspace-mention-joins-the-target
(cw-get 'participants (cw-conversation (ws-fixture-queued) 'room))
=>
(uppercase person)
```

```rbel example=workspace-removal-cancels-queued-work
(cw-get 'phase (cw-invocation (car (ws-fixture-remove (ws-fixture-queued))) 'work))
=>
cancelled
```

```rbel example=workspace-removal-requests-active-stop
(cw-get 'phase (cw-invocation (car (ws-fixture-remove (ws-fixture-running))) 'work))
=>
cancelling
```

```rbel example=workspace-cancelled-work-is-private-to-participants
(ws-fixture-code (ws-step (car (ws-fixture-remove (ws-fixture-queued)))
  '((op run) (ref work))
  (cw-set 'caller 'other (ws-fixture-environment 'uppercase--text))))
=>
unavailable
```

```rbel example=workspace-stop-completion-needs-authentication
(ws-fixture-code (ws-step (car (ws-fixture-remove (ws-fixture-running)))
  '((op complete) (ref work) (claim forged) (result (\H \I)))
  (ws-fixture-executor)))
=>
denied
```

```rbel example=workspace-stop-settlement-does-not-publish-reply
((fn (state)
  (cw-get 'ref (car (cw-get 'messages (cw-conversation state 'room)))))
  (car (ws-step (car (ws-fixture-remove (ws-fixture-running)))
    '((op complete) (ref work) (claim claim) (result (\H \I)))
    (ws-fixture-executor))))
=>
source
```

```rbel example=workspace-reload-emits-a-host-effect
(car (car (cdr (cdr (ws-fixture-call nil 'language--do--reload nil 'reload-call)))))
=>
reload
```


```rbel example=workspace-coworker-directory-includes-nonexecutables
(car (cdr (car (cdr (ws-fixture-call nil 'repl--do--coworkers nil 'directory-call)))))
=>
(person other repl language uppercase)
```


```rbel example=workspace-legacy-invocation-list-retains-input
((fn (answer)
  (cw-get 'input (cw-find 'work (car (cdr (car (cdr answer)))))))
  (ws-step (ws-fixture-queued) '((op list) (kind invocation))
    (ws-fixture-environment 'uppercase--text)))
=>
"hi"
```


```rbel example=workspace-check-emits-a-host-effect
(car (car (cdr (cdr (ws-fixture-call nil 'language--do--check nil 'check-call)))))
=>
check
```

```rbel example=workspace-check-retains-the-language-target
(cw-target-action (cw-get 'target
  (car (cw-get 'invocations (car (ws-fixture-call nil 'language--do--check nil 'check-call))))))
=>
language--do--check
```

```rbel example=workspace-check-requires-current-authority
(ws-fixture-code (ws-step nil
  (cw-set 'op 'workspace-call (cw-set 'ref 'check-call (cw-set 'origin 'check-call
    (cw-set 'target (ws-fixture-target 'language--do--check) nil))))
  (cw-set 'allow nil (ws-fixture-environment 'language--do--check))))
=>
denied
```

```rbel example=workspace-check-retry-does-not-repeat-host-effect
((fn (event environment)
  (car (cdr (cdr (ws-step (car (ws-step nil event environment)) event environment)))))
  '((op workspace-call) (ref check-call) (origin check-call)
    (target (runtime language role language--do--check binding contract)))
  (ws-fixture-environment 'language--do--check))
=>
nil
```
