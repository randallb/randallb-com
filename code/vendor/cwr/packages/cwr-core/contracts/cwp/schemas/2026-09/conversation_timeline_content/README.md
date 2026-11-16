# Conversation timeline content link

Schema ref:
`cwp://org.coworkerprotocol/2026-09/schemas/conversation_timeline_content`

[Schema](schema.json) · [Example](example.json) ·
[All CWR resource types](../../README.md)

A timeline item links to canonical content without copying it or taking over its
identity. The first content kinds are `message` and `invocation`. A message
continues to own its author, text, reply relationship, and progress metadata. An
invocation continues to own its input, lifecycle, responses, and private
recovery state.

The content ref is not a placement identity or an idempotency key. The same
content may be published more than once through distinct timeline items.
Following a content link requires fresh authorization; possession of a timeline
item does not by itself grant current access to the content.
