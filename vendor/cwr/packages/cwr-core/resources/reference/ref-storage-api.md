# CWR Ref Storage API

This is the first API contract for CWR resource CRUD, directed relationships,
and traversal. It defines the implementation target; it does not claim these
entrypoints have shipped. It follows the
[CWR storage policy](../../../../memos/policy/cwr/ref-storage.md). CWP owns
the shared
[resource and relationship meanings](../../../../memos/policy/cwp/coworker-protocol.md).

The signatures below describe logical operations, not a selected wire protocol
or programming-language binding. A bound store supplies the caller, namespace,
authorization, and owning domain contracts. Callers never provide physical IDs,
index entries, or backend pagination tokens.

## Values and results

A `Ref` is a canonical absolute CWP ref. Resolve relative authoring refs before
calling this boundary. First-version mutations target live, uncheckpointed refs
without fragments. Checkpoint reads and member operations need separate resolver
contracts and are outside this interface.

A resource value contains its schema ref and schema-valid JSON object. Resource
reads return `{ ref, value, revision }`. An edge read returns
`{ ref, sourceRef, kind, targetRef, properties, revision }`. Revisions are
opaque per-record comparison tokens, not timestamps or a global sequence. They
cannot be reused across removal and re-establishment of the same edge.

Every mutation carries `requestId`, unique within the bound caller and store.
`expectedRevision` is required for updating or deleting an existing record.
Values and edge properties use JSON equality ignoring object-key order; omitted
edge properties mean `{}`. Edges have no mutable properties in this first API.

## Operations

```ts
createRef(ref, value, { requestId });
getRef(ref);
updateRef(ref, changes, { expectedRevision, requestId });
deleteRef(ref, { expectedRevision, requestId });

linkRef(sourceRef, kind, targetRef, { properties, requestId });
getEdge(edgeRef);
unlinkRef(sourceRef, kind, targetRef, { expectedRevision, requestId });

queryEdges(ref, { direction, kinds, pageSize, continuation });
queryRefAncestors(ref, { kinds, maxDepth, maxResults });
queryRefDescendants(ref, { kinds, maxDepth, maxResults });
```

Resource CRUD acts on resource-value records. Edges are independently identified
resources in CWP, but their records are managed through link operations here.
Passing an edge ref to resource CRUD returns `wrong_record_type`; callers use
`getEdge` to read it. `getRef` and `getEdge` return `not_found` when their
record is absent. A local ref binding alone does not constitute a record.

### Resource CRUD

`createRef` creates one value and returns its committed state. An existing live
record returns `already_exists`, including when its value matches. Replaying the
same request ID returns the original result while its payload remains retained,
or the expired-result outcome described below. A deleted resource ref is retired
for this first API: `createRef` returns `ref_retired` rather than resurrecting
it. Restoration is a separate future operation. The binding remains reserved and
cannot acquire another meaning.

`updateRef` applies JSON Merge Patch to the value's JSON object and returns the
committed state. Omitted fields remain; `null` removes a field; arrays replace
as a whole. Validate the complete resulting value against its schema and domain
rules before committing. The schema identity and fields declared immutable by
the domain cannot change. Replacing or clearing a required field fails
validation. A revision mismatch returns `revision_conflict` without writing.
Even a valid no-op update checks the expected revision and returns unchanged
state without advancing its revision.

`deleteRef` removes one value, returning `{ ref, deletedRevision }`. It rejects
with `relationships_exist` while any authoritative incoming or outgoing edge
uses that ref. The check and deletion must serialize with relationship changes;
an eventually consistent traversal is insufficient. Deleting a missing record
returns `not_found`, except when replaying an already recorded delete request.
Removal does not free the identity or imply a cascade. Derived projections are
maintained according to their owner's deletion rules; stale query results may
remain visible. This first operation supports a domain only when every incident
relationship contract requires rejecting deletion until its authoritative edges
are explicitly removed, and permits the applicable derived projection cleanup.
If any affected kind instead requires retaining a relationship to a deleted
endpoint, cascading removal, or another deletion behavior, return
`unsupported_domain_contract` before changing anything. Do not substitute this
API's default for that owner's rule.

### Link and unlink

This first link/query/traversal interface supports unordered relationship kinds
only. A kind with domain-defined position or ordering returns
`unsupported_domain_contract` before mutation or query, including when combined
with unordered kinds. It cannot be silently treated as unordered. Ordered
membership such as message and activity timelines needs an extended contract
that carries positions and respects them through pagination before migration.

`linkRef` establishes a directed authoritative relationship and returns its full
committed edge record. This API allows one relationship per exact
`(sourceRef, kind, targetRef)` tuple in the bound store. The store allocates its
canonical edge ref; callers do not construct it. The tuple reuses that edge
identity after removal, with a fresh revision. Endpoints and kind never change
for that identity. Different kinds express different relationships.

Repeating establishment with equal properties returns the existing edge without
advancing its revision. Different properties return `edge_conflict`; this call
is not an upsert. Self-links and cycles are permitted only where the owning
relationship contract permits them. Endpoint kinds, cardinality, and current
permissions are checked authoritatively during the operation.

The relationship contract states whether endpoints must have live local records
or may name external resources or other edges. Link operations cannot target
locally retired resource identities. A link racing deletion must either commit
first and prevent deletion, or observe deletion and fail; it cannot leave a new
relationship to that retired local resource. Merely binding an external ref does
not validate its existence or grant access.

`unlinkRef` removes only the selected relationship and returns
`{ edgeRef, removed: true, deletedRevision }`. Both endpoints remain intact. An
existing edge must match `expectedRevision`; a mismatch returns
`revision_conflict`. If no edge exists, it returns `{ removed: false }` without
creating a binding. A stale unlink cannot remove a re-established edge because
its revision is different. Domains that allow relationships to edges must reject
unlink with `relationships_exist` while authoritative incident edges refer to
that edge; there is no implicit recursive deletion. If an incident
relationship's contract requires a different removal behavior, return
`unsupported_domain_contract` without changing the graph.

Derived lookup edges are maintained from their owning facts. Callers cannot
create, edit, or remove them through `linkRef` or `unlinkRef`. The relationship
kind's registered domain contract determines whether it is derived; callers
cannot relabel it authoritative to bypass that ownership.

## Atomicity, concurrency, and retries

Each mutation either commits its record, required authoritative relationship
checks, revision, and retry receipt together, or makes no change. Storage may
need stronger internal coordination than one physical row to provide this
promise. It does not expose a general multi-ref transaction API.

All concurrent changes to one record use the same revision check. Competing
creates or identical links cannot produce duplicate authoritative records.
Concurrent delete/link and edge-removal/link operations preserve the
incident-edge rules above. Access decisions and uniqueness checks never rely on
lagging query results. Domain invariants are enforced within the operation that
changes them.

Retry the same request ID with exactly the same operation and arguments after an
uncertain response. Successful mutations retain an opaque replay marker across
restart for the lifetime of the store. That marker prevents executing the same
request twice; it is separate from any saved arguments or result payload. The
resource owner's retention and erasure rules govern every payload copy,
including receipts and argument-comparison material. Retaining a receipt never
creates permission to retain an expired or deleted value.

While its payload is retained, a retry returns the original committed result
after current authorization checks; different arguments return
`request_conflict`. When retention expires or deletion requires erasure, storage
removes protected arguments, results, and content-derived comparison material.
Any later use of that request ID returns
`{ outcome: "committed_result_expired" }`, after current authorization checks,
without replaying the operation or revealing the erased value. This outcome also
applies to changed arguments once comparison material is gone. It means a
mutation committed, not that its old result remains available. The owning domain
must permit the minimal opaque replay marker; otherwise these mutation
operations return `unsupported_domain_contract` before accepting work.

Erasure and expiration apply to receipt payloads as well as live records; they
must prevent subsequent replay of protected data when the owner's deadline is
reached. Failures before commit do not reserve the ID. A caller-visible timeout
does not establish failure: reconcile an uncertain outcome with the same ID.
Receipts grant no permission. An explicit store reset or import into a different
store is outside the retry guarantee.

`getRef` and `getEdge` read authoritative state. A read beginning after a
successful mutation returns that revision or a later state, including absence
after a later deletion. A replay receipt describes the original commit, not
necessarily the present state. Queries below can lag behind either direct read.

## Direct relationships and traversal

`queryEdges` returns direct incoming or outgoing edges. Direction is mandatory;
`kinds` is an explicit nonempty set of registered relationship kinds. It can
include authoritative and derived kinds the caller may read. Results contain
full edge records ordered by canonical edge ref. No record appears twice within
one page. A continuation is opaque to callers and bound to the query, store, and
authorized caller; changed parameters return `invalid_continuation`.
Continuation resumes strictly after the previous edge ref, so it cannot repeat
an earlier edge in that page sequence. A new relationship ordered before that
position can be missed while the graph changes.

The page result is `{ items, continuation, complete }`. `pageSize` is a positive
integer. At the end of the currently visible relation, `complete` is true and
continuation is absent. Otherwise continuation is present and `complete` is
false. Backend paging stays internal. Neither `complete` nor a continuation
claims a graph snapshot or that propagation has caught up.

Ancestor traversal follows incoming edges; descendant traversal follows outgoing
edges. Those names describe direction, not universal parentage. `kinds` is an
explicit nonempty filter applied at every hop, and the same filter applies to
every path. The starting ref is excluded. Traversal returns distinct reachable
refs with their minimum observed hop count; cycle detection prevents revisiting
a ref. External refs may be returned as endpoints without inventing local
records. Only authorized edges and endpoints may contribute to returned paths;
hidden relationships must not be used to reveal otherwise unreachable targets.

`maxDepth` and `maxResults` are mandatory positive integers. Storage traverses
breadth first, with edge refs as the tie-break order. It fetches backend pages
internally until it reaches the visible frontier or a requested limit. Results
are `{ items, complete, stoppedBy }`, where items contain `{ ref, depth }`.
`stoppedBy` lists `max_depth` or `max_results` when unexplored visible work
remains because of those limits; in that case `complete` is false. This first
transitive API has no public continuation. A caller can rerun with larger limits
but receives no snapshot guarantee. An operational deadline or service safety
limit returns an explicit incomplete result with `deadline` or `service_limit`,
never a silently complete prefix.

Queries use eventually consistent views and may temporarily omit new edges or
return removed edges. Each page uses one consistent read of its visible view; a
sequence of pages or traversal across pages does not promise one snapshot. With
a stable graph and successful propagation, new queries converge to the
authoritative relationships. Authorization is checked for each call and page; a
continuation never preserves revoked access.

## Projection recovery and errors

A successful authoritative mutation durably records any unfinished projection
work before returning. Storage retries propagation after restart and can rebuild
derived views from authoritative facts. It exposes stalled propagation through
operator diagnostics; no maximum convergence delay is promised. Callers do not
repair indexes or repeat successful writes to make queries catch up.

Errors distinguish invalid input or schema, wrong record type, unavailable or
unauthorized resources, missing records, existing records, retired refs,
revision conflicts, edge conflicts, relationship constraints, request-ID
conflicts, unsupported domain contracts, invalid continuations, and storage
failure. Access-denied responses must not reveal protected existence, revisions,
or relationships. A storage failure with an uncertain commit outcome tells the
caller to reconcile using its request ID. No failure response reports a partial
mutation as successful.

## Implementation boundary

Implement these operations behind the existing domain authorization and schema
owners, then migrate one real consumer end to end. The public shape does not
require a backend key format, a maximum propagation delay, global snapshots,
mutable edge metadata, batch transactions, cascading deletion, checkpoint
retrieval, or cross-runtime resolution. Those need separate contracts before
callers depend on them.
