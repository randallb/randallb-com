# CWR SQLite removal inventory

November 2, 2026. Baseline `a717eae070115796b0daf8e20e1321093aa385bb`.

Expanded baseline source scan identified 163 files with SQL/native callback
surface, including tests. These are migration targets, not all independent
storage owners. The scan also matches calls that require inspection; absence of
a match alone does not prove the boundary. Include SQL in non-Rust artifacts,
Cargo dependencies, exposed cwpdb signatures, database path selection and
application FFI consumers before declaring removal complete.

Existing records must migrate without reset or history duplication. Each owner
uses named atomic cwpdb operations with canonical business refs, explicit work
limits, expected revisions and receipts. A generic SQL/connection alias is not a
migration. New declared document and invocation ports are implemented separately
and do not yet replace these retained-data consumers.

## Desktop registry, versions, history and receipts

| File                                                              | Surface matches | Status                                                                                        |
| ----------------------------------------------------------------- | --------------: | --------------------------------------------------------------------------------------------- |
| `packages/cwr-core/src/desktop_storage/domain_errors.rs`          |               6 | migrated by coordinator; named native cwpdb domain operations / physical tests moved to cwpdb |
| `packages/cwr-core/src/desktop_storage/history.rs`                |               3 | migrated by coordinator; named native cwpdb domain operations / physical tests moved to cwpdb |
| `packages/cwr-core/src/desktop_storage/ordering.rs`               |               3 | migrated by coordinator; named native cwpdb domain operations / physical tests moved to cwpdb |
| `packages/cwr-core/src/desktop_storage/personal_collections.rs`   |               4 | migrated by coordinator; named native cwpdb domain operations / physical tests moved to cwpdb |
| `packages/cwr-core/src/desktop_storage/receipts.rs`               |               1 | migrated by coordinator; named cwpdb Desktop operations / physical tests moved to cwpdb       |
| `packages/cwr-core/src/desktop_storage/registry.rs`               |               3 | migrated by coordinator; named cwpdb Desktop operations / physical tests moved to cwpdb       |
| `packages/cwr-core/src/desktop_storage/registry_tests.rs`         |              15 | migrated by coordinator; named cwpdb Desktop operations / physical tests moved to cwpdb       |
| `packages/cwr-core/src/desktop_storage/versions.rs`               |               5 | migrated by coordinator; named cwpdb Desktop operations / physical tests moved to cwpdb       |
| `packages/cwr-core/src/desktop_storage/versions_tests.rs`         |              11 | migrated by coordinator; named cwpdb Desktop operations / physical tests moved to cwpdb       |
| `packages/cwr-core/src/desktop_storage/workspace_import.rs`       |               5 | migrated by coordinator; named cwpdb Desktop operations / physical tests moved to cwpdb       |
| `packages/cwr-core/src/desktop_storage/workspace_import_tests.rs` |              15 | migrated by coordinator; named cwpdb Desktop operations / physical tests moved to cwpdb       |
| `packages/cwr-core/src/desktop_storage.rs`                        |               2 | migrated by coordinator; named cwpdb Desktop operations / physical tests moved to cwpdb       |

## Metrics and diagnostics (coordinator-owned migration)

| File                                                      | Surface matches | Status                                                   |
| --------------------------------------------------------- | --------------: | -------------------------------------------------------- |
| `packages/cwr-core/src/diagnostics/resident.rs`           |               2 | migrated by coordinator; typed cwpdb operations/fixtures |
| `packages/cwr-core/src/diagnostics/store.rs`              |               6 | migrated by coordinator; typed cwpdb operations/fixtures |
| `packages/cwr-core/src/metrics/posthog/recovery_tests.rs` |               3 | migrated by coordinator; typed cwpdb operations/fixtures |
| `packages/cwr-core/src/metrics/posthog.rs`                |               1 | migrated by coordinator; typed cwpdb operations/fixtures |
| `packages/cwr-core/src/metrics/sqlite/tests.rs`           |               5 | migrated by coordinator; typed cwpdb operations/fixtures |
| `packages/cwr-core/src/metrics/sqlite.rs`                 |               7 | migrated by coordinator; typed cwpdb operations/fixtures |
| `packages/cwr-core/src/metrics/tests.rs`                  |              13 | migrated by coordinator; typed cwpdb operations/fixtures |

## Declared entities, usage, pins and PostHog

| File                                                                       | Surface matches | Status                                                                                        |
| -------------------------------------------------------------------------- | --------------: | --------------------------------------------------------------------------------------------- |
| `packages/cwr-core/src/entities/disposable.rs`                             |               3 | migrated by coordinator; named native cwpdb domain operations / physical tests moved to cwpdb |
| `packages/cwr-core/src/entities/disposable_tests.rs`                       |              15 | migrated by coordinator; named native cwpdb domain operations / physical tests moved to cwpdb |
| `packages/cwr-core/src/entities/local_usage.rs`                            |               5 | migrated by coordinator; named native cwpdb domain operations / physical tests moved to cwpdb |
| `packages/cwr-core/src/entities/local_usage_tests.rs`                      |              23 | migrated by coordinator; named native cwpdb domain operations / physical tests moved to cwpdb |
| `packages/cwr-core/src/entities/personal_collection_member_order_tests.rs` |               3 | migrated by coordinator; named native cwpdb domain operations / physical tests moved to cwpdb |
| `packages/cwr-core/src/entities/personal_collection_ordering.rs`           |               3 | migrated by coordinator; named native cwpdb domain operations / physical tests moved to cwpdb |
| `packages/cwr-core/src/entities/personal_collections.rs`                   |               1 | migrated by coordinator; named native cwpdb domain operations / physical tests moved to cwpdb |
| `packages/cwr-core/src/entities/personal_collections_tests.rs`             |               5 | migrated by coordinator; named native cwpdb domain operations / physical tests moved to cwpdb |
| `packages/cwr-core/src/entities/posthog/accounting.rs`                     |               2 | migrated by coordinator; named native cwpdb domain operations / physical tests moved to cwpdb |
| `packages/cwr-core/src/entities/posthog/accounting_tests.rs`               |               6 | migrated by coordinator; named native cwpdb domain operations / physical tests moved to cwpdb |
| `packages/cwr-core/src/entities/posthog/admission.rs`                      |               1 | migrated by coordinator; named native cwpdb domain operations / physical tests moved to cwpdb |
| `packages/cwr-core/src/entities/posthog/admission_tests.rs`                |               6 | migrated by coordinator; named native cwpdb domain operations / physical tests moved to cwpdb |
| `packages/cwr-core/src/entities/posthog/architecture_tests.rs`             |              17 | migrated by coordinator; named native cwpdb domain operations / physical tests moved to cwpdb |
| `packages/cwr-core/src/entities/posthog/migration.rs`                      |              13 | migrated by coordinator; named native cwpdb domain operations / physical tests moved to cwpdb |
| `packages/cwr-core/src/entities/posthog/migration_tests.rs`                |              19 | migrated by coordinator; named native cwpdb domain operations / physical tests moved to cwpdb |
| `packages/cwr-core/src/entities/posthog/mutation_tests.rs`                 |               2 | migrated by coordinator; named native cwpdb domain operations / physical tests moved to cwpdb |
| `packages/cwr-core/src/entities/posthog/mutations.rs`                      |               1 | migrated by coordinator; named native cwpdb domain operations / physical tests moved to cwpdb |
| `packages/cwr-core/src/entities/posthog/reader_integration_tests.rs`       |               4 | migrated by coordinator; named native cwpdb domain operations / physical tests moved to cwpdb |
| `packages/cwr-core/src/entities/posthog/reads.rs`                          |               2 | migrated by coordinator; named native cwpdb domain operations / physical tests moved to cwpdb |
| `packages/cwr-core/src/entities/posthog/security.rs`                       |               8 | migrated by coordinator; named native cwpdb domain operations / physical tests moved to cwpdb |
| `packages/cwr-core/src/entities/posthog/startup_validation_tests.rs`       |               5 | migrated by coordinator; named native cwpdb domain operations / physical tests moved to cwpdb |
| `packages/cwr-core/src/entities/posthog.rs`                                |               1 | migrated by coordinator; named native cwpdb domain operations / physical tests moved to cwpdb |
| `packages/cwr-core/src/entities/relationships.rs`                          |               5 | pending                                                                                       |
| `packages/cwr-core/src/entities/sequence.rs`                               |               1 | migrated by coordinator; named native cwpdb domain operations / physical tests moved to cwpdb |
| `packages/cwr-core/src/entities/sequence_accounting.rs`                    |               1 | migrated by coordinator; named native cwpdb domain operations / physical tests moved to cwpdb |
| `packages/cwr-core/src/entities/sequence_tests.rs`                         |               4 | migrated by coordinator; named native cwpdb domain operations / physical tests moved to cwpdb |
| `packages/cwr-core/src/entities/sidebar_pin_move.rs`                       |               1 | migrated by coordinator; named native cwpdb domain operations / physical tests moved to cwpdb |
| `packages/cwr-core/src/entities/sidebar_pin_move_tests.rs`                 |              11 | migrated by coordinator; named native cwpdb domain operations / physical tests moved to cwpdb |
| `packages/cwr-core/src/entities/sidebar_pins.rs`                           |               1 | migrated by coordinator; named native cwpdb domain operations / physical tests moved to cwpdb |
| `packages/cwr-core/src/entities/sidebar_pins_tests.rs`                     |              13 | migrated by coordinator; named native cwpdb domain operations / physical tests moved to cwpdb |
| `packages/cwr-core/src/entities.rs`                                        |               8 | pending                                                                                       |
| `packages/cwr-core/src/entities_lifecycle_tests.rs`                        |               2 | pending                                                                                       |
| `packages/cwr-core/src/entities_tests.rs`                                  |               3 | pending                                                                                       |

## Conversation admission, state, projections and reads

| File                                                                                         | Surface matches | Status  |
| -------------------------------------------------------------------------------------------- | --------------: | ------- |
| `packages/cwr-core/src/v2026_09/conversation/admission.rs`                                   |               1 | pending |
| `packages/cwr-core/src/v2026_09/conversation/files.rs`                                       |               5 | pending |
| `packages/cwr-core/src/v2026_09/conversation/fork.rs`                                        |               2 | pending |
| `packages/cwr-core/src/v2026_09/conversation/invocation/tests/shared_reads/browse_limits.rs` |               3 | pending |
| `packages/cwr-core/src/v2026_09/conversation/invocation/tests/shared_reads.rs`               |               2 | pending |
| `packages/cwr-core/src/v2026_09/conversation/invocation/tests/shared_reads_regressions.rs`   |               1 | pending |
| `packages/cwr-core/src/v2026_09/conversation/invocation/tests/shared_reply_identity.rs`      |               3 | pending |
| `packages/cwr-core/src/v2026_09/conversation/invocation/tests/timeline_contract.rs`          |               4 | pending |
| `packages/cwr-core/src/v2026_09/conversation/invocation/tests/timeline_rollout.rs`           |               2 | pending |
| `packages/cwr-core/src/v2026_09/conversation/invocation/tests/timeline_scaling.rs`           |               3 | pending |
| `packages/cwr-core/src/v2026_09/conversation/invocation/tests/transactions.rs`               |               2 | pending |
| `packages/cwr-core/src/v2026_09/conversation/mod.rs`                                         |               2 | pending |
| `packages/cwr-core/src/v2026_09/conversation/model.rs`                                       |               1 | pending |
| `packages/cwr-core/src/v2026_09/conversation/participation.rs`                               |               1 | pending |
| `packages/cwr-core/src/v2026_09/conversation/provider_question.rs`                           |               1 | pending |
| `packages/cwr-core/src/v2026_09/conversation/read/current/execution.rs`                      |               5 | pending |
| `packages/cwr-core/src/v2026_09/conversation/read/current/reply.rs`                          |               3 | pending |
| `packages/cwr-core/src/v2026_09/conversation/read/current/streaming.rs`                      |               2 | pending |
| `packages/cwr-core/src/v2026_09/conversation/read/current.rs`                                |              12 | pending |
| `packages/cwr-core/src/v2026_09/conversation/read/indexed/canonical_timeline/lookup.rs`      |               5 | pending |
| `packages/cwr-core/src/v2026_09/conversation/read/indexed/canonical_timeline.rs`             |              12 | pending |
| `packages/cwr-core/src/v2026_09/conversation/read/indexed/canonical_visibility.rs`           |               4 | pending |
| `packages/cwr-core/src/v2026_09/conversation/read/indexed/history/decode.rs`                 |               8 | pending |
| `packages/cwr-core/src/v2026_09/conversation/read/indexed/history/queries.rs`                |               4 | pending |
| `packages/cwr-core/src/v2026_09/conversation/read/indexed/history/search.rs`                 |               2 | pending |
| `packages/cwr-core/src/v2026_09/conversation/read/indexed/legacy_timeline.rs`                |               4 | pending |
| `packages/cwr-core/src/v2026_09/conversation/read/indexed/targeting.rs`                      |               1 | pending |
| `packages/cwr-core/src/v2026_09/conversation/read/indexed.rs`                                |              22 | pending |
| `packages/cwr-core/src/v2026_09/conversation/read/indexed_dependency_tests.rs`               |               2 | pending |
| `packages/cwr-core/src/v2026_09/conversation/read/indexed_fixtures.rs`                       |               1 | pending |
| `packages/cwr-core/src/v2026_09/conversation/read/indexed_tests.rs`                          |               3 | pending |
| `packages/cwr-core/src/v2026_09/conversation/read/publication/actors.rs`                     |               4 | pending |
| `packages/cwr-core/src/v2026_09/conversation/read/publication/attachments.rs`                |               5 | pending |
| `packages/cwr-core/src/v2026_09/conversation/read/publication/controls.rs`                   |               4 | pending |
| `packages/cwr-core/src/v2026_09/conversation/read/publication/filtered.rs`                   |               6 | pending |
| `packages/cwr-core/src/v2026_09/conversation/read/publication/inspector/details.rs`          |               7 | pending |
| `packages/cwr-core/src/v2026_09/conversation/read/publication/inspector.rs`                  |               8 | pending |
| `packages/cwr-core/src/v2026_09/conversation/read/publication/metadata.rs`                   |               9 | pending |
| `packages/cwr-core/src/v2026_09/conversation/read/publication.rs`                            |               8 | pending |
| `packages/cwr-core/src/v2026_09/conversation/read/summaries.rs`                              |              12 | pending |
| `packages/cwr-core/src/v2026_09/conversation/read/summary_revisions.rs`                      |               4 | pending |
| `packages/cwr-core/src/v2026_09/conversation/read.rs`                                        |               6 | pending |
| `packages/cwr-core/src/v2026_09/conversation/read_tests/summary_tests.rs`                    |               3 | pending |
| `packages/cwr-core/src/v2026_09/conversation/read_tests.rs`                                  |              16 | pending |
| `packages/cwr-core/src/v2026_09/conversation/sqlite/admissions.rs`                           |               4 | pending |
| `packages/cwr-core/src/v2026_09/conversation/sqlite/commentary.rs`                           |               1 | pending |
| `packages/cwr-core/src/v2026_09/conversation/sqlite/completion_order.rs`                     |               7 | pending |
| `packages/cwr-core/src/v2026_09/conversation/sqlite/conversation_records.rs`                 |               8 | pending |
| `packages/cwr-core/src/v2026_09/conversation/sqlite/conversation_records_tests.rs`           |               1 | pending |
| `packages/cwr-core/src/v2026_09/conversation/sqlite/declared.rs`                             |               3 | pending |
| `packages/cwr-core/src/v2026_09/conversation/sqlite/declared_ownership_tests.rs`             |               2 | pending |
| `packages/cwr-core/src/v2026_09/conversation/sqlite/deferred.rs`                             |              10 | pending |
| `packages/cwr-core/src/v2026_09/conversation/sqlite/deferred_history_tests.rs`               |               1 | pending |
| `packages/cwr-core/src/v2026_09/conversation/sqlite/identity_owners.rs`                      |               1 | pending |
| `packages/cwr-core/src/v2026_09/conversation/sqlite/load_state.rs`                           |               3 | pending |
| `packages/cwr-core/src/v2026_09/conversation/sqlite/message_boundary.rs`                     |               2 | pending |
| `packages/cwr-core/src/v2026_09/conversation/sqlite/participant_versions.rs`                 |               1 | pending |
| `packages/cwr-core/src/v2026_09/conversation/sqlite/relationships.rs`                        |              13 | pending |
| `packages/cwr-core/src/v2026_09/conversation/sqlite/schema.rs`                               |              12 | pending |
| `packages/cwr-core/src/v2026_09/conversation/sqlite/streaming.rs`                            |               2 | pending |
| `packages/cwr-core/src/v2026_09/conversation/sqlite/sync.rs`                                 |               7 | pending |
| `packages/cwr-core/src/v2026_09/conversation/sqlite/timeline.rs`                             |               3 | pending |
| `packages/cwr-core/src/v2026_09/conversation/sqlite/timeline_changes.rs`                     |               2 | pending |
| `packages/cwr-core/src/v2026_09/conversation/sqlite/timeline_store.rs`                       |               6 | pending |
| `packages/cwr-core/src/v2026_09/conversation/sqlite/tools.rs`                                |               3 | pending |
| `packages/cwr-core/src/v2026_09/conversation/sqlite/tools_tests.rs`                          |               1 | pending |
| `packages/cwr-core/src/v2026_09/conversation/sqlite.rs`                                      |               7 | pending |
| `packages/cwr-core/src/v2026_09/conversation/sqlite_upgrade_tests.rs`                        |              23 | pending |
| `packages/cwr-core/src/v2026_09/conversation/tests/addressing.rs`                            |               6 | pending |
| `packages/cwr-core/src/v2026_09/conversation/tests/context_snapshots.rs`                     |               1 | pending |
| `packages/cwr-core/src/v2026_09/conversation/tests/fork_storage_compatibility.rs`            |               4 | pending |
| `packages/cwr-core/src/v2026_09/conversation/tests/inspector.rs`                             |               2 | pending |
| `packages/cwr-core/src/v2026_09/conversation/tests/message_boundary_lookup.rs`               |               3 | pending |
| `packages/cwr-core/src/v2026_09/conversation/tests/ownership_admission.rs`                   |               1 | pending |
| `packages/cwr-core/src/v2026_09/conversation/tests/retained_timeline_persistence.rs`         |              10 | pending |
| `packages/cwr-core/src/v2026_09/conversation/tests/revisions.rs`                             |               1 | pending |
| `packages/cwr-core/src/v2026_09/conversation/tests/streaming.rs`                             |               2 | pending |
| `packages/cwr-core/src/v2026_09/conversation/tests/timeline_persistence.rs`                  |              33 | pending |
| `packages/cwr-core/src/v2026_09/conversation/tests/titles.rs`                                |               3 | pending |
| `packages/cwr-core/src/v2026_09/conversation/tests/working/selective.rs`                     |              12 | pending |
| `packages/cwr-core/src/v2026_09/conversation/tests/working/selective_stopped.rs`             |               1 | pending |
| `packages/cwr-core/src/v2026_09/conversation/tests/working/tools.rs`                         |               1 | pending |
| `packages/cwr-core/src/v2026_09/conversation/tests/working.rs`                               |               1 | pending |
| `packages/cwr-core/src/v2026_09/conversation/tests.rs`                                       |               2 | pending |
| `packages/cwr-core/src/v2026_09/conversation/working/progress.rs`                            |               2 | pending |

## Other CWR callers and fixtures

| File                                                                      | Surface matches | Status  |
| ------------------------------------------------------------------------- | --------------: | ------- |
| `packages/cwr-core/src/v2026_09/execution/tests/legacy.rs`                |               4 | pending |
| `packages/cwr-core/src/v2026_09/graph/schema.rs`                          |               1 | pending |
| `packages/cwr-core/src/v2026_09/local/owned_action.rs`                    |               1 | pending |
| `packages/cwr-core/src/v2026_09/local/runtime.rs`                         |               1 | pending |
| `packages/cwr-core/src/v2026_09/local/tests/archive_lifecycle.rs`         |               3 | pending |
| `packages/cwr-core/src/v2026_09/local/tests/archive_recovery.rs`          |               1 | pending |
| `packages/cwr-core/src/v2026_09/local/tests/availability_boundary.rs`     |               1 | pending |
| `packages/cwr-core/src/v2026_09/local/tests/conversation_reads.rs`        |               5 | pending |
| `packages/cwr-core/src/v2026_09/local/tests/metrics.rs`                   |               1 | pending |
| `packages/cwr-core/src/v2026_09/local/tests/schedule_occurrence_reads.rs` |               7 | pending |
| `packages/cwr-core/src/v2026_09/local/workspace/composition.rs`           |               2 | pending |
| `packages/cwr-core/src/v2026_09/local/workspace/prepared.rs`              |               1 | pending |
| `packages/cwr-core/src/v2026_09/startup_profile.rs`                       |               2 | pending |

## Retained scheduling state (no feature expansion)

| File                                                         | Surface matches | Status  |
| ------------------------------------------------------------ | --------------: | ------- |
| `packages/cwr-core/src/v2026_09/local/schedules/controls.rs` |               7 | pending |
| `packages/cwr-core/src/v2026_09/local/schedules/storage.rs`  |               7 | pending |
| `packages/cwr-core/src/v2026_09/local/schedules/worker.rs`   |              11 | pending |

## Workspace environment and secret cleanup

| File                                                               | Surface matches | Status                                                                                    |
| ------------------------------------------------------------------ | --------------: | ----------------------------------------------------------------------------------------- |
| `packages/cwr-core/src/workspace_environment/declared_secret.rs`   |               1 | migrated to cwpdb environment operations; 24 CWR tests pass, native Keychain test ignored |
| `packages/cwr-core/src/workspace_environment/declared_variable.rs` |               1 | migrated to cwpdb environment operations; 24 CWR tests pass, native Keychain test ignored |
| `packages/cwr-core/src/workspace_environment/entry_read.rs`        |               1 | migrated to cwpdb environment operations; 24 CWR tests pass, native Keychain test ignored |
| `packages/cwr-core/src/workspace_environment/recovery.rs`          |               4 | migrated to cwpdb environment operations; 24 CWR tests pass, native Keychain test ignored |
| `packages/cwr-core/src/workspace_environment/report_tests.rs`      |               2 | migrated to cwpdb environment operations; 24 CWR tests pass, native Keychain test ignored |
| `packages/cwr-core/src/workspace_environment/review_tests.rs`      |               1 | migrated to cwpdb environment operations; 24 CWR tests pass, native Keychain test ignored |
| `packages/cwr-core/src/workspace_environment/storage.rs`           |               4 | migrated to cwpdb environment operations; 24 CWR tests pass, native Keychain test ignored |
| `packages/cwr-core/src/workspace_environment/tests.rs`             |               2 | migrated to cwpdb environment operations; 24 CWR tests pass, native Keychain test ignored |
| `packages/cwr-core/src/workspace_environment/upgrade_tests.rs`     |               1 | migrated to cwpdb environment operations; 24 CWR tests pass, native Keychain test ignored |
| `packages/cwr-core/src/workspace_environment.rs`                   |               7 | migrated to cwpdb environment operations; 24 CWR tests pass, native Keychain test ignored |

## Application consumers beyond the Rust baseline

- `apps/cwp-runtime/tests/{host_stdio,startup_diagnostics}.rs`: migrated to
  typed diagnostic pages; startup diagnostic check passes.
- `apps/workloop-desktop/invocation_performance_worker.ts`: migrated to named
  `cwpdb_invocation_performance` FFI; SQL and narrow query-plan regression now
  storage-owned. Four actual Deno worker checks pass, including
  a30,050-observation fixture, no-create/read-only behavior and replacement
  database.
- `apps/workloop-desktop/invocation_performance_query.ts`: removed; physical
  query is `packages/cwpdb/src/metrics/projection.sql`.
- Desktop private application metadata still uses the existing generic WlpDb
  transport. Review those consumers individually against the requested CWR
  migration; this is not a claim that all SQL in the entire monorepo is removed.
  `output/workloop-cloud-integration/non-rust-sql-inventory.txt` records the
  current integration scan.

Native facade signatures still select NativeStore in the migrated Desktop
domain; common hosted port selection, full canonical conversation binding,
remaining entity/usage/schedule operations and public raw API retirement remain
unfinished. Native physical tests moving to cwpdb does not establish hosted
equivalence.

Coordinator native increment: all Desktop operation wrappers, telemetry outbox,
usage, pins, collections, sequence/disposable mutation details and physical
lifecycle tests now live behind cwpdb. The complete integration cwpdb suite
passes160 tests (one subprocess-only helper ignored); CWR core1235
pass/7ignored. CWR declaration/schema/format validation remains in EntityCatalog
and its pure ValueValidator. Generic legacy transaction methods in
EntityCatalog, relationship SQL facts adapters, old conversation/indexed readers
and schedule consumers remain until their owning port migrations finish. These
native checks do not establish full removal or hosted acceptance.
