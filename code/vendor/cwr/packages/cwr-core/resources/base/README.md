# CWR base pack experiment

This reusable LoopGraph pack composes `entities/CWR.toml`. It is a source-level
runtime pack, not a Desktop `.cwpack` release. No manager coworker, automatic
installation, new global intrinsic, or existing conversation migration is added.
Normal CWR loading validates the declared resource type; the explicit entity
catalog additionally compiles each value schema.

The first supported definition is a cwpdb-backed variable value. Its TOML source
is authoritative configuration; instances are graph records. The October 22
entity-definition schema is experimental and only supports inline JSON Schema
object values and the `cwpdb` storage binding. No arbitrary evaluator,
callbacks, automatic relationships, secret storage, change cycles, or general
ORM is loaded. Edit the TOML to explore the contract; use a fresh isolated store
when changing its meaning. Schema evolution and pinned historical definitions
are deferred.

## Try it

From the repository root, build the CLI:

```sh
direnv exec . cargo build -p cwr-cli
```

Use a temporary directory and fictional values. Each command below is a separate
process; the get command demonstrates persistence across CLI restart:

```sh
entity_demo_dir=$(mktemp -d)
printf '%s\n' '{"name":"EXAMPLE","value":"fictional"}' > "$entity_demo_dir/value.json"
target/debug/cwr entity types packages/cwr-core/resources/base/CWR.toml
target/debug/cwr entity create packages/cwr-core/resources/base/CWR.toml "$entity_demo_dir/store.sqlite" cwp://org.coworkerprotocol/cwr/entities/variable cwp://com.example/entities/one "$entity_demo_dir/value.json"
target/debug/cwr entity get packages/cwr-core/resources/base/CWR.toml "$entity_demo_dir/store.sqlite" cwp://org.coworkerprotocol/cwr/entities/variable cwp://com.example/entities/one
```

Repeat create: it fails `already_exists` without changing the stored value.
Change `name` to `bad name` and choose a fresh instance ref: it fails schema
validation without creating that entity. `cwr check` accepts the composed pack;
`cwr entity types` also compiles its value schemas.

These commands explicitly access a caller-selected local store using the user's
filesystem authority. They neither start a runtime nor borrow actor permissions.
Do not point them at a live product database or treat them as a public host API.
Existing authorization, workspace membership/name uniqueness and credential
custody remain in their owning implementations. The example does not connect
these records to Settings, execute a new coworker action, or store secret bytes.

An individual insertion is serialized atomically, rejects live records and
previously claimed identities, and does not upsert. Existing unused endpoint
bindings are permitted. Instance refs must be absolute named live CWP refs with
no checkpoint or fragment. Reads validate kind, definition identity and value.

There is no request-ID receipt/replay interface in this experiment. After a lost
create response, read the same chosen ref and inspect its value; absence after
an uncertain response is not a generic exactly-once guarantee. The published
`createRef` contract requires additional work before this primitive implements
that API. Existing intrinsics/coworkers/conversations can be ported into later
modules; none is a prerequisite for this base create/read slice.

## Workspace variable and secret metadata lifecycle

The base shell also composes `workspace/CWR.toml`, exporting the product
`workspace-variable` and `workspace-secret` declarations. The variable schema
describes the existing `record.name`, `record.kind = "variable"`, and
`record.value` envelope. `record_kind` and `record_schema` declare the existing
product storage format; when omitted they default to `declared_entity` and the
definition ref. These are storage bindings, not additional records or SQL schema
changes.

The native Settings `cwr environment` save path uses the bundled TOML through
`EntityCatalog::from_toml` for variables and secret metadata. Creation invokes
`create_in_transaction`; edits and deletion use declared revision-checked
`replace_in_transaction` and `remove_in_transaction`. These join the domain
transaction that checks workspace name uniqueness and writes membership. Caller
authorization, byte limits, revision checks and workspace access remain in their
existing owners. Bundling uses the same declaration and definition/value
validation as graph loading without depending on a development source path in
packaged builds.

Variable and secret metadata reads use `get_in_connection` after the existing
membership lookup. Membership, revision and declared value are read in one
deferred read snapshot; an existing transaction is reused. Declared operations
check the storage binding and value schema, edits require the observed revision,
and removal retains graph retirement and incident-edge cleanup. Missing records
cannot be recreated by an edit, and retired refs cannot be reused. Domain byte
limits and unique names remain enforced before writes.

The secret declaration allows only `record.name`, `record.kind = "secret"`, and
`record.credential_id` (a canonical UUID). Credential bytes remain in Keychain.
The existing cleanup journal still brackets credential writes, retires old items
after replacement, and retries failed cleanup after removal. Declared validation
runs before stored metadata is used by reads or mutations.

No retained-data rewrite, SQL schema change or configuration migration is needed
for workspace metadata.

## Conversations module

The base pack exports [conversations](conversations/README.md), including its
entity definitions and native service binding. Independent importing packs can
bind the module through `LocalRuntime::conversation_module()`. Workloop binds
the bundled copy for create/send while retaining its owner-filtered publication
snapshots and archive orchestration. Both use the same native admission,
authorization, storage and recovery implementation. Startup validates retained
conversation records against the declarations without rewriting identities or
assigning owners. See the module documentation for operations and dependencies.
