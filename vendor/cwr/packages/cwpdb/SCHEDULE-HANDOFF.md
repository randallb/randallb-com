# Canonical schedule storage recovery increment

November 2, 2026. Ada's existing `workloop-cloud-storage` checkout continues from the retained native schedule migration; this is not a new implementation kickoff. No formal d3l3r3x3 review cycle has begun.

CWR still owns declaration validation, installed-context authority, due-time/overlap decisions and ordinary invocation admission. cwpdb owns typed installed/occurrence records, opaque revision CAS, immutable identities, atomic reservation and execution links. No SQL, graph transaction or physical handle crosses the ScheduleStorage port.

Native schema/index upgrades already exist in the coordinator and are preserved. Existing schedules retain their exact canonical refs and stored records. Native recent/archive reads keep indexed query plans even after more than20,000 retained occurrences. The native fixture now seeds and checks that history inside cwpdb rather than exporting a transaction to CWR tests.

RemoteSchedules<T> takes a trusted DeclaredTransport and invocation::Context. The private Worker dispatch uses `operation:"schedule"`, context and a finite `schedule_request` union. Existing token and workspace binding checks run first; the public Worker remains404. Schedule operations repeat owner scope and expected revisions, reject incompatible declaration revision reuse, preserve active occurrence when a declaration changes, refuse endpoint/terminal/archive regressions and rollback entire reservation/batch operations on failure. No Google/model/provider credentials enter this port.

Recent reads retain the latest100 occurrences with native-compatible due-time and retained-insertion ordering. Unarchived terminal occurrences and owner/schedule indexes cap20,000 active entries, independently of unlimited archived history; operations fail atomically when the active work limit is exhausted. Millisecond times must fit the shared safe integer range. Mutation requests cap512KiB, response pages24MiB.

The CWR wrapper selects the same trusted environment as Kit: CWR_CONVERSATION_STORAGE_URL, CWR_CONVERSATION_STORAGE_TOKEN, CWR_CONVERSATION_WORKSPACE_REF and CWR_CONVERSATION_CALLER_REF. CWR_REQUIRE_REMOTE_STATE=1/true refuses native fallback. There is no fallback after a configured remote operation fails. This checkout contains dependency-only Cloud transport/module copies to compile the wrapper. Preserve the coordinator's existing Cloud module and Kit's full implementation instead of copying these dependency files.

Integration: copy only the packet's owned_files, then apply shared-integration.patch after checking the current shared modules. It adds schedules to cwpdb lib.rs, Deserialize to opaque RefRevision, the Worker dispatcher branch, and the schedule line-protocol fixture hook. Do not overwrite the coordinator's lib.rs/Cloud module, Worker changes from other owners, or existing fixture_worker.ts duplicate-field fix. Packages/cwpdb/worker/fixture_worker.ts is not in this packet.

Evidence lives in packet evidence/: schedule-recovery-core-tests.log (31 native CWR regressions), schedule-conformance-recovery.log (same15 checks pass native, actual local workerd, Rust RemoteSchedules to the same workerd), schedule-recovery-final-clippy.log, schedule-hosted-check.log and schedule-hosted-lint.log. Full composed hosted CWR schedule behavior still requires Kit's backend/runtime composition and integration verification. No hosted resources, real models/providers, customer actions, deployment, merge or complete acceptance are claimed.

Next storage work is finite EntityCatalog/relationship fact extraction, coordinated with Kit's startup/indexed reader migration. All remaining generic SQL/native handles remain open until the full inventory is clear.
