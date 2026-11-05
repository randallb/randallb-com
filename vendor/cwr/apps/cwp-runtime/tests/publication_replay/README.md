# Runtime publication replay

From the repository root, run:

```sh
direnv exec . python3 scripts/runtime_publication_replay.py --output /tmp/publication-baseline
```

Use a new or empty output directory. The command builds the real resident and an
ignored integration harness with the locked dependencies, starts synthetic
fixture workspaces, and measures activity through the real scheduler and GraphQL
subscription. No provider account, installed application, or running user
workspace is needed.

The default matrix repeats each configuration three times, with one and four
active workers. For each worker count it runs a zero-history baseline, then 64
completed tool activities per active invocation, then 32 unrelated retained
conversations containing eight messages each. Relevant and unrelated history
change independently. All workers are admitted in separate conversations.

For a smaller smoke run:

```sh
direnv exec . python3 scripts/runtime_publication_replay.py \
  --output /tmp/publication-smoke --workers 1 --relevant-history 0 \
  --unrelated-conversations 0 --repeats 1
```

`--workers`, `--relevant-history`, and `--unrelated-conversations` accept
comma-separated integers. Both history lists must include zero. Workers must be
between 1 and 16. `--unrelated-messages` and `--repeats` accept positive
integers. Use `--profile release` for a release build; the default is Cargo's
development profile. Compare runs using the same profile, source workload, and
machine load. Custom Cargo profile or compiler overrides are flagged in metadata
by variable name; their values are not saved. Avoid overrides for reproducible
comparisons.

## Workload and timing boundary

Each worker emits these correlated events in this order:

1. Published progress.
2. Tool activity started.
3. Streaming text.
4. Tool activity completed.
5. Successful terminal result.

The fixture first writes completed tool activities to each active invocation to
establish relevant worker history, and seeds unaddressed unrelated messages
through public GraphQL mutations before admitting the measured workers. The
content and provider output are synthetic.

For each phase, the harness captures the trigger immediately before writing one
release file shared by all workers. Workers run concurrently within that phase.
The next phase waits until every worker's marker has reached the GraphQL
subscriber. These barriers keep deliberate subscription snapshot coalescing from
hiding intermediate samples. This is a controlled concurrent workload; it does
not measure unpaced producer throughput.

Each sample measures release-file trigger to first correlated subscriber
observation. That interval includes file polling, adapter work, runtime
scheduling/publication, transport, and GraphQL selection work. It does not
isolate publication-lock wait or hold time, CPU time, host application time, or
native rendering. UTC nanosecond timestamps identify the observations, while
latency uses the harness's monotonic clock.

## Results and isolation

`metadata.json` records the exact source revision and pending paths, source and
binary SHA-256 hashes, build command/profile/toolchain, workload, and machine
load snapshots including the busiest executable names. It omits tokens, provider
continuations, and inherited environment values. `build.log` and `replay.log`
retain command output.

Each `run-N.json` retains ordered per-worker samples, fixture and invocation
identities, trigger/subscriber timestamps, terminal outcomes, and cleanup
results. `summary.json` groups latency distributions by workload and event and
retains the median of each repeat plus its range. A failed or interrupted
command reports an incomplete result and returns nonzero; partial results are
preserved for diagnosis. No latency threshold is imposed.

The harness creates a short fixture directory under `/tmp`, directs resident,
adapter, provider-home, and XDG state there, and manages only fixture processes.
Adapters register their exact process identities because they have independent
process groups. Cleanup stops the fixture resident and registered adapters and
checks that they are gone. The ordinary `streaming_graphql` tests remain
separate and unchanged:

```sh
direnv exec . cargo test --locked -p cwp-runtime --test streaming_graphql
```

Retain the output directory, source revision, and command as the baseline for
later coalescing and Desktop comparisons. A successful run establishes that the
measurement is available; it does not establish a performance improvement.
