# Gateway definition

Schema ref: `cwp://org.coworkerprotocol/2026-09/schemas/gateway`

A gateway definition is imported as a standalone resource. It binds one role's
synchronous action to one configured integration. The gateway itself is not a
coworker and does not appear in the conversational actor catalog.

```toml
type = "cwp://org.coworkerprotocol/2026-09/schemas/gateway"
authority = ["process.execute"]
role = { ref = "~@/handler" }
integration = { ref = "~@/google" }
action_name = "send--mail"

[[imports]]
path = "./handler/ROLE.toml"
as = "handler"

[[imports]]
path = "./google.toml"
as = "google"
```

The referenced role declares the action contract and supported adapter. The
integration selects the adapter installation and private inputs. Declare typed
`[[inputs]]` on the gateway when the host supplies values, then forward them
through the import of the receiving resource. The selected handler action must
exist exactly once and return synchronously.

`authority` declares effects available to the handler, such as
`process.execute`. It does not grant credentials or authorize a connection or
caller. Workspace reconfiguration is reserved for coworkers; gateways cannot
declare `cwr:configure`. Optional ingress and delivery actions require a
host-managed `connection_ref`; the host owns connection approval and lifecycle.
A gateway is not a coworker and its actions stay out of ordinary coworker
discovery. A host request can address the gateway through its CWR import alias.
