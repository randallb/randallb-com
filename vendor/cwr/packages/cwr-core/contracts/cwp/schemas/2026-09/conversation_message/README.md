# Conversation message

Schema ref: `cwp://org.coworkerprotocol/2026-09/schemas/conversation_message`

[Schema](schema.json) · [All CWR resource types](../../README.md)

A conversation message is a lasting value contained by a conversation. Its
`message_ref` names that member without making the message a separately declared
resource.

`addressed_actor_refs` records the resolved recipients. `addressing_mode`
records how those recipients were selected:

- `explicit` means the author directly selected the recipient in this input,
  such as by writing `@feedback` or invoking an actor action;
- `implicit` means CWR selected the recipient from conversation context, a reply
  relationship, or another protocol rule.

Addressing and recipient presentation are independent. `addressing_mode` does
not require every destination to repeat a visible mention. Likewise, a delivery
may make an implicitly selected recipient explicit when the destination would
otherwise be ambiguous. A projection or retained delivery directive selects
recipient visibility for that destination; it must not rewrite canonical text or
change `addressing_mode`.

```json
{
  "message_ref": "cwp://com.example.runtime/conversations/01J#messages/01K",
  "author_actor_ref": "~@/actors/randall",
  "addressed_actor_refs": ["~@/actors/feedback"],
  "addressing_mode": "explicit",
  "text": "What's a golden sombrero in baseball?"
}
```

Connectors may remove provider-native address syntax from `text` after resolving
it, but they must preserve the resolved recipients and explicit addressing mode.
Hosts may backfill legacy retained messages from unambiguous relationships—for
example, an initial selected target is explicit and a reply is implicit. When
legacy history does not establish the mode, hosts treat it as implicit. New
messages include the field.

Agent messaging through `cwr.send_message` follows
[CWR Conversation Participation and Discovery](../../../../../../../memos/policy/cwr/cwr-conversation-participation.md).
It addresses an existing participant explicitly; admission precedes delivery
when a host combines invitation with an initial message.

## Published progress

A published interim contribution has `progress.invocation_ref` and
`progress.source_activity_ref`. Its author is the executing coworker; its reply
link names the source request. It has no addressed actors and does not dispatch
another invocation. `action` and `progress` are mutually exclusive.

A published numeric action response also includes its `1xx` `progress.status`;
the linked source activity retains the original response summary. Commentary
omits status and does not advance action response sequencing. The source
activity identifies a publication across retries; equal text alone is not an
identity. Publication and source activity are retained atomically.

Completed updates remain in shared history after a later failure, stop, or final
reply. Hosts may coalesce their display through the linked execution activity,
while adapters still receive the canonical messages. Raw tool output, private
reasoning, partial text, and provider phase indicators are not progress
messages.

## Concrete coworker and configuration context

Current requests identify concrete coworkers. Preserve exact configured runtime,
selected volume, owning role, VO action, named integration binding, and action
contract selection at admission. CVO shorthand resolves once; queuing, retries,
and default changes cannot retarget it. Retained serialized actor fields in
historical records keep their original concrete identities and are never
resolved through today's aliases.

Shared requests, useful updates, and results retain their authors, recipients,
conversation boundaries, and invocation ancestry. Private history belongs to its
runtime, volume, coworker, binding, and requesting conversation. Contract
changes, unsupported historical decoding, and unavailable targets report failure
without starting recipient work or rewriting prior messages, calls, results, or
checkpoints. Retry initialization must remain exact and current permissions
still apply.
