# Conversation live status

Schema ref:
`cwp://org.coworkerprotocol/2026-09/schemas/conversation_live_status`

[Schema](schema.json) · [Example](example.json) ·
[All CWR resource types](../../README.md)

A conversation live status describes what one actor appears to be doing now. It
is a leased, replaceable value rather than a retained conversation event,
message, or action response.

The first published status is `composing`. It lets a participating client say
that its actor is preparing input for a conversation. `target_actor_refs`
narrows who should care about that activity when the composer already has an
implicit or explicit target.

The publisher session and positive sequence make concurrent clients distinct and
reject stale updates. CWR assigns `expires_at_unix_ms` when it accepts an active
update. A refresh from the same publisher replaces its earlier value. A clear
removes only that publisher's value, and expiry removes abandoned status without
requiring a clear.

Live status is best-effort coordination input. It may affect presentation or a
reversible decision to yield briefly, but it cannot admit, buffer, stop,
terminalize, address, or discard a conversation turn. It is absent from normal
conversation replay. A host may keep bounded diagnostic telemetry without making
that trace part of the conversation record.

`composing` can exist before a new conversation turn, so this first value does
not require a turn or invocation ref. Later statuses that describe admitted work
should identify their related turn when CWP publishes that extension.
