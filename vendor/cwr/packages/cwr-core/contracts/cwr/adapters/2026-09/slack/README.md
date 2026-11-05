# Slack legacy adapter contract

The Slack adapter definition and parameter schemas remain built into CWR so
existing workspace declarations can still resolve and load:

`cwp://org.coworkerprotocol/2026-09/adapters/slack`

[Adapter](adapter.toml) · [coworker parameters](coworker-params.schema.json)
· [integration parameters](integration-params.schema.json) ·
[actor-integration parameters](actor-integration-params.schema.json)

CWR no longer includes or packages a Slack host. Loading a workspace that uses
this contract does not connect to Slack and does not provide Slack ingress or
delivery. Existing declarations can remain while deployments remove or migrate
them without making the whole workspace unreadable.

Conversation history, identities, integration refs, and runtime transport
records are independent of the retired host and should be preserved. A future
Slack gateway will require its own implementation and migration; this legacy
contract does not reserve that design.
