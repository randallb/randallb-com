# Conversation timeline item

Schema ref:
`cwp://org.coworkerprotocol/2026-09/schemas/conversation_timeline_item`

[Schema](schema.json) · [Example](example.json) ·
[All CWR resource types](../../README.md)

A timeline item is one durable publication placement. `item_ref` identifies the
placement, `publication_ref` makes the publication intent idempotent, and `content`
links to the canonical message or invocation. These identities are distinct.
Replaying one publication ref with different content or placement is invalid;
publishing the same content through another publication ref creates another
item.

`sibling_sequence` is assigned once within `connection_ref` and never changes.
Items are append-only in this release. Removal, movement, or reparenting is not
supported, and referenced canonical content cannot be removed while a placement
remains.

Invocation items receive their stable `child_connection_ref` when the item is
created, even while the child connection is empty. Message items do not have a
child connection.
