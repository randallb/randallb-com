# Conversation timeline connection

Schema ref:
`cwp://org.coworkerprotocol/2026-09/schemas/conversation_timeline_connection`

[Schema](schema.json) · [Example](example.json) ·
[All CWR resource types](../../README.md)

A conversation timeline is a tree of ordered, independently pageable
connections. The root connection is owned by the conversation. A child
connection is owned by exactly one invocation timeline item. Every item has one
parent connection, so the retained topology is a forest across conversations,
not a general graph and not one globally flattened list.

Each connection orders only its direct children. Appending to an invocation's
child connection does not revise or splice the root connection. Clients read
newest history with `last` and continue backward with `before`; they may use
`first` and `after` for forward catch-up. Returned edges always remain in
oldest-first sibling order.

`revision` identifies the returned connection snapshot and supports live-update
ordering. It does not participate in cursor identity. Hosts authorize the
conversation and referenced content for every read. A connection or item ref is
not an access grant.
