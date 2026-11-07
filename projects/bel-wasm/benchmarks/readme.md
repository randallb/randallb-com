# Performance evidence for rbel

These small benchmarks answer “did this change make things faster or allocate
more?” They measure the actual source compiler and ABI2 runtime. Timing is
informational; correctness and rooting failures stop the run. CI has no timing
threshold because shared runner load changes between runs.

From `projects/bel-wasm`, after building the native command or portable bootstrap:

```sh
python3 benchmarks/run.py --native --output target/benchmarks/reference
python3 benchmarks/run.py --native --output target/benchmarks/candidate \
  --compare target/benchmarks/reference/report.json
```

Omit `--native` for portable Wasm-only measurement. Select the portable compiler
with `--rbel target/rbel-wasm-bootstrap/bin/rbel`. `--case tail-direct` selects a
case; repeat the option to choose several in order. Default sampling uses two
warmup instances, seven measured instances, and three calls per instance.
`--iterations 30` gives longer batches when scheduler/timer noise matters, but
uses a different comparison protocol. Keep the machine otherwise idle for local
comparisons. Run both revisions more than once before interpreting small changes.

Reports go under ignored `target`, outside the compiler bundle. The runner writes
`report.json` with raw samples and provenance and `report.md` with a compact table.
The comparison baseline is read-only. Every output alias is checked before
clearing prior evidence. Semantic/build/execution failures leave no success
report. A compiler bundle or output-alias validation failure preserves prior files.

## Workloads and measurements

| Workload | One measured call |
| --- | --- |
| tail-direct | Traverse 10,000 nil cells by direct tail recursion |
| tail-apply | Traverse the same input using first-class `apply` |
| allocation-churn | Traverse that input while dropping two fresh pairs each step |
| list-reverse | Reverse 1,024 alternating symbols |
| exact-arithmetic | Multiply two 15/16-digit integers, add, and print the exact result |
| utf8-roundtrip | Encode and decode the three scalars `hé😉` using the rbel library |

The source dispatcher, library operations and loops are compiled from
[workloads.rbel](workloads.rbel). Hosts only create inputs, invoke code, time it,
and validate the complete expected results. Each call is checked outside the
execution timer. Validation uses a nonallocating symbol-name view, so it does not
inflate language allocation counters. Errors and root leaks fail the benchmark.

The phase profile separates source-to-WAT wall time, assembly, Wasm module
compilation, and measured execution. JavaScript samples also retain initialization,
input encoding and result-validation times. Native reports currently time
execution only; native translation and C compilation have separate build times.
Build times are single cold observations, not repeated compilation benchmarks.
This is a coarse profile, not a per-function CPU sampler or GC pause profiler.

Runtime snapshots record collections, reclaimed/live cells, tail transfers,
live symbols, host roots, maximum root/call depth and configured total linear
memory. The live-cell counter means allocated, unreclaimed cells; unreachable
garbage stays counted until a collection. Allocated cells are derived exactly as `change in live + reclaimed`;
bytes allocated are eight times that count. This does not measure host/Node RSS.
High-water root/call values describe the whole instance, including initialization
and input encoding. The runtime has no resettable execution-only high-water counters.

Each warmup and measured sample gets a fresh initialized instance. Inputs and
argument lists are retained, followed by an explicit preparation collection.
Its time and counters are excluded. Repeated calls in a sample then include
all automatic GC and preserve the same input graph; host setup does not accumulate
across samples. Wasm uses a shared compiled module; warmups also warm that engine.
A single module contains all workloads and required libraries, so this does not
represent the smallest possible executable for each case.

The configured heap is 32,768 pair cells, 65,536 root slots, 4,096 host roots,
1,024 symbol descriptors and 128 non-tail calls. This intentionally collects
frequently and exposes allocation pressure. Real applications with larger heaps
will have different collection rates and timings. Compare like configurations.
The Wasm and native runners use the same settings and validate identical outputs.
Native wasm2c uses production safety/stack flags and disables C sibling-call
optimization; its timing is a separate backend series, not a promise that native
must beat V8.

## Comparing revisions

A comparison reports median milliseconds per call and median absolute deviation
(MAD), minimum, maximum, allocations and collections per call. Percent changes are
labelled only when workload, harness, ordered cases, sampling settings, heap,
engine and hardware/OS metadata match. Native comparisons also require matching C
compiler and flags. Positive change means slower. An incompatible report stays
visible as `incompatible`; it never becomes a misleading percentage or CI failure.

Compiler and supporting-library hashes are provenance, not compatibility gates:
those are the implementations we want to optimize. Workload and measurement
harness changes require a new baseline. JSON retains both sides' identity through
the baseline file and the current compiler/module/source hashes. Timings alone do
not distinguish CPU frequency, contention or thermal changes on the same host.

The [initial local reference](baselines/macos-arm64.json) is an example and
starting point for that recorded machine/protocol, not a cross-machine target.
It already identifies a useful optimization lead: exact arithmetic and even a
three-character UTF-8 round trip create many temporary pairs. Use the counters
and repeated timings to verify an improvement before adding invasive profiling.
