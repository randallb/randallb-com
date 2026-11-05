# Declared Workloop Cloud storage operations

November 2, 2026. Implementation contract under the Motherboard Cloud Initiative
and the coordinator's `packages/cwr-core/src/cloud_execution/CONTRACT.md`.

This contract extends the existing CWR Ref Storage API. These interfaces are
implemented for native SQLite and a workspace-owned Cloudflare Durable Object.
They do not establish migration of existing CWR consumers or customer acceptance.

## Authority and scope

The trusted CWR coordinator resolves the workspace, actor, current source grants,
conversation participation, operation permission and domain schema. Caller-supplied
context is never authentication. Native operations invoke the trusted `Authorizer`
inside their storage snapshot/transaction before reading a receipt or value.
Document mutation also invokes the domain `Validator` on the complete next value.

The Durable Object is a private namespace/service dependency. Its ordinary Worker
HTTP entrypoint always returns 404. `dispatch` requires a coordinator secret of at
least 32 characters, then pins one object to one workspace. That secret belongs to
the trusted coordinator; executors and customer clients never receive it. The
coordinator applies current domain authority and schema validation before this
internal RPC. A shared coordinator credential does not authenticate an arbitrary
customer, actor or source grant. The fixture HTTP worker is a separate local test
entrypoint and must never be deployed as the product storage endpoint.

Source revocation and conversation removal are CWR authority facts. They must be
checked on every protected operation and dependent publication. The document
storage port does not replace those facts with an `allowed` field or a cached token.

## Documents

`DocumentStorage` in `src/documents.rs` and `DocumentStore` in `worker/client.ts`
expose authoritative reads and atomic create/replace/patch/delete. Records contain
canonical reference, immutable schema reference, JSON object value and opaque
revision. Create does not upsert or resurrect retired identities. Replace replaces
the complete value; patch follows JSON Merge Patch. All mutations except create
require the current revision, including no-ops. No-op writes return the unchanged
revision. Schema/domain validation precedes commit. Deletion rejects authoritative
incident relationships, erases receipt payloads retaining the deleted value and
preserves the retired identity.

The exact TypeScript port is `get(context, reference)` and `mutate(request)`.
Each request contains context `{workspace_ref,caller_ref}`, reference, request_id,
expected_revision (null only on create), and an operation. Create carries
schema_ref and value; replace carries value; patch carries patch; delete has no
additional payload. Outcomes are committed `{record,deleted_revision?}` or
committed_result_expired. The native trait uses the same JSON shape.

A route's aggregate document can own its immutable derived onboarding workspace
identity and provisioning state, so admission does not require an unsafe
cross-workspace transaction or traversal-based uniqueness check.

## Invocation journal

`InvocationStorage` in `src/invocation.rs` retains ordinary CWR admission context;
it is not another action authorization engine. Admit, assign, append progress or a
terminal event, and coordinator stop are named atomic operations. Assignment keeps
executor, assignment identity, generation, route fingerprint and expiry. It is
immutable for an invocation. Appends check expected revision, exact assignment,
executor, generation, expiry, next sequence and nonterminal state in the same
commit as the event, record update and receipt. Stop records interruption or
cancellation after expiry without assigning another executor. A later invocation
may use another executor. No database transaction includes an external tool effect.

Read-after-write and ordered event suffix reads use authoritative storage. Pages
require limits 1..1000, return explicit completion and the last sequence as a
continuation when more events remain. Each page rechecks current authority. The
journal retains conversation and parent invocation linkage supplied by normal CWR
admission, along with retained context independent of the executor.

Receipt lookup returns not_committed, retained `{request,receipt}`, or
committed_result_expired. It checks current authorization before returning the
original mutation. A coordinator recovering from acknowledgement loss can replay
that exact mutation, including the original expected revision, without weakening
request equality or inventing the precondition from the latest state.

## Retries, ordering and physical storage

Successful document and invocation mutations reserve a workspace/caller-scoped
request ID in one shared declared-operation namespace. Equal arguments replay the
original result after current authorization. Different retained arguments fail.
Failures roll back value, revision, identities, events and receipt together.
Content-free replay markers outlive disposable payloads. Once a payload is erased,
that request ID returns committed_result_expired even with changed arguments; it
never executes again. Receipts do not authorize access or external replay.

Native persistence reuses cwpdb's graph and `sequence_receipt`; there are no new
physical tables. Durable Object KV transactions serialize the workspace owner;
cwpdb owns keys, chunks and receipt markers. Large values are chunked into 60 KiB
pieces within the same transaction. Mutation and retained-result payloads are
limited to 512 KiB; receipt dependency maintenance has an explicit work limit.
Revisions are opaque per backend, not globally comparable clocks.

## Verification and remaining migration

`cargo test --locked -p cwpdb` exercises the native contract and existing graph
invariants. `worker/conformance.ts` runs one behavior suite against the native
`storage_fixture` example and actual local workerd Durable Object. Native fixture
calls reopen the database on every operation. It covers CAS, competing writers,
request replay/conflict, no-op revision, workspace isolation, erasure/retirement,
assignment fencing, event ordering, terminal barriers and receipt reconciliation.

This is implementation verification. Real native CWR binding, existing-store
migration, hosted CWR execution, model action/delegation, gateways, handset receipt
and guided-to-paid customer acceptance remain separate required checks. All legacy
CWR SQL/connection consumers must still migrate through named operations. The
removal inventory is maintained beside this contract. No wrapper, fixture, component
review or local workerd result establishes full storage-track completion.

## Trusted remote binding and retained environment metadata

`RemoteStore<T: DeclaredTransport>` selects the hosted document/invocation backend
at the trusted CWR host. The transport accepts only declared operation JSON and
must authenticate/mediate a private Worker service binding. An executor cannot
choose its destination or obtain its coordinator credentials. Current domain
permission is checked before every call. Document receipt reconciliation precedes
schema validation and latest-state reads, preserving original replay semantics.
New values are validated against the read revision; the backend repeats the CAS
inside its commit, so a race cannot accept validation from another state.

The common conformance suite also runs through Rust `RemoteStore` against actual
local workerd. Its curl transport is confined to the fictional loopback fixture;
it is not a production transport or deployment configuration.

`environment::EnvironmentStore` now owns the retained local workspace environment
metadata format and named bind/read/snapshot/save/remove/credential-cleanup
operations. It preserves identity, membership, revisions, unique names and
pending cleanup in one commit. CWR retains TOML schema validation, process-name
policy and Keychain effects outside transactions. The backend selects the local
file path and SQLite mechanics. Environment test-support methods are domain-only
legacy/corruption/failure fixtures; none accepts SQL or exposes native handles.
This migration removes all SQLite/SQL/callback use from the CWR environment domain
and its tests. Hosted handlers for this internal host metadata, remaining CWR
consumers and real conversation-backend selection remain unverified.
