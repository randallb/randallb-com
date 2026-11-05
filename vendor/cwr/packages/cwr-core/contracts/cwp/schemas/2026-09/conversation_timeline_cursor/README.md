# Conversation timeline cursor

Schema ref:
`cwp://org.coworkerprotocol/2026-09/schemas/conversation_timeline_cursor`

[Schema](schema.json) · [Example](example.json) ·
[All CWR resource types](../../README.md)

A timeline cursor identifies one item at one immutable sibling sequence within
one connection. It is valid only for that connection. Appending later siblings
does not invalidate it, and a connection revision is not part of the cursor.

Reads support `first` with `after` and `last` with `before`. Responses remain in
oldest-first sibling order in both directions. A host rejects an unknown cursor
version, a cursor from another connection, or a cursor whose item and sequence
do not agree. Revision values fence snapshots and subscription updates; they do
not make earlier cursors stale.
