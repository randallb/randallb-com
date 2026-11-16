# Retired actor integration declaration

A concrete [coworker](../coworker/README.md) now owns named `[[integrations]]`
bindings. Each locates a shared [integration](../integration/README.md),
supplies its route parameters, and retains a stable binding name. Multiple
execution bindings require an explicit default. An unavailable default does not
fall back.

The standalone actor integration authoring contract is retired. Its historical
identities remain part of retained records and cannot be substituted with a
current binding merely because names match. See
[workspace migration](../../../../../../../memos/runbooks/migrate-cwr-coworkers-and-roles.md).
