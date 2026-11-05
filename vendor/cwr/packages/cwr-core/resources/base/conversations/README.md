# Conversations

Import `./conversations/CWR.toml` from the base library (or the same module by
normal relative composition). The module exports `entities`, `relationships` and `service`.
`entities` contains conversation, message, activity, transfer metadata,
participation and timeline connection/item definitions. Invocation, file and
private provider recovery records remain owned by their existing CWR services;
this module does not expose provider continuation through shared history.

A native embedding binds an imported module with
`LocalRuntime::conversation_module()`. CWR checks the exported service and
entity contracts against the supported bundled definitions before providing
native operations. The binding uses the authenticated host actor and ingress
identity. An established product host can bind the bundled copy with
`bundled_conversation_module_as(ingress, actor)`; it must supply its
authenticated identity. This is the same service implementation used by an
importing pack. Neither path grants conversation participation or retained-data
permissions. The trusted bundled binding carries the host's authenticated
identity; it does not require that actor to be a local graph actor before the
operation's context is known. Publication resolves the existing owner-scoped
catalog and authority using the supplied connection/owner context, so registered
software actors keep their supported authorship. Actors unavailable in that
scope remain denied. Imported binding still checks the exact supported service
and entity exports.

`publish(request_id, conversation_ref, text)` creates a conversation when the
reference is absent and continues it otherwise. `publish_message_in_context`
also carries attachments and existing owner context. Admission is transactional;
repeating a request identity with the same input recovers the accepted
publication. Conflicting reuse is rejected. Publications and activity follow
existing CWR admission, streaming, and terminal execution operations.

`timeline_layout(conversation_ref, deadline)` discovers the retained format.
`Some(root)` selects canonical connection reads; `None` selects legacy projected
reads. Discovery and both readers leave rollout and stored history unchanged.
Disabled activation, old histories under `NewConversations`, and histories whose
migration is skipped remain usable through the legacy path.

`legacy_timeline` returns bounded message/turn anchors from the stored semantic
projection, with first/last keyset pages and no count, payload or dependency
expansion. Its cursor keys carry position and the stored timeline revision;
changed membership/order invalidates them. `legacy_message` reads one exact
returned message key/reference under that revision, including final replies.
Invocation payloads remain with the existing invocation service. Once a
canonical root appears, legacy reads reject rather than reinterpret old
positions. An explicitly empty canonical root remains authoritative and never
selects legacy history.

`canonical_timeline(conversation_ref, connection_ref, page, deadline)` returns a
bounded connection page. `None` selects the root; an invocation item links its
child connection. Callers page those connections independently in sibling order.
Cursors carry connection, item identity, sequence and format version; appends
retain valid item cursors. A connection revision changes with its membership.
The API does not flatten branches, compute a total count, or expand referenced
content during paging. Repeated placements remain distinct.

`message(conversation_ref, item_ref, deadline)` reads only the message at a
reachable placement, including current provisional text. It does not expand
reply chains, invocation work or unrelated placements. Authorization and
conversation ownership apply to both timeline and message reads. Final response
and progress messages live in the invocation's child connection; private
provider continuation is never exposed. Full message payload size is independent
of a placement page's size.

Hosts observe content/publication changes through CWR's existing publication
snapshot and notification services, then read relevant pages or messages again;
text changes need not change placement cursors. The module creates no second
event log. GraphQL binds the same service for its actor/owner-scoped publication
read session and message admission. See [caller boundaries](callers.md) for
external operations and trusted host projections.

`set_archived` accepts true for archive and false for restore. It preserves
history and uses CWR's existing archive reservations and ordered stop handling.
For an owner-scoped upstream software actor, `set_archived_in_context` carries
the authenticated host's connection owner, as publication does. This connection
scope is separate from a pack's owner context. Current registration and
participation are checked again before stopping work and committing the control.
No destructive delete or clear operation is exported. Product-specific sidebar,
pin and presentation rules remain in Workloop.

The storage port retains canonical refs and physical record schemas. Startup
validates retained records against these declarations without assigning owners
or rewriting history. Existing private continuation and recovery contracts stay
in CWR. Test retained fixtures or isolated copies before opening live data with
a changed storage contract.

Message publication appends new shared records. Updating progress or invocation
state uses the owning invocation contract and publishes shared projections;
callers cannot rewrite earlier message text or reorder timeline placements
through this facade. Entity declarations describe storage values and do not
supply a generic edit API for domain records. File attachments are canonical
file references; validation and read access remain with the existing file
service. Recovery reuses retained admission/invocation identities, including
uncertain-write receipts, and never selects private provider continuation from
shared timeline content.
