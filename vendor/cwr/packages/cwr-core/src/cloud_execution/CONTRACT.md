# Workloop Cloud shared contracts

November 2, 2026. Coordinator implementation decision for the Motherboard Cloud
Superbuild. Parent: memos/intents/initiatives/motherboard-through-workloop-cloud.md.
These are required implementation semantics, not a claim they are implemented or
customer-verified. Routine API signatures may evolve jointly before integration.

## Storage and authority

All CWR persistence uses declared cwpdb operations. Reuse the existing Ref Storage
API and sequence receipt semantics; SQLite, SQL strings, native connections and
transactions remain private to cwpdb. Inventory and remove the full external CWR
SQLite surface; moving one facade does not complete that requirement. Preserve
retained data and existing identity/ordering semantics. Local and hosted backends
must pass common behavior checks for atomicity, revisions, duplicate receipts,
ordered reads, failure/reopen, access denial and stale publication.

A trusted coordinator resolves workspace, caller and current grants. Mutation
requests carry caller/store-scoped request identity and expected revision; do not
trust caller-provided context as authentication. Exact duplicate requests return
the original result after current authorization; conflicting reuse fails. An
expired disposable result returns committed-result-expired without executing again.
Admission, immutable assignment, ordered progress, terminal result and their
receipts must commit atomically with the relevant revision/assignment precondition.
No external provider effect is included in a database transaction.

## Execution

Reuse normal CWR action admission, resolved actor/integration/action contracts,
input/output validation and existing process-adapter frames. Do not create a
second action engine. The coordinator owns invocation identity and linked child
admission. Infrastructure selects the executor from execution requirements.

Each assignment retains workspace, conversation, invocation, optional parent,
executor, attempt/assignment identity, generation, route fingerprint and expiry.
One invocation stays on one executor. Callback authority is short-lived and
assignment-scoped. Callback progress/results carry event/request identity and
sequence. Current assignment, revision and nonterminal state are checked
atomically; exact duplicates return receipts and stale/foreign/conflicting
callbacks fail. Terminal interruption uses the existing failed status with an
explicit interruption reason and retained uncertain effects. It never silently
reassigns or replays that invocation. A later invocation may run elsewhere using
retained history, with no previous executor-local sessions or credentials.

Delegation uses normal target discovery, admission and parent/call linkage.
Executors cannot manufacture authority, provider destinations or grants.
Gateways resolve credentials and selected source grants outside executors, enforce
workspace/person/operation/scopes/current revision, and deny after cancellation
or revocation. Network egress must enforce this route, rather than relying on
cooperative proxy settings. Retain uncertain external outcomes for reconciliation.

## Customer identity and conversation access

Provider adapters authenticate the webhook and supply provider/account/message,
business-number, sender and route/group identity plus participant evidence.
Only configured Whitney onboarding authority creates an onboarding workspace.
Deduplicate provider event admission and make group-route-to-workspace unique.
A new onboarding group gets a separate workspace; neither participants sorted
into a set nor a message ID is sufficient proof of a new group. Until a transport
can identify it without ambiguity, preserve the attempt and reject provisioning.
Never guess a last-used workspace for a multi-workspace phone/direct message.

Verified Accounts person, phone, workspace member, conversation participant,
source grant, guided service and paid entitlement are separate facts.
Guided service permits authorized work before payment. Motherboard is the actual
addressed coworker, with stable identity independent of number/container.

Private OAuth uses existing authenticated Accounts identity, expiring single-use
workspace/session binding and PKCE. Reusable provider credentials stay encrypted
with the trusted gateway. Source grant binds owner person, workspace, source,
scopes and revision. Current grants are checked on every protected operation and
before accepting dependent publication. A revoked source is never usable because
a token was issued earlier.

Verified signed payment events bind checkout/person/workspace/product and are
idempotent. Graduation creates/reuses one private conversation in that same
workspace; connections and ongoing work stay in place. Whitney stays a workspace
coworker; membership alone gives no source access or private-conversation access.
Use existing CWR participation rules. Removal denies later access immediately.
Invitation/reinvitation requires the customer
to explicitly agree to share this conversation, including its retained earlier
messages. Without that agreement the invitation fails. This matches existing CWR
shared-history semantics; it grants no mailbox/source access. Removal denies later
activity while retaining history and the coworker identity. The customer-facing
invitation must say what history will be shared before exposure.

## Motherboard work and external effects

Use authorized Gmail/Calendar gateways; source text is evidence, never authority.
Keep confirmed family facts, source observations, ambiguity and corrections
separate. A stable workspace/customer/source-event identity links calendar effects.
Retries and updated/cancelled messages reconcile the original event instead of
creating duplicates. Customer-confirmed corrections outrank stale source facts.
Missing timezone/date/identity information produces a focused clarification before
a write. Persist intended effect and request identity before a provider mutation;
on acknowledgement loss inspect provider state and record uncertainty before retry.
Do not advance delivered-result memory without a verified delivery receipt.

Minimum ongoing processing is a scoped provider-change delivery/cursor mechanism
with retained pending work and retry/restart behavior; do not expand the paused
general scheduler. Source revocation/cancellation ends further protected work.

## Ownership and acceptance

Storage owns packages/cwpdb and CWR persistence migration. Execution owns
packages/cwr-core/src/cloud_execution, its process route/proxy, and
experimental/workloop-cloud-execution. Messaging owns group ingress/routing and
hosted Accounts identity/OAuth/payment source-grant plumbing. Motherboard owns
tools/motherboard behavior and its calendar reconciliation. The coordinator owns
cross-track integration and shared top-level/Cargo/module files. Agree changes to
another track's files first. Store detailed contracts beside their implementation.

First integrate real CWR model action plus linked fixture, retained result and
next invocation on a replacement executor with Desktop/Bolty offline; then group,
Google/calendar, verified payment and private continuation. Synthetic tests support
implementation but do not satisfy hosted/model/handset/customer acceptance.
Full d3l3r3x3 review has not started. No product merge, deployment, provider spend
or live customer operation is authorized by this implementation decision.
