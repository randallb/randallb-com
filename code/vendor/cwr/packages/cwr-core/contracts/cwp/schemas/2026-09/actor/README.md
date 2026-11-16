# Retired actor declaration

Concrete addressable identities now use [COWORKER.toml](../coworker/README.md).
Reusable behavior uses [ROLE.toml](../role/README.md). The standalone actor
authoring contract, authored identity fields, admission lists, and automatic
participant defaults are retired.

Existing records retain their original identities and historical contracts. They
must not be reinterpreted through current aliases. Follow the
[migration guide](../../../../../../../memos/runbooks/migrate-cwr-coworkers-and-roles.md)
to convert declarations and select fresh state while retaining old records.
