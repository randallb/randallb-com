# Benchmark review and acceptance

Assistant integration, November 7, 2026. Base: language-core `c400691` (draft #13).
This follow-on adds measurement tooling; the language runtime/compiler are unchanged.

Local acceptance: two full runs, each with six native and six Wasm rows, two
warmups, seven samples and three calls per sample. Every call validates the full
expected result. The repeat identifies every row as comparable, retains the exact
baseline SHA, and reports variation without a timing gate. Execution allocation
counts agree across the two backends. Churn actually collects; retained argument
roots stay balanced at one host root and zero shadow roots, then release completely.

Independent reviewer `benchmark_review` inspected the final frozen methods and
executed output/source/baseline alias, failed-report cleanup and comparison tests.
An actual source mutation from `done` to `wrong` failed Wasm result validation and
produced no success JSON. Six actual CLI guard cases and six compatibility cases
passed; changing harness, ordered cases, heap or workload invalidates comparison,
while changing a supporting implementation library remains measurable. Earlier
findings about generated-output aliases, stale human reports, missing harness
fingerprints and module-load timing were resolved and independently retested.

The native runner's author separately ran all workloads and invalid arguments;
that author's own tests are not counted as independent implementation review.
Parent integrated the phase profile, input setup, explicit preparation collection,
nonallocating validation, source snapshots, comparison metadata and CI smoke.
Native phase timing currently covers execution; its compilation has separate
wall-time observations. The JS runner also records instance/input/validation phases.

Evidence is retained in ignored `target/benchmark-independent-review` and
`target/benchmarks/{reference,repeat,native-agent-check}`. The checked-in
[reference report](baselines/macos-arm64.json) includes full raw samples,
source/compiler/module/method hashes and machine/protocol metadata. The guide
states small-heap, unreclaimed-cell, whole-instance high-water, contention and
cross-machine limits. No per-function CPU or GC-pause attribution is claimed.

CI runs all six portable workloads for result/root correctness and retains
reports without timing thresholds. Hosted result is tracked in the draft PR;
local review alone does not establish hosted acceptance. No merge or release.
