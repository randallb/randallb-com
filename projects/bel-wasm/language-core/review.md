# Language core implementation and review record

Owner: Randall Bennett. Integration: Assistant, November 7, 2026.
Scope: the seven-feature commission documented in [the implementation guide](readme.md).
Base: numeric candidate `d38c27f`, draft PR #11. No merge or release is authorized.
This record describes this language commission; it does not reset or complete
review cycles recorded for earlier libcwr/application milestones.

## Design and independent work

Astra (`gpt-6-astra`, maximum reasoning effort) produced the preserved design
before implementation. Separate agents implemented compiler lowering, runtime,
and source modules. Parent integrated the bootstrap, native/JavaScript adapters,
publication, UTF-8 chapter and full command paths. Review claims below identify
the independently reviewed boundary; an implementer's own tests are not counted
as independent approval.

- `numeric_library` independently reviewed runtime and compiler behavior, then
  module hygiene using the actual linker and actual compiler. Compiler findings
  were fixed: statically known arity failures in dead code; a last-result handoff
  keeping discarded temporaries alive; and public entry root-exhaustion returning
  a success status. Runtime review fixed odd heap-capacity alignment and capped
  unsafe native non-tail call configurations. Tight heap, stale-generation,
  interrupted interning, boundary Unicode, evaluation order and rest-tail
  identity cases were independently exercised.
- `runtime_core` independently reviewed parent host changes, excluding its own
  runtime implementation. It verified explicit instance epochs against same-address
  and same-token native instance reuse, 140 successive lifetimes, large-array
  codec collection and a persistent native filter over 8193 chunks. No remaining
  findings were reported after the host-lifetime and compiler-capacity fixes.
- `numeric_reader` independently tested the UTF-8 library with 36 actual ABI2
  cases, including malformed byte sequences, scalar boundaries and pair cycles.
  A missing source parenthesis was corrected before acceptance.
- `numeric_library` reviewed final build/CLI integration with real native and
  portable commands: module check/build/emit/read, native execution with empty
  PATH, arbitrary bytes, invalid UTF-8 prose, transitive hard-link aliases,
  bundle manifest and component hashes. Shipped-source reproduction passed with an empty PATH. Portable example
  checking and guarded core reuse received a separate focused review.

Evidence is retained under ignored `target/{compiler-independent-review,
module-independent-review,runtime-review,runtime-core,integration-independent-review}`.
The focused final integration review independently reran all 14 portable command
groups and rejected stale component/composition/ABI/binary/WAT reuse before
publication. It found no remaining actionable issues. The multiple-body test
wrapper update and implementation guide were also checked.
No hosted human/security review is implied by local agents or CI.

## Maintained local acceptance

- Runtime: 14 groups cover collection, host retention, weak symbols, cycles,
  recoverable resource limits, tail dispatch, diagnostics, generations and UTF-8.
- Compiler: 28 Wasm groups and six rejected invalid programs; four native
  million-step tail-loop forms and recoverable non-tail exhaustion. Native tests
  disable sibling-call optimization and keep the physical WABT stack bound.
- Bootstrap: stage zero and three successors have identical Wasm bytes;
  12 fixture outputs, compiler mutation detection, 27 rejection/cleanup cases.
- Source modules: 25 native/portable groups cover private exports, diamonds,
  cycles, hygiene, qualified primitives, source lines and raw controls.
- Chapters: 15 core, 19 combined UTF-8/integer and three module examples pass.
  The portable test wrapper accepts multiple-body definitions and independently
  runs the core and UTF-8 chapters through the packaged Wasm transport.
- Native command: reader comparisons/rejections, relocated bundle with forbidden
  script tools, standalone filters, failed publication/alias recovery, deterministic
  compiler self-emission and canonical packaged-core WAT equality pass.
- Portable command: 14 relocated-package publication groups pass, including
  bad imported examples, recovery, strict UTF-8, and aliases. Core reuse checks
  ABI/version, every current source component and the verified Wasm hash.
- Native-seeded rebuild: canonical core Wasm is byte-identical to the Wasm
  bootstrap; the installed compiler hash stays unchanged.
- libcwr: maintained workflow and all 31 native-inclusive verification groups
  pass, including deliberate failures and foundation/reader regression suites.
- Embedding: all 26 groups pass with official Node 26.12.0 SEA packaging, real
  native executable relocation, lifecycle, persistence, restart and recovery.

Reports retain exact source and generated artifact hashes. The final acceptance
snapshot in `acceptance.json` identifies the reviewed sources and relevant report
hashes. Linux hosted evidence is recorded only after the draft PR checks finish.

## Limits

ABI2 requires rebuilding host adapters and compiled modules together. Numeric
persistence migration remains an explicit application task. Diagnostics identify
containing forms and functions; they do not promise expression-level columns.
Collection is stop-the-world; latency tuning and generational collection are
future work. Native distribution acceptance is macOS ARM64; portable Linux CI
covers Wasm. No OS APIs, finalizers, application-preview deployment or release
artifacts are added to the commissioned scope.

## First Linux hosted run

Run `42344260009` passed clean source bootstrap, self-hosting and portable command
publication. It then exposed a verifier default pointing at the macOS-only
assembler directory. The runtime verifier now defaults to the existing portable
WABT installation; all 14 groups passed again with that exact default. This
changes test-tool selection, not emitted runtime behavior. Hosted acceptance of
the complete corrected candidate remains pending.
