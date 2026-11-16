# Conversations and controlled actions in libcwr

This is the authoritative literate source for the first Wasm slice. Rules,
implementation, examples and executable tests live here. It is a fresh rbel
implementation. It does not link to CWR. This slice is not the complete protocol.

## Scope and source mapping

The reference is Sapling revision
`2aabe143b8a1ed65f4adc1b452ad7074581b5091`; consulted path hashes are in
[reference-baseline.json](reference-baseline.json). Paths below are relative to
that read-only source checkout.

| Rule | Source | Definition / adjacent test |
| --- | --- | --- |
| R1: exact identity, authorship and causal links | `memos/policy/cwp/coworker-protocol.md`, Requests and Conversations; `memos/policy/cwr/api-identity.md` | `cw-create`, `cw-admit-new`; `ordinary-workflow` |
| R2: explicit creation, no implicit continuation, stable origin retries | `memos/policy/cwr/cwr-conversation-participation.md`, Conversation creation and continuation | `cw-create`, `cw-admit`; `admission-boundaries` |
| R3: live authority; participation and discovery grant nothing | `memos/policy/cwr/actor-authority.md`; `memos/policy/cwr/cwr-subjects-actors-and-tool-activation.md`; `memos/policy/cwr/cwr-conversation-participation.md`, Native conversation availability | `cw-authorized`, `cw-run`; `authority-boundaries` |
| R4: accepted target and route never change | `memos/policy/cwp/coworker-protocol.md`, Requests; `packages/cwr-core/README.md`, Execution | `cw-run`; `retained-route` |
| R5: completion differs from execution and delivery; one retained terminal result | `memos/policy/cwp/coworker-protocol.md`; observed `packages/cwr-core/src/v2026_09/invocation/state/core.rs`, finish | `cw-complete`; `ordinary-workflow`, `completion-boundaries` |
| R6: interrupted execution remains visible and needs reconciliation | Accepted libcwr strategy and workflow Intent, linked from project superbuild | `cw-recover`, `cw-reconcile`, `cw-run`; `recovery-with-receipt`, `safe-retry`, `unsafe-retry` |

R6 is an explicit local recovery contract for this slice, not a claim that the
reference engine has an `uncertain` invocation state. We retain `running` and
record an uncertainty flag. The reference's `finish` recovers its original
terminal response even when a repeated completion supplies different data; this
slice preserves that behavior after authenticating the execution claim.

Unsupported standards remain unmigrated: nested work, private history, providers,
cancellation, archive/import/export, collections, gateway routing, schedules,
ref-storage CRUD, and general action schemas. This internal tuple ABI is not a
replacement wire contract. Inputs here come from a trusted host with resolved
canonical refs, checked context configuration and an authenticated caller. No
handle resolution or new ref grammar is introduced. The host must not accept
caller-provided identity, permissions, claim tokens, or reconciliation evidence.

## Data and host contract

Records are association lists of `(key value)` entries. Canonical refs are
interned symbols containing the complete absolute ref, never display aliases.
Text is a list of characters. The slice rejects non-text action inputs and source-message bodies before admission. The state contains `conversations` and
`invocations`, each a list of records with a stable `ref`. A transition receives
`(state event environment)` and returns `(next-state response effect)`.
`response` is `(code value)`; `effect` is nil or `(execute invocation)`.
The host commits next-state atomically before consuming an effect. It must not
execute after a failed commit. A single owning host serializes these transitions;
this milestone does not provide concurrent or distributed storage.

The trusted environment contains runtime/caller/ingress refs, the context
selected by the retained conversation owner, current availability, allowed exact
targets, and one checked binding. Reads, admission, execution and reconciliation
reject an environment whose context differs from that immutable owner; navigation
or a different context must not supply the permission policy for existing work. A binding supplies
its exact six-part target `(runtime coworker role action binding contract)`, the
union of selected role requirements, coworker grants, adapter capabilities,
route fingerprint, and a known repeat-safe flag. Authorization is decided below.
Completion requires the host-authenticated retained claim. Recovery is a trusted
host operation. Reconciliation evidence is a durable adapter receipt bound to
the exact invocation, claim, target and input. Missing evidence is never proof
that execution did not happen.

```rbel definition=record-operations
(def cw-get (key record)
  (if record (if (id key (car (car record))) (car (cdr (car record)))
    (cw-get key (cdr record))) nil))
(def cw-entry (key value) (join key (join value nil)))
(def cw-set (key value record)
  (if record (if (id key (car (car record)))
    (join (cw-entry key value) (cdr record))
    (join (car record) (cw-set key value (cdr record))))
    (join (cw-entry key value) nil)))
(def cw-two (a b) (join a (join b nil)))
(def cw-three (a b c) (join a (join b (join c nil))))
(def cw-equal (a b)
  (if (id a b) t
    (if (id (type a) 'pair) (if (id (type b) 'pair)
      (if (cw-equal (car a) (car b)) (cw-equal (cdr a) (cdr b)) nil) nil) nil)))
(def cw-member (value values)
  (if values (if (cw-equal value (car values)) t
    (cw-member value (cdr values))) nil))
(def cw-subset (needed available)
  (if needed (if (cw-member (car needed) available)
    (cw-subset (cdr needed) available) nil) t))
(def cw-find (reference records)
  (if records (if (id reference (cw-get 'ref (car records))) (car records)
    (cw-find reference (cdr records))) nil))
(def cw-put (record records)
  (if records (if (id (cw-get 'ref record) (cw-get 'ref (car records)))
    (join record (cdr records)) (join (car records) (cw-put record (cdr records))))
    (join record nil)))
(def cw-save (kind record state) (cw-set kind (cw-put record (cw-get kind state)) state))
(def cw-reply (state code value) (cw-three state (cw-two code value) nil))
(def cw-allowed-reader (conversation environment)
  (if (id (cw-get 'owner conversation) (cw-get 'context environment))
    (cw-member (cw-get 'caller environment) (cw-get 'participants conversation)) nil))
(def cw-target-worker (target) (car (cdr target)))
(def cw-authorized (target environment)
  ((fn (binding)
    (if (cw-equal target (cw-get 'target binding))
      (if (id (car target) (cw-get 'runtime environment))
        (if (cw-member target (cw-get 'allow environment))
          (if (cw-member (cw-get 'caller environment) (cw-get 'available environment))
            (if (cw-member (cw-target-worker target) (cw-get 'available environment))
              (if (cw-subset (cw-get 'required binding) (cw-get 'grants binding))
                (cw-subset (cw-get 'required binding) (cw-get 'capabilities binding)) nil) nil) nil) nil) nil) nil))
    (cw-get 'binding environment)))
(def cw-conversation (state reference) (cw-find reference (cw-get 'conversations state)))
(def cw-invocation (state reference) (cw-find reference (cw-get 'invocations state)))
(def cw-visible (state invocation environment)
  (cw-allowed-reader (cw-conversation state (cw-get 'conversation invocation)) environment))
```

```rbel example=structural-data
(cw-equal '(a (b c)) '(a (b c)))
=>
t
```

## Reading the tests

The adjacent test blocks are compiled rbel expressions. A continuation is a
function saying what happens after a host operation returns. `rt-request` yields
an operation and its continuation; `rt-assert` compares values inside Wasm and
yields a named assertion. Each test starts with empty disposable host storage.
These fixtures name fictional identities and inputs, not expected protocol state.

```rbel definition=acceptance-fixtures
(def cw-append (a b) (if a (join (car a) (cw-append (cdr a) b)) b))
(def cw-fixture-ref (tail) (sym (cw-append "cwp://org.example.libcwr/" tail)))
(def cw-fixture-target ()
  (join (cw-fixture-ref "runtime") (join (cw-fixture-ref "worker")
    (join (cw-fixture-ref "role") (join 'store--text (join (cw-fixture-ref "binding")
      (join (cw-fixture-ref "contract") nil)))))))
(def cw-fixture-binding ()
  (cw-set 'target (cw-fixture-target) (cw-set 'required (join (sym "workspace.write") nil)
    (cw-set 'grants (join (sym "workspace.write") nil) (cw-set 'capabilities (join (sym "workspace.write") nil)
      (cw-set 'route (cw-fixture-ref "route-one") (cw-set 'repeat-safe t nil)))))))
(def cw-fixture-create ()
  (cw-set 'op 'create (cw-set 'conversation (cw-fixture-ref "conversation-one")
    (cw-set 'owner (cw-fixture-ref "workspace") nil))))
(def cw-fixture-admission ()
  (cw-set 'op 'admit (cw-set 'ref (cw-fixture-ref "invocation-one")
    (cw-set 'origin (cw-fixture-ref "request-one")
      (cw-set 'conversation (cw-fixture-ref "conversation-one")
        (cw-set 'message (cw-fixture-ref "message-one")
          (cw-set 'target (cw-fixture-target)
            (cw-set 'input "hello from rbel" (cw-set 'text "Store this text." nil)))))))))
(def cw-fixture-event (operation)
  (cw-set 'op operation (cw-set 'ref (cw-fixture-ref "invocation-one") nil)))
(def cw-expect-code (label event expected continuation)
  (rt-request 'dispatch event (fn (response) (rt-assert label (car response) expected continuation))))
(def cw-open (continuation)
  (cw-expect-code 'conversation-created (cw-fixture-create) 'created continuation))
(def cw-admitted (continuation)
  (cw-open (fn (ignored) (cw-expect-code 'invocation-admitted (cw-fixture-admission) 'admitted continuation))))
```

## R1 and R2: create explicitly; preserve admission

Creating a conversation records its immutable owner context and initiating
participant. Continuation never creates it. An accepted action adds the concrete
recipient to participation and records a source message with author, ingress,
addressed coworker, reply link and text. A retry with the same ingress, caller and
origin recovers that admission only when its material request matches. Changed
input, target, identities or reused refs conflict. Rejected work adds no message,
participant, invocation, or action effect.

```rbel definition=conversation-creation
(def cw-textp (value)
  (if value (if (id (type value) 'pair)
    (if (id (type (car value)) 'char) (cw-textp (cdr value)) nil) nil) t))
(def cw-create (state event environment)
  ((fn (old)
    (if (id (cw-get 'owner event) (cw-get 'context environment))
      (if (cw-member (cw-get 'caller environment) (cw-get 'available environment))
        (if old
          (if (id (cw-get 'owner old) (cw-get 'owner event))
            (if (id (cw-get 'creator old) (cw-get 'caller environment))
              (cw-reply state 'existing old) (cw-reply state 'unavailable nil))
            (cw-reply state 'conflict nil))
          (if (cw-used-ref state (cw-get 'conversation event)) (cw-reply state 'conflict nil)
            ((fn (conversation) (cw-reply (cw-save 'conversations conversation state) 'created conversation))
              (cw-set 'ref (cw-get 'conversation event)
                (cw-set 'owner (cw-get 'owner event)
                  (cw-set 'creator (cw-get 'caller environment)
                    (cw-set 'participants (join (cw-get 'caller environment) nil) nil)))))))
        (cw-reply state 'denied nil)) (cw-reply state 'denied nil)))
    (cw-conversation state (cw-get 'conversation event))))
(def cw-message-ref-used (reference conversations)
  (if conversations
    (if (cw-find reference (cw-get 'messages (car conversations))) t
      (cw-message-ref-used reference (cdr conversations))) nil))
(def cw-used-ref (state reference)
  (if reference (if (cw-conversation state reference) t
    (if (cw-invocation state reference) t
      (cw-message-ref-used reference (cw-get 'conversations state)))) t))
(def cw-origin (event environment)
  (cw-three (cw-get 'ingress environment) (cw-get 'caller environment) (cw-get 'origin event)))
(def cw-find-origin (origin records)
  (if records (if (cw-equal origin (cw-get 'origin (car records))) (car records)
    (cw-find-origin origin (cdr records))) nil))
(def cw-admit (state event environment)
  (if (cw-textp (cw-get 'input event))
    (if (cw-textp (cw-get 'text event)) (cw-admit-checked state event environment)
      (cw-reply state 'invalid nil)) (cw-reply state 'invalid nil)))
(def cw-admit-checked (state event environment)
  ((fn (conversation)
    (if (cw-allowed-reader conversation environment)
      ((fn (existing)
        (if existing
          (if (cw-equal event (cw-get 'request existing)) (cw-reply state 'existing existing)
            (cw-reply state 'conflict nil))
          (if (cw-authorized (cw-get 'target event) environment)
            (if (cw-get 'parent event) (cw-reply state 'unsupported nil)
              (if (cw-get 'origin event)
                (if (cw-used-ref state (cw-get 'ref event)) (cw-reply state 'conflict nil)
                  (if (cw-used-ref state (cw-get 'message event)) (cw-reply state 'conflict nil)
                    (if (id (cw-get 'ref event) (cw-get 'message event)) (cw-reply state 'conflict nil)
                      (if (cw-get 'reply event)
                        (if (cw-find (cw-get 'reply event) (cw-get 'messages conversation))
                          (cw-admit-new state event environment conversation) (cw-reply state 'unavailable nil))
                        (cw-admit-new state event environment conversation)))))
                (cw-reply state 'invalid nil))) (cw-reply state 'denied nil))))
        (cw-find-origin (cw-origin event environment) (cw-get 'invocations state)))
      (cw-reply state 'unavailable nil)))
    (cw-conversation state (cw-get 'conversation event))))
(def cw-admit-new (state event environment conversation)
  ((fn (invocation)
    (cw-reply
      (cw-save 'invocations invocation
        (cw-save 'conversations
          (cw-set 'participants
            (if (cw-member (cw-target-worker (cw-get 'target event)) (cw-get 'participants conversation))
              (cw-get 'participants conversation)
              (join (cw-target-worker (cw-get 'target event)) (cw-get 'participants conversation)))
            (cw-set 'messages
              (join (cw-set 'ref (cw-get 'message event)
                (cw-set 'author (cw-get 'caller environment)
                  (cw-set 'ingress (cw-get 'ingress environment)
                    (cw-set 'addressed (cw-target-worker (cw-get 'target event))
                      (cw-set 'reply (cw-get 'reply event) (cw-set 'text (cw-get 'text event) nil))))))
                (cw-get 'messages conversation)) conversation)) state))
      'admitted invocation))
    (cw-set 'ref (cw-get 'ref event)
      (cw-set 'origin (cw-origin event environment)
        (cw-set 'request event
          (cw-set 'conversation (cw-get 'conversation event)
            (cw-set 'author (cw-get 'caller environment)
              (cw-set 'ingress (cw-get 'ingress environment)
                (cw-set 'target (cw-get 'target event)
                  (cw-set 'input (cw-get 'input event)
                    (cw-set 'route (cw-get 'route (cw-get 'binding environment))
                      (cw-set 'phase 'admitted nil))))))))))))
```


```rbel test=admission-boundaries
(cw-expect-code 'unknown-conversation (cw-fixture-admission) 'unavailable (fn (ignored)
(cw-open (fn (ignored)
(cw-expect-code 'unknown-reply (cw-set 'reply (cw-fixture-ref "missing") (cw-fixture-admission)) 'unavailable (fn (ignored)
(cw-expect-code 'first-admitted (cw-fixture-admission) 'admitted (fn (ignored)
(cw-expect-code 'same-request-recovered (cw-fixture-admission) 'existing (fn (ignored)
(cw-expect-code 'same-origin-changed-input (cw-set 'input "changed" (cw-fixture-admission)) 'conflict (fn (ignored)
(cw-expect-code 'duplicate-ref-new-origin (cw-set 'origin (cw-fixture-ref "request-two") (cw-fixture-admission)) 'conflict (fn (ignored)
(rt-request 'snapshot nil (fn (answer)
(rt-assert 'one-admission (cdr (cw-get 'invocations answer)) nil (fn (ignored)
(rt-done)))))))))))))))))))
```

```rbel test=create-and-target-boundaries
(cw-open (fn (ignored)
  (cw-expect-code 'create-retry (cw-fixture-create) 'existing (fn (ignored)
    (cw-expect-code 'wrong-owner
      (cw-set 'owner (cw-fixture-ref "other-context") (cw-fixture-create)) 'denied (fn (ignored)
        (cw-expect-code 'cross-runtime-denied
          (cw-set 'target (join (cw-fixture-ref "other-runtime") (cdr (cw-fixture-target)))
            (cw-fixture-admission)) 'denied (fn (ignored)
              (rt-request 'snapshot nil (fn (snapshot)
                (rt-assert 'rejected-work-has-no-invocations (cw-get 'invocations snapshot) nil
                  (fn (ignored) (rt-assert 'rejected-work-adds-no-participant
                    (cw-get 'participants (car (cw-get 'conversations snapshot)))
                    (join (cw-fixture-ref "alice") nil) (fn (ignored) (rt-done)))))))))))))))
```

```rbel test=conversation-audience
(cw-open (fn (ignored)
(rt-request 'configure (join (cw-entry 'caller (cw-fixture-ref "outsider")) nil) (fn (configured)
(cw-expect-code 'outsider-cannot-admit (cw-fixture-admission) 'unavailable (fn (ignored)
(cw-expect-code 'outsider-cannot-read (cw-set 'kind 'conversation (cw-set 'ref (cw-fixture-ref "conversation-one") (cw-fixture-event 'read))) 'unavailable (fn (ignored)
(rt-done)))))))))
```

```rbel test=reply-and-participation
(cw-admitted (fn (ignored)
  (cw-expect-code 'second-message-admitted
    (cw-set 'ref (cw-fixture-ref "invocation-two")
      (cw-set 'origin (cw-fixture-ref "request-two")
        (cw-set 'message (cw-fixture-ref "message-two")
          (cw-set 'reply (cw-fixture-ref "message-one") (cw-fixture-admission)))))
    'admitted (fn (ignored)
      (rt-request 'snapshot nil (fn (snapshot)
        ((fn (conversation)
          (rt-assert 'reply-link-preserved
            (cw-get 'reply (car (cw-get 'messages conversation))) (cw-fixture-ref "message-one")
            (fn (ignored)
              (rt-assert 'recipient-participates-once
                (cw-get 'participants conversation)
                (cw-two (cw-fixture-ref "worker") (cw-fixture-ref "alice"))
                (fn (ignored) (rt-done))))))
          (car (cw-get 'conversations snapshot)))))))))
```

## R3 and R4: execute only the exact authorized route

`run` checks current permissions and the retained route before issuing an effect.
It persists `running` and a fresh host claim before execution. Another `run` while
an attempt is active issues no effect. A changed binding cannot retarget accepted
work. An interrupted attempt cannot run again until reconciliation establishes
that repeating this exact action is known safe. This does not assert exactly-once
external effects.

```rbel definition=execution
(def cw-run (state event environment)
  ((fn (invocation)
    (if (cw-visible state invocation environment)
      (if (id (cw-get 'phase invocation) 'completed) (cw-reply state 'completed invocation)
        (if (cw-authorized (cw-get 'target invocation) environment)
          (if (cw-equal (cw-get 'route invocation) (cw-get 'route (cw-get 'binding environment)))
            (if (cw-get 'attempt invocation) (cw-reply state 'running invocation)
              (if (cw-get 'uncertain invocation)
                (if (cw-get 'checked invocation)
                  (if (cw-get 'repeat-safe (cw-get 'binding environment))
                    (cw-start state invocation environment) (cw-reply state 'reconciliation-required invocation))
                  (cw-reply state 'reconciliation-required invocation))
                (cw-start state invocation environment)))
            (cw-reply state 'route-changed invocation)) (cw-reply state 'denied nil)))
      (cw-reply state 'unavailable nil))) (cw-invocation state (cw-get 'ref event))))
(def cw-start (state invocation environment)
  ((fn (running) (cw-three (cw-save 'invocations running state)
    (cw-two 'running running) (cw-two 'execute running)))
    (cw-set 'phase 'running (cw-set 'attempt t (cw-set 'uncertain nil
      (cw-set 'checked nil (cw-set 'claim (cw-get 'claim environment) invocation)))))))
```


```rbel test=authority-boundaries
(cw-open (fn (ignored)
(rt-request 'configure (join (cw-entry 'allow nil) nil) (fn (configured)
(cw-expect-code 'participation-is-not-authority (cw-fixture-admission) 'denied (fn (ignored)
(rt-request 'configure (join (cw-entry 'binding (cw-set 'grants nil (cw-fixture-binding))) nil) (fn (configured)
(cw-expect-code 'role-is-not-grant (cw-fixture-admission) 'denied (fn (ignored)
(rt-request 'configure (join (cw-entry 'binding (cw-set 'capabilities nil (cw-fixture-binding))) nil) (fn (configured)
(cw-expect-code 'adapter-cannot-enforce (cw-fixture-admission) 'denied (fn (ignored)
(rt-request 'configure (cw-two (cw-entry 'binding (cw-fixture-binding)) (cw-entry 'allow (join (cw-fixture-target) nil))) (fn (configured)
(cw-expect-code 'admitted-before-revocation (cw-fixture-admission) 'admitted (fn (ignored)
(rt-request 'configure (join (cw-entry 'allow nil) nil) (fn (configured)
(cw-expect-code 'revoked-before-execution (cw-fixture-event 'run) 'denied (fn (ignored)
(rt-request 'observe (cw-fixture-ref "invocation-one") (fn (answer)
(rt-assert 'execution-count (cw-get 'executions answer) '() (fn (ignored)
(rt-assert 'artifact (cw-get 'artifact answer) nil (fn (ignored)
(rt-done)))))))))))))))))))))))))))))
```

```rbel test=retained-route
(cw-admitted (fn (ignored)
(rt-request 'configure (join (cw-entry 'binding (cw-set 'route (cw-fixture-ref "route-two") (cw-fixture-binding))) nil) (fn (configured)
(cw-expect-code 'changed-route-rejected (cw-fixture-event 'run) 'route-changed (fn (ignored)
(rt-request 'observe (cw-fixture-ref "invocation-one") (fn (answer)
(rt-assert 'execution-count (cw-get 'executions answer) '() (fn (ignored)
(rt-assert 'artifact (cw-get 'artifact answer) nil (fn (ignored)
(rt-done)))))))))))))
```

```rbel test=commit-before-effect
(cw-admitted (fn (ignored)
(rt-request 'fault 'fail-save (fn (injected)
(rt-request 'try-dispatch (cw-fixture-event 'run) (fn (answer)
(rt-assert 'commit-failed (car answer) 'host-error (fn (ignored)
(rt-request 'observe (cw-fixture-ref "invocation-one") (fn (answer)
(rt-assert 'execution-count (cw-get 'executions answer) '() (fn (ignored)
(rt-assert 'artifact (cw-get 'artifact answer) nil (fn (ignored)
(cw-expect-code 'recovered-run (cw-fixture-event 'run) 'completed (fn (ignored)
(rt-request 'observe (cw-fixture-ref "invocation-one") (fn (answer)
(rt-assert 'execution-count (cw-get 'executions answer) '(t) (fn (ignored)
(rt-assert 'artifact (cw-get 'artifact answer) "hello from rbel" (fn (ignored)
(rt-done)))))))))))))))))))))))
```

## R5: authenticate completion; retain the original outcome

Only the execution boundary holding the retained claim can complete. A successful
completion clears the active attempt and stores one outcome. A duplicate returns
that same record, including after restart, without another effect or publication.
A later different result cannot overwrite it. Completing admitted work without
execution is rejected. The successful deterministic action is the slice's only
terminal response variant; other status classes remain unmigrated.

```rbel definition=completion
(def cw-finish (state invocation result)
  ((fn (completed) (cw-reply (cw-save 'invocations completed state) 'completed completed))
    (cw-set 'phase 'completed (cw-set 'attempt nil (cw-set 'uncertain nil
      (cw-set 'checked nil (cw-set 'result result invocation)))))))
(def cw-complete (state event environment)
  ((fn (invocation)
    (if invocation
      (if (cw-get 'executor environment)
        (if (cw-equal (cw-get 'executor environment) (cw-get 'target invocation))
          (if (id (cw-get 'claim event) (cw-get 'claim invocation))
            (if (id (cw-get 'phase invocation) 'completed) (cw-reply state 'completed invocation)
              (if (cw-get 'attempt invocation) (cw-finish state invocation (cw-get 'result event))
                (cw-reply state 'reconciliation-required invocation)))
            (cw-reply state 'denied nil)) (cw-reply state 'denied nil))
        (cw-reply state 'denied nil)) (cw-reply state 'unavailable nil)))
    (cw-invocation state (cw-get 'ref event))))
```


```rbel test=ordinary-workflow
(cw-admitted (fn (ignored)
(rt-request 'snapshot nil (fn (answer)
(rt-assert 'author-preserved (cw-get 'author (car (cw-get 'messages (car (cw-get 'conversations answer))))) (cw-fixture-ref "alice") (fn (ignored)
(rt-assert 'ingress-preserved (cw-get 'ingress (car (cw-get 'invocations answer))) (cw-fixture-ref "ingress") (fn (ignored)
(rt-assert 'exact-target-retained (cw-get 'target (car (cw-get 'invocations answer))) (cw-fixture-target) (fn (ignored)
(rt-request 'dispatch (cw-fixture-event 'run) (fn (answer)
(rt-assert 'completed (car answer) 'completed (fn (ignored)
((fn (original)
(rt-request 'redeliver-different (cw-fixture-ref "invocation-one") (fn (duplicate)
(rt-assert 'different-completion-recovers-original (cw-get 'result (car (cdr duplicate))) original (fn (ignored)
(rt-request 'restart nil (fn (restarted)
(rt-request 'redeliver (cw-fixture-ref "invocation-one") (fn (answer)
(rt-assert 'duplicate-code (car answer) 'completed (fn (ignored)
(rt-request 'snapshot nil (fn (answer)
(rt-assert 'single-invocation (cdr (cw-get 'invocations answer)) nil (fn (ignored)
(rt-assert 'single-source-message (cdr (cw-get 'messages (car (cw-get 'conversations answer)))) nil (fn (ignored)
(rt-request 'observe (cw-fixture-ref "invocation-one") (fn (answer)
(rt-assert 'execution-count (cw-get 'executions answer) '(t) (fn (ignored)
(rt-assert 'artifact (cw-get 'artifact answer) "hello from rbel" (fn (ignored)
(rt-done)))))))))))))))))))))))) (cw-get 'result (car (cdr answer))))))))))))))))))
```

```rbel test=completion-boundaries
(cw-admitted (fn (ignored)
(cw-expect-code 'unauthenticated-completion (cw-set 'claim (cw-fixture-ref "fake") (cw-fixture-event 'complete)) 'denied (fn (ignored)
(cw-expect-code 'unauthenticated-recovery (cw-fixture-event 'recover) 'denied (fn (ignored)
(rt-request 'observe (cw-fixture-ref "invocation-one") (fn (answer)
(rt-assert 'execution-count (cw-get 'executions answer) '() (fn (ignored)
(rt-assert 'artifact (cw-get 'artifact answer) nil (fn (ignored)
(rt-done)))))))))))))
```

## R6: recover before retrying

A fresh instance reloads persisted values, not old arena pointers. Recovery clears
active claims while retaining running phase, target, input, authorship and claim.
A matching durable action receipt settles the outcome without executing again.
Absent evidence leaves uncertainty visible. Only after that inspection may a
repeat-safe current binding issue another attempt. A missing receipt never makes
an unsafe action safe. Receipts with mismatched identity, claim, target or input
are rejected and do not unlock retry.

```rbel definition=recovery
(def cw-recover-records (records)
  (if records (join
    (if (cw-get 'attempt (car records))
      (cw-set 'attempt nil (cw-set 'uncertain t (cw-set 'checked nil (car records)))) (car records))
    (cw-recover-records (cdr records))) nil))
(def cw-recover (state environment)
  (if (cw-get 'system environment)
    ((fn (recovered) (cw-reply recovered 'recovered nil))
      (cw-set 'invocations (cw-recover-records (cw-get 'invocations state)) state))
    (cw-reply state 'denied nil)))
(def cw-valid-receipt (receipt invocation)
  (if (id (cw-get 'ref receipt) (cw-get 'ref invocation))
    (if (id (cw-get 'claim receipt) (cw-get 'claim invocation))
      (if (cw-equal (cw-get 'target receipt) (cw-get 'target invocation))
        (cw-equal (cw-get 'input receipt) (cw-get 'input invocation)) nil) nil) nil))
(def cw-reconcile (state event environment)
  ((fn (invocation)
    (if (cw-visible state invocation environment)
      (if (id (cw-get 'phase invocation) 'completed) (cw-reply state 'completed invocation)
        (if (cw-get 'uncertain invocation)
          ((fn (evidence)
            (if evidence
              (if (cw-valid-receipt evidence invocation)
                (cw-finish state invocation (cw-get 'result evidence))
                (cw-reply state 'invalid-evidence invocation))
              ((fn (checked) (cw-reply (cw-save 'invocations checked state) 'reconciliation-required checked))
                (cw-set 'checked t invocation))))
            (cw-get 'evidence environment))
          (cw-reply state 'not-uncertain invocation)))
      (cw-reply state 'unavailable nil))) (cw-invocation state (cw-get 'ref event))))
(def cw-read (state event environment)
  (if (id (cw-get 'kind event) 'conversation)
    ((fn (conversation) (if (cw-allowed-reader conversation environment)
      (cw-reply state 'found conversation) (cw-reply state 'unavailable nil)))
      (cw-conversation state (cw-get 'ref event)))
    ((fn (invocation) (if (cw-visible state invocation environment)
      (cw-reply state 'found invocation) (cw-reply state 'unavailable nil)))
      (cw-invocation state (cw-get 'ref event)))))
(def cw-step (state event environment)
  (if (id (cw-get 'op event) 'create) (cw-create state event environment)
    (id (cw-get 'op event) 'admit) (cw-admit state event environment)
    (id (cw-get 'op event) 'run) (cw-run state event environment)
    (id (cw-get 'op event) 'complete) (cw-complete state event environment)
    (id (cw-get 'op event) 'recover) (cw-recover state environment)
    (id (cw-get 'op event) 'reconcile) (cw-reconcile state event environment)
    (id (cw-get 'op event) 'read) (cw-read state event environment)
    (cw-reply state 'unsupported nil)))
```

## Adjacent compiled acceptance tests

Tests below are rbel expressions. Their continuations and assertions run in Wasm.
The runner only supplies the named host mechanisms. `rt-request` suspends the
test with an operation and a continuation; engine recreation does not recreate
the separate test-driver instance. The protocol instance is destroyed and its
values are re-encoded from the on-disk snapshot. No JavaScript protocol model is
used to compute the expected answers.

```rbel definition=workflow-entry
(def cw-engine (request) (cw-step (car request) (car (cdr request)) (car (cdr (cdr request)))))
```


```rbel test=recovery-with-receipt
(cw-admitted (fn (ignored)
(rt-request 'fault 'lose-completion (fn (injected)
(cw-expect-code 'execution-not-completion (cw-fixture-event 'run) 'running (fn (ignored)
(cw-expect-code 'active-attempt-not-repeated (cw-fixture-event 'run) 'running (fn (ignored)
(rt-request 'restart nil (fn (restarted)
(cw-expect-code 'no-blind-retry (cw-fixture-event 'run) 'reconciliation-required (fn (ignored)
(cw-expect-code 'receipt-recovers-completion (cw-fixture-event 'reconcile) 'completed (fn (ignored)
(cw-expect-code 'completed-is-not-rerun (cw-fixture-event 'run) 'completed (fn (ignored)
(rt-request 'observe (cw-fixture-ref "invocation-one") (fn (answer)
(rt-assert 'execution-count (cw-get 'executions answer) '(t) (fn (ignored)
(rt-assert 'artifact (cw-get 'artifact answer) "hello from rbel" (fn (ignored)
(rt-done)))))))))))))))))))))))
```

```rbel test=safe-retry
(cw-admitted (fn (ignored)
(rt-request 'fault 'skip-execution (fn (injected)
(cw-expect-code 'crash-before-action (cw-fixture-event 'run) 'running (fn (ignored)
(rt-request 'restart nil (fn (restarted)
(cw-expect-code 'before-inspection-no-retry (cw-fixture-event 'run) 'reconciliation-required (fn (ignored)
(cw-expect-code 'missing-receipt-inspected (cw-fixture-event 'reconcile) 'reconciliation-required (fn (ignored)
(cw-expect-code 'retry-completes (cw-fixture-event 'run) 'completed (fn (ignored)
(rt-request 'observe (cw-fixture-ref "invocation-one") (fn (answer)
(rt-assert 'execution-count (cw-get 'executions answer) '(t) (fn (ignored)
(rt-assert 'artifact (cw-get 'artifact answer) "hello from rbel" (fn (ignored)
(rt-done)))))))))))))))))))))
```

```rbel test=unsafe-retry
(cw-open (fn (ignored)
((fn (unsafe-target)
(rt-request 'configure (cw-two (cw-entry 'allow (join unsafe-target nil)) (cw-entry 'binding (cw-set 'target unsafe-target (cw-set 'repeat-safe nil (cw-fixture-binding))))) (fn (configured)
(cw-expect-code 'unsafe-admitted (cw-set 'target unsafe-target (cw-fixture-admission)) 'admitted (fn (ignored)
(rt-request 'fault 'lose-receipt (fn (injected)
(cw-expect-code 'append-completion-lost (cw-fixture-event 'run) 'running (fn (ignored)
(rt-request 'restart nil (fn (restarted)
(cw-expect-code 'absent-receipt-is-not-safety (cw-fixture-event 'reconcile) 'reconciliation-required (fn (ignored)
(cw-expect-code 'unsafe-repeat-blocked (cw-fixture-event 'run) 'reconciliation-required (fn (ignored)
(rt-request 'observe (cw-fixture-ref "invocation-one") (fn (answer)
(rt-assert 'execution-count (cw-get 'executions answer) '(t) (fn (ignored)
(rt-assert 'artifact (cw-get 'artifact answer) "hello from rbel" (fn (ignored)
(rt-done)))))))))))))))))))))) (join (cw-fixture-ref "runtime") (join (cw-fixture-ref "worker") (join (cw-fixture-ref "role") (join 'append--text (join (cw-fixture-ref "binding") (join (cw-fixture-ref "contract") nil)))))))))
```

```rbel test=receipt-binding
(cw-admitted (fn (ignored)
(rt-request 'fault 'lose-completion (fn (injected)
(cw-expect-code 'action-executed (cw-fixture-event 'run) 'running (fn (ignored)
(rt-request 'restart nil (fn (restarted)
(rt-request 'fault 'tamper-receipt (fn (injected)
(cw-expect-code 'tampered-receipt-rejected (cw-fixture-event 'reconcile) 'invalid-evidence (fn (ignored)
(cw-expect-code 'invalid-evidence-does-not-unlock (cw-fixture-event 'run) 'reconciliation-required (fn (ignored)
(rt-request 'observe (cw-fixture-ref "invocation-one") (fn (answer)
(rt-assert 'execution-count (cw-get 'executions answer) '(t) (fn (ignored)
(rt-assert 'artifact (cw-get 'artifact answer) "hello from rbel" (fn (ignored)
(rt-done)))))))))))))))))))))
```

```rbel test=action-input-schema
(cw-open (fn (ignored)
  (cw-expect-code 'non-text-input-rejected (cw-set 'input 'wrong (cw-fixture-admission)) 'invalid
    (fn (ignored) (rt-request 'snapshot nil (fn (snapshot)
      (rt-assert 'invalid-input-not-admitted (cw-get 'invocations snapshot) nil
        (fn (ignored) (rt-done)))))))))
```

```rbel test=safe-repeat-after-effect
(cw-admitted (fn (ignored)
  (rt-request 'fault 'lose-receipt (fn (ignored)
    (cw-expect-code 'effect-without-receipt (cw-fixture-event 'run) 'running (fn (ignored)
      (rt-request 'restart nil (fn (ignored)
        (cw-expect-code 'inspect-before-repeat (cw-fixture-event 'reconcile) 'reconciliation-required (fn (ignored)
          (cw-expect-code 'known-safe-repeat (cw-fixture-event 'run) 'completed (fn (ignored)
            (rt-request 'observe (cw-fixture-ref "invocation-one") (fn (observed)
              (rt-assert 'effects-can-repeat (cw-get 'executions observed) '(t t) (fn (ignored)
                (rt-assert 'same-artifact-after-repeat (cw-get 'artifact observed) "hello from rbel"
                  (fn (ignored) (rt-done)))))))))))))))))))
```

```rbel test=owner-context-boundaries
(cw-admitted (fn (ignored)
(rt-request 'fault 'lose-completion (fn (answer)
(cw-expect-code 'completion-lost-for-context-test (cw-fixture-event 'run) 'running (fn (ignored)
(rt-request 'restart nil (fn (answer)
(rt-request 'configure (join (cw-entry 'context (cw-fixture-ref "different-owner")) nil) (fn (answer)
(cw-expect-code 'wrong-context-cannot-execute (cw-fixture-event 'run) 'unavailable (fn (ignored)
(cw-expect-code 'wrong-context-cannot-reconcile (cw-fixture-event 'reconcile) 'unavailable (fn (ignored)
(cw-expect-code 'wrong-context-cannot-admit (cw-set 'origin (cw-fixture-ref "request-two") (cw-set 'message (cw-fixture-ref "message-two") (cw-set 'ref (cw-fixture-ref "invocation-two") (cw-fixture-admission)))) 'unavailable (fn (ignored)
(cw-expect-code 'wrong-context-cannot-read (cw-set 'kind 'conversation (cw-set 'ref (cw-fixture-ref "conversation-one") (cw-fixture-event 'read))) 'unavailable (fn (ignored)
(rt-request 'snapshot nil (fn (answer)
(rt-assert 'owner-unchanged (cw-get 'owner (car (cw-get 'conversations answer))) (cw-fixture-ref "workspace") (fn (ignored)
(rt-assert 'wrong-context-adds-no-work (cdr (cw-get 'invocations answer)) nil (fn (ignored)
(rt-request 'configure (join (cw-entry 'context (cw-fixture-ref "workspace")) nil) (fn (answer)
(cw-expect-code 'original-owner-can-reconcile (cw-fixture-event 'reconcile) 'completed (fn (ignored)
(rt-request 'observe (cw-fixture-ref "invocation-one") (fn (answer)
(rt-assert 'wrong-context-no-extra-effect (cw-get 'executions answer) '(t) (fn (ignored)
(rt-done)))))))))))))))))))))))))))))))))
```
