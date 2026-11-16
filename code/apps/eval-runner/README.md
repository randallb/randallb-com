# rbdc evals

Run a command in a fresh ARM64 Linux VM from the repository development shell:

```sh
direnv allow
direnv exec . rbdc evals build-image
direnv exec . rbdc evals run -- sh -c 'echo hello'
```

Prerequisites: Apple Silicon macOS with Hypervisor support, Nix and nix-direnv,
anonymous access to GitHub, Docker Hub, releases.nixos.org and cache.nixos.org,
and at least 60 GiB free disk for the builder. The default build reserves a
48 GiB storage disk plus 2 GiB overlay, four CPUs and 8 GiB RAM. Workloads use
one CPU, 512 MiB RAM, 4 GiB storage and 2 GiB overlay; they require 8 GiB free
before creation. The host must have capacity for those budgets. No paid service,
provider account or company checkout is needed. Intel and Linux outputs are
Nix-evaluable; only Apple Silicon macOS running ARM64 guests has live acceptance.

`rbdc --help`, `rbdc evals --help` and `rbdc evals run --help` describe the CLI.
`code/bin/rbdc` is the source entry point; the root Nix shell exposes the packaged
command. `nix develop .#evals` provides a smaller app shell. The shared root lock
pins SmolVM tag v1.32.3 (CLI version 1.32.2), including its kernel/virtualization
inputs. Other backend versions are rejected. Existing installed tools and VMs
are not replaced.

## Linux image build from a Mac

`rbdc evals build-image` creates one separately owned Debian ARM64 builder,
bootstraps checksum-verified Nix 2.34.1, and invokes the root output:

```sh
nix build --store local --no-update-lock-file --no-write-lock-file \
  path:/opt/rbdc-source#packages.aarch64-linux.evals-image
```

A native ARM64 Linux Nix host can build the same output directly with
`nix build .#packages.aarch64-linux.evals-image`. The app-local `image.nix`
uses `dockerTools.buildLayeredImage`; it includes Bash, coreutils, Python and
`guest.py`. Kernel and VM boot support come from pinned SmolVM. This is not a
NixOS boot image. Host CLI dependencies and Linux guest packages are separate.
The image build keeps Nix sandboxing enabled and records actual guest disk
capacity, the root lock hash, declared source hashes, retrieved size and both
guest/host image checksums.

The source archive is an explicit allowlist in `builder.py`: root flake/lock,
Rust pin and the app's Nix/Python/bootstrap inputs. It excludes credentials,
checkout metadata, unrelated source and generated state. There is one root lock;
no app flake is introduced. `.envrc` stages only the exact shell evaluation
inputs and watches each imported file.

The builder is stopped after success, failure or cancellation and its exact
ownership remains under `.rbdc-evals/builder/owner.json`. Repeating the build
reuses only that stopped, configuration-matching builder. It replaces its
source archive and image output with the current declared inputs. A changed
backend requires a new builder state directory. Workload VMs never reuse this
builder's writable state or network settings.

The default image is `.rbdc-evals/builder/image.tar.gz`. Set `RBDC_EVALS_IMAGE`
or pass `--image /absolute/path/image.tar.gz` to use another built artifact.
`build-result.json` and `source-manifest.json` retain its identity. The first
build downloads several hundred megabytes. Image transfer is limited to files
smaller than 4 GiB and fails explicitly above that limit.

## Execution and records

Arguments after `--` retain their boundaries. Shell syntax is evaluated only
when you explicitly run a guest shell, for example `-- sh -c 'echo hello'`.
The guest starts in `/tmp`, with empty stdin and only declared `PATH=/bin`,
`HOME=/tmp` and `LANG=C.UTF-8`. Each run gets a new UUID VM and fresh writable
state from the same image. There are no host mounts, forwarded credentials,
host environment, ports or workload network access. The CLI checks the actual
backend inventory's image hash and those boundaries before execution. These
settings describe the synthetic workload boundary; Nix reproducibility itself
does not provide VM isolation. Interactive input/attach and malicious-workload
containment testing are outside this milestone.

The CLI prints the run-record directory and streams terminal output. Each
`.rbdc-evals/run-*/` keeps `owner.json`, `result.json`, backend operation
transcripts, exact guest stdout/stderr and a structured guest result. Records
include argv, source revision/dirty state, image SHA-256, backend version/hash,
resources, empty declared fixture set, Linux identity, guest status, execution
outcome and cleanup result. Backend transport streams retain partial output if
the guest cannot finish its result record. Completed guest files separate guest
output from backend diagnostics. The workload must not modify the runner's
`/tmp/rbdc-<token>` result files.

Success exits 0, a guest command failure preserves its exit status (for example
7), and a missing guest executable exits 127. Cancellation exits 128+signal.
Preparation, transport, timeout and cleanup failures exit nonzero; cleanup
failure remains an overall failure even after a successful guest command.
The JSON preserves execution and cleanup outcomes separately. Default command
deadline is 300 seconds; `--timeout` accepts 1..86400. Backend control operations
also have deadlines. Ctrl-C/SIGTERM kills the owned exec process group and then
stops/deletes the exact workload VM. Further interruption signals are recorded
but do not interrupt cleanup. Cleanup takes at most about 90 seconds under
backend faults; its diagnostics and ownership remain if absence is unverified.

## Recovery

SIGKILL or host loss cannot run cleanup. Recover an inactive run by the directory
printed at startup; an active-run lock prevents concurrent recovery:

```sh
rbdc evals recover .rbdc-evals/run-<id>
rbdc evals recover .rbdc-evals/builder
# Delete reusable builder state only when no longer needed:
rbdc evals recover .rbdc-evals/builder --delete-builder
```

Recovery validates the ownership schema, UUID, exact machine name, backend hash
and actual VM configuration. It attempts the exact owned stop even if the first
inventory read fails, and requires verified absence before reporting removal.
A configuration mismatch refuses mutation. It never scans for machines to adopt
or cleans unrelated VMs. Keep the ownership directory until recovery succeeds.
The default backend state/cache is managed by SmolVM; deleting a VM does not
purge shared cached immutable images. Generated records/state are ignored by
Git; publish only deliberately selected evidence after inspection.

## Verification

```sh
direnv exec . python3 -m unittest discover -s code/apps/eval-runner -p 'test_*.py' -v
nix flake check --all-systems --no-build
# Real VMs, requires built image; output directory must be new:
direnv exec . python3 code/apps/eval-runner/acceptance.py --output /path/to/new/evidence
```

Focused tests inject lifecycle faults; the live acceptance script uses the
public CLI and actual VMs for stdout/stderr, reset, argv, exit 7, missing backend,
invalid image/command, guest timeout versus exit124, Ctrl-C, forced-kill recovery and workload boundaries.
Mocked lifecycle tests are not evidence of guest execution. The TeamWorkspace
superbuild brief is maintained outside source at `TeamWorkspace/Projects/rbdc-evals/SUPERBUILD.md`; the PR carries
selected current-revision evidence and known limits. Source notices are in
`NOTICE` and `LICENSE.wlp-mono`.
