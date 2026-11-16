# Exact conversation summary port

Ada recovery increment, November2,2026. This is a finite current exact-ref read, not a CWR history mirror or a cached permission decision.

`ConversationStorage::summary(scope, reference, authorizer)` returns `Option<SummaryFacts>`. The native implementation reads one SQLite snapshot; Worker uses one DO transaction; trusted transport repeats authorization before and after the result. Facts expose current root membership, parent, sidebar eligibility, nullable title/owner, current participants, storage-owned creation order, visible preview identity, visible completions and an opaque semantic revision. CWR applies root/sidebar/parent/participant/owner policy to these same-snapshot facts.

Canonical visibility follows nested authoritative timeline placements. Detached raw messages and unfinished streaming messages are excluded. Completion facts include only invocations reachable through the selected canonical root. The latest500 visible completion refs are retained for compatibility, while completed_work carries only the latest visible completion with its sequence. With no canonical root, existing retained scalar preview/completion behavior remains unchanged.

For canonical previews, last_message_record is the plain selected message record (`GraphRecord.properties.record`) with retained visualization metadata composed from the owning message relationship. CWR must deserialize this one record, check its message_ref, and render `nonvisual_text().chars().take(240)` itself. The backend plain-text preview is useful only before protocol rendering. No Markdown/presentation engine was moved into storage.

Revision is a digest of visible summary facts and selected message text/visualization metadata, excluding global commit clocks, streaming progress and message progress fields. Thus unrelated/progress-only writes leave the summary revision stable while visible title, preview, completion, membership, owner or archive changes advance it. Creation order uses the existing derived root position/storage-owned header clock, never a newly supplied membership position.

Work is restricted to the selected conversation/root, with50,000 relationship/placement and128-depth guards and the existing24MiB response ceiling. Guards return an error rather than a guessed/truncated summary. No other conversation history is hydrated into CWR.

Integration: copy only manifest.owned_files and apply shared-summary.patch. The patch adds DTO export/trait method, native/remote forwarding, Worker types/method/dispatch and fixture support. It preserves integrated schedules, identity lookup and coordinator-owned modules. This storage checkout copied current coordinator profiling/Cargo trace dependencies only for local validation; those dependency copies are excluded from the packet.

Evidence: ten same cases pass native, actual local workerd DO and Rust remote→the same DO, including canonical nesting, streaming/detachment hiding, only-visible completion, nullable title, root/child facts, current participants/owner, missing refs and progress-stable semantic revisions. The native digest is also checked against the Worker canonical digest algorithm. cwpdb all-target check/Clippy and Worker typecheck/lint pass. Kit separately verified CWR source authorization against current participant/owner changes with the candidate native port. No hosted customer/model/provider acceptance or formal review convergence is claimed.

Pending follow-up: native initializer parity (unchanged open avoids immediate write locks; unsupported user_version fails before writes), and remaining test/readonly SQL adapter migration. No merge or deployment was performed by Ada.
