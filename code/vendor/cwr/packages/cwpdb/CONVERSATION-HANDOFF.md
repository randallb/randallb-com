# Canonical conversation storage checkpoint v2

Ada storage checkout: `/Users/randallb/code/getworkloop/wlp-workspaces/wlp-mono--workloop-cloud-storage`, base `a717eae070115796b0daf8e20e1321093aa385bb`. This increment is ready for integration. It does not claim full CWR consumer migration, formal review, real hosted deployment or final Superbuild acceptance.

## Declared contract

`src/conversation.rs` owns typed canonical graph DTOs. CWR keeps protocol validation, action admission and authoritative projection. cwpdb owns records, immutable identity/ownership bindings, indexes, conditional commits and retry receipts. Canonical history is stored once; changed records, delta memberships and semantic publication suffixes cross the port. A progress activity is separate from Message|Turn anchors.

`ConversationStorage` declares read/get/commit/reconcile plus finite retained_order and published_progress_conversation lookups. Read selection includes Bootstrap, a cold Conversation, finite Records kind, first-message ConversationMetadata and exact provider Commentary. Retained order is a backend-owned comparison ordinal; native preserves existing retained ordering. It is not an address or a general transaction handle. All operations recheck the trusted Authorizer; context is never authentication.

`Changes.timeline_rollout: Option<String>` accepts disabled/new_conversations/migrate_legacy monotonically and checks repeated live placements on activation. `Changes.context_policy: Option<Value>` stores the validated opaque CWR wrapper, including explicit nulls. It is returned in Bootstrap.metadata. Core owns authenticated policy validation and one-time legacy-file import. No native workspace path crosses hosted initialization.

Optional JournalCommit and current source AuthorityGuards commit the execution event/receipt with ordinary CWR publication atomically. Failure in later graph work rolls everything back. Independently committed execution events require reconciliation/reload, never duplicate publication.

## Common limits

Native, Worker and Rust RemoteStore share a24MiB serialized mutation/page ceiling. Resulting record properties reserve16KiB for graph framing. Page accounting includes the complete SnapshotPage framing and continuation allowance. Requests over the mutation budget fail before storage work; RemoteStore rejects before transport. A requested32MiB page fails finitely. Chunked private KV records also cap24MiB. Existing10MiB retained files (20MiB hex) remain intact. This leaves headroom under the official [Workers32MiB RPC limit](https://developers.cloudflare.com/workers/runtime-apis/rpc/#limitations).

## Native maintenance and tests

Bootstrap.metadata.storage_maintenance reports finite flags message_boundary, visualization_metadata, identity_owners, timeline_projection and completion_order. Clean snapshots remain cold. CWR hydrates/validates legacy state once when flagged and supplies canonical projection deltas with repair_legacy:true. The backend repairs owned boundary/progress field indexes, moves legacy visualization metadata to its message relationship preserving extensions, repairs claims and completion facts, and updates the projection layout version. It refuses to clear a stale projection version without canonical root suffixes from position0. One-time open normalization removes only the legacy projected WorkUpdate attachment and bumps the root revision once.

Kit owns core backend selection, pure protocol decode/deltas, authoritative projection and indexed reader migration. Ada owns cwpdb/native/Worker parity, repairs and schedules. Test-only publication audit is Kit's finite port: real TEMP triggers on the owning native connection count physical message/anchor/activity writes. No raw connection crosses the test boundary.

## Exact integration files

Copy the full `src/conversation.rs` and `src/conversation/` tree (Rust plus SQL). They include Kit's released opaque/semantic/provider-child parity fixes and merged test audit. Copy `src/remote.rs` (new lookups, budget checks and ConversationStorage implementation). Preserve coordinator lib.rs modules and Cargo changes; `pub mod conversation;` and runtime receipt-expiry hook in lib.rs, invocation mutate_in_transaction helper and sequence_receipt tip visibility were supplied in v1 and are already required. They remain reference-only dependencies in the packet.

Copy new Worker files conversation_types.ts, conversation_graph.ts, conversation_write.ts, conversation_publications.ts, conversation_read.ts, conversation_maintenance.ts, conversations.ts, identity.ts, conversation_conformance.ts, conversation_fixture_cases.ts and conformance_process.ts. Copy updated worker.ts, fixture_worker.ts, kv.ts and engine.ts. engine.ts now shares immutable canonical bindings across document/conversation domains, preventing the same canonical ref from becoming both a document and CWR record. Production HTTP stays404. Local fixture retains the http/loopback-only guard.

Copy new `examples/conversation_fixture.rs`. It supports native reopen and the same Rust RemoteStore, with loopback-only fictional fixture transport. Existing storage_fixture remains supported.

## Evidence

- `output/conversation-suite.log`:67 native cwpdb tests pass, including10 conversation tests.
- `output/conversation-clippy.log`: all-targets Clippy, test-support, -D warnings passes.
- `output/hosted-conversation-{check,lint}.log`: Deno typecheck and lint pass.
- `output/conversation-conformance.log`:20 common checks pass in each of native canonical graph, actual local workerd Durable Object, and Rust RemoteStore to that same workerd.
- Checks include cold paging/reopen; opaque nulls; first-message metadata; exact commentary; binding-only actors; Message|Turn vs projected activities; CAS; atomic journal rollback and joint replay; receipt erasure; current source guard;10MiB file; legacy layout repair; cross-domain identity collision;26MiB two-record atomic refusal; intact20+6MiB file paging within actual24MiB serialized pages; monotonic rollout; retained context policy; retained order/progress lookup.

The document/journal shared suite is rerun separately after the identity/limit changes. The task-owned workerd fixture currently runs127.0.0.1:8797 with fictional data and private persisted fixture directory; no live resource is created.

## Open scope

Normal real CWR hosted backend selection and remaining indexed reader ports are being integrated by Kit. Schedules and remaining consumers/raw exported APIs still require full migration. Coordinator owns entities/Desktop/metrics/diagnostics; do not overwrite those changes with this older checkout. Native-only entity/environment increments do not prove hosted equivalence for those domains. No merge, deployment or customer/provider spend is authorized by this checkpoint.
