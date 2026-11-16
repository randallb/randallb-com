# rbdc evals

For the standalone local CWR invoice action, build and install the
[invoice pack](../invoice-cwr/README.md). Its bundle reuses this runner and
`acceptance_eval.py`; the harness retains all five check outcomes and the
solution, distinguishing failed evaluations from execution/cleanup errors.

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
inputs. Other backend versions are rejected. SmolVM is included only on platforms
provided by its pin; Intel macOS has no upstream package and keeps the root
shell/task runner available without a backend. VM execution is unsupported
there. The root shell likewise omits the separately pinned Deno package on
Intel macOS because that input dropped the platform. Existing installed tools
and VMs are not replaced.

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


## macOS guests with Tart

The optional Tart backend runs actual ARM64 macOS guests on Apple Silicon. It
requires macOS 14 or newer, installed Tart **2.41.1**, at least 40 GiB free space
before each run, and capacity for the default four CPUs and 8192 MiB guest RAM.
macOS 15+ unattended hosts may need an unlocked login keychain; see
<https://tart.run/faq/#headless-machines>. No host keychain settings are changed
by this runner. The Linux backend remains the default.

Install Tart separately (the root Nix lock continues to own Linux dependencies):

```sh
brew install openai/tools/tart
tart --version  # must print 2.41.1; newer releases need explicit validation
code/bin/rbdc evals run --vm-type tart -- /usr/bin/sw_vers
code/bin/rbdc evals run --vm-type tart -- /bin/sh -c 'echo hello; echo diagnostic >&2'
```

`--backend` retains its existing meaning as the SmolVM executable path. Select
Tart with `--vm-type tart`; use `--tart /absolute/path/to/tart` or `RBDC_TART`
for its executable. The packaged Nix command also includes the Tart adapter;
it discovers the separately installed signed Tart app rather than building
or stripping its app bundle or provisioning profile.

The default base is pinned to this immutable OCI identity, never `latest`:

```
ghcr.io/cirruslabs/macos-tahoe-base@sha256:65c3133a633fda8753cf1f71f2d55727792d534a2125b0032466563dcc8e58fd
```

The first run may download approximately 27 GB compressed, with a one-hour
clone/download deadline. Later runs reuse the OCI cache and clone fresh disks
using APFS copy-on-write. This third-party base includes the Tart guest agent;
it is not a Nix-built macOS image, and Xcode/model harnesses are not provisioned
by this milestone. `--macos-image` accepts other digest-pinned OCI images that
provide a working Tart guest agent. Their compatibility must be verified.
Image acquisition needs host network access; workload guests do not receive
that access.

Tart state/cache defaults to `.rbdc-evals/tart/`, isolated from the person's
normal Tart home. Override with `--tart-home` or `RBDC_TART_HOME`, keeping this
a dedicated directory. Automatic Tart pruning is disabled. One workload is
allowed at a time per Tart home; leftover owned VMs block new runs until their
run record is recovered. The runner retains the OCI cache, but no local base
or stopped workload VM after successful cleanup. It does not inspect or mutate
the person's normal Tart VM inventory.

Each workload boots headlessly with clipboard, audio and USB sharing disabled,
no host filesystem shares, and a datagram network attachment connected only
to a host packet sink. The sink discards all frames and never sends frames
back or connects to a host/LAN/internet interface. Neither NAT nor Softnet is
used. Commands use the guest-agent channel rather than SSH. This bounds the
synthetic evaluation environment; it is not a security certification for
hostile kernels or guest agents.

After the guest agent becomes ready (default deadline 180 seconds), commands
run in `/private/tmp` with empty stdin and declared PATH, HOME and LANG only.
PATH includes macOS system tools and `/opt/homebrew/bin`. Argument boundaries
are retained. A shell runs only when explicitly requested. A guest completion
marker distinguishes a completed command's exit status from transport failure;
commands must not tamper with `/private/tmp/rbdc-<run-id>.exit`. Result JSON
retains the image digest, backend version/hash, source revision/dirty state,
resources, boot duration, Darwin identity, guest status and cleanup status.
Raw backend streams retain partial output on interruption; completed runs also
have `guest-stdout` and `guest-stderr`. Artifact transfer and interactive attach
are not yet implemented.

`--timeout` governs the guest command separately from `--boot-timeout`.
`--cpus` (default 4) and `--memory` (MiB; default 8192) configure macOS resources;
the caller must leave adequate capacity for the host. On failure/cancellation,
the exact owned VM is stopped, deleted, and checked for absence. A failed
cleanup remains an overall failure and preserves the ownership record. After
SIGKILL or host loss, use the same recovery command as Linux:

```sh
code/bin/rbdc evals recover .rbdc-evals/run-<id>
```

Recovery checks the Tart backend binary identity, UUID VM name, dedicated home
and saved VM configuration before mutation. Do not move state or upgrade Tart
before recovery. Apple macOS license terms and Tart's current FSL license apply
separately; see <https://www.apple.com/legal/sla/> and
<https://github.com/openai/tart/blob/main/LICENSE>.

Actual macOS acceptance (creates nine disposable VMs in sequence):

```sh
python3 code/apps/eval-runner/acceptance_tart.py --output /path/to/new/evidence
```

It exercises Darwin identity, stdout/stderr, host-environment/mount boundaries,
failed IPv4/IPv6 egress, reset between clones, tricky argv, guest exits 7/124/127,
timeout, Ctrl-C and forced-kill recovery. Evidence stays outside tracked source.


## First harness: Codex CLI in macOS guests

The installation-only increment provisions **Codex CLI 0.169.0** in a fresh
macOS guest, without login, host account state or a model call:

```sh
direnv allow
direnv exec . rbdc evals run --vm-type tart --with-codex -- codex --version
# Inspect CLI help without authenticating:
direnv exec . rbdc evals run --vm-type tart --with-codex -- codex --help
```

On Apple Silicon macOS, the Nix-packaged `rbdc` supplies `RBDC_CODEX_PAYLOAD`
to its own process. The root flake also exposes `evals-codex-payload`; the
app-local `codex-payload.nix` and `codex-pin.json` declare the official upstream
ARM64 macOS release URL, exact version and SHA-256. Nix verifies the archive
before constructing a payload directory containing the CLI archive, the same-version
code-mode-host archive, and their public manifest. The existing root lock supplies Nixpkgs; no second flake or
guest Nix bootstrap is introduced. Other host platforms do not expose the
macOS payload or claim guest harness support.

For the source entry point, first run
`nix build .#evals-codex-payload --no-link --print-out-paths`, then pass the
printed directory as `--codex-payload /nix/store/...` alongside `--with-codex`.
An explicitly supplied `RBDC_CODEX_PAYLOAD` may also select that directory;
the runner rejects manifests or archive checksums differing from its own pin.
A host-side `codex` executable is not used or implicitly copied.

After normal fresh-VM boot, provisioning checks that its guest installation
path is absent, verifies the host archive, transfers both pinned archives over
`tart exec -i` stdin, and verifies each received archive and installed executable
checksum inside the guest. The original release binary is not patched or
stripped. The CLI and `codex-code-mode-host` are installed together under
`/private/tmp/rbdc-tools/bin`, placed on
the command's declared PATH. A version check must match `codex-cli 0.169.0`
before the requested command runs. No host mounts, extra disks, guest network
access, SSH transport or credentials are needed. Host Nix acquisition may use
the network to fetch the public pinned release.

`codex-installation.json` and `result.json` retain release URL, archive and
binary hashes, expected/observed version, transfer method and guest path.
Backend operation records hash stdin without retaining a duplicate payload.
A missing/invalid payload fails preparation before a VM is created. A transfer,
installation or version-check failure reports `provisioning_failure` and still
runs owned-VM cleanup; cleanup failure remains the overall failure. Every run
starts from the unchanged pinned macOS base, so opting into Codex does not
modify the cache or subsequent guests. `--with-codex` is rejected for Linux.

Actual installation acceptance requires the Nix payload path explicitly:

```sh
python3 code/apps/eval-runner/acceptance_codex.py \
  --payload /nix/store/...-rbdc-codex-payload-0.169.0 \
  --output /path/to/new/evidence
```

This checks two fresh installs with matching provenance/version, offline CLI
help, absent host secret/auth/mounts, blocked egress, a subsequent unprovisioned
reset, and a real-VM injected transfer failure with verified cleanup. It does
not log in, select a model, run a coding task or provision Claude Code/Linux.


## Interactive macOS terminal

From Terminal on this Mac, launch the pinned Codex UI in a fresh guest:

```sh
direnv exec . rbdc evals run --vm-type tart --with-codex --interactive -- codex --no-daemon
```

The standalone pinned Codex release uses `--no-daemon` for its interactive UI;
it has no bundled background-server package. A normal terminal such as Terminal
or iTerm should supply its real `TERM` value (for example `xterm-256color`).
Codex rejects `TERM=dumb`. The current offline guest can display the welcome
and sign-in selector, but provider authentication and model/network access are
still separate work. No host account is automatically available in the guest.

For an interactive guest shell with Codex installed:

```sh
direnv exec . rbdc evals run --vm-type tart --with-codex --interactive -- /bin/sh -i
# Then, inside the guest:
codex --no-daemon
# Return to the guest shell, then type exit or Ctrl-D to remove the VM.
```

`-it` is an alias for `--interactive`. Interactive mode requires foreground controlling-terminal stdin
and stdout; redirected/piped input is rejected before VM creation. It currently
supports Tart macOS guests. Tart's guest PTY handles input, terminal dimensions
and resize events. Ctrl-C reaches the foreground guest process; an interactive
shell can remain usable afterward. Exiting the command or Ctrl-D at a shell
prompt ends the run and removes the owned VM. There is no persistent attach
or reconnect session.

The default interactive command deadline is one hour; `--timeout <seconds>`
accepts 1..86400 and excludes VM boot/provisioning. Noninteractive mode retains
its 300-second default. Timeout and host SIGINT/SIGTERM/SIGHUP stop the owned terminal
client, restore host input/echo settings and common TUI modes, then remove the
VM. If the terminal disappears, the operation records restoration failure
separately while preserving the original cancellation/timeout and VM cleanup. Guest exit status is checked against its completion marker just as for
noninteractive runs. Host SIGKILL cannot run restoration/cleanup; use `stty sane`
if the terminal is left in raw mode, then the existing `rbdc evals recover`
command for the retained owner record.

Interactive output is displayed live and is not saved as guest stdout/stderr;
keystrokes are not recorded. Run metadata, command argv, lifecycle/provisioning
diagnostics, guest exit status and cleanup results remain in the run directory,
with `interactive: true` and `output_capture: live_terminal`. Noninteractive
runs retain their separate captured output streams. Guest networking, host
mount and credential boundaries are unchanged.

Actual PTY/VM acceptance, with only synthetic test input and transcripts:

```sh
python3 code/apps/eval-runner/acceptance_interactive.py \
  --payload /nix/store/...-rbdc-codex-payload-0.169.0 \
  --output /path/to/new/evidence
```

This verifies input, 24x80 to 43x119 resizing, Ctrl-C followed by continued shell
use, exit 7, shell EOF, timeout, host cancellation, host terminal settings
restoration, VM removal and the Codex welcome screen exited without signing in.


## Authenticated model proof

The agreed [scope and anti-goals](AUTH-PROOF-SCOPE.md) govern this experiment.
Copy the current host Codex auth into a disposable guest and make one request:

```sh
direnv exec . rbdc evals run --vm-type tart --reuse-host-auth -- codex exec --skip-git-repo-check 'Reply with exactly AUTH_PROOF_OK.'
# Or use an interactive Codex terminal:
direnv exec . rbdc evals run --vm-type tart --reuse-host-auth --interactive -- codex --no-daemon
```

`--reuse-host-auth` enables Codex installation and Tart shared networking. It
reads `$CODEX_HOME/auth.json`, falling back to `~/.codex/auth.json`; use
`--codex-auth-home` to select an explicit directory. The source entry point
also requires the pinned `--codex-payload` as described above.

Credentials travel on guest-agent stdin into a private guest CODEX_HOME.
The host file is unchanged and guest auth is removed with the disposable VM.
There is no refresh writeback, reusable guest cache, or automatic sign-in.
Shared networking permits host/LAN/internet access with no custom helper;
default runs remain offline. `--network shared` can also be used independently.
Interactive terminal input/output is not captured; noninteractive command
output and lifecycle/cleanup results remain in the run directory.


## One end-to-end coding eval

Build the pinned payload, then run one invoice-total coding task:

```sh
nix build .#evals-codex-payload --no-link --print-out-paths
python3 code/apps/eval-runner/acceptance_eval.py --payload /nix/store/... --output /path/to/new/evidence
```

This uses the current host auth copy, shared networking, and a fresh guest.
Codex reads the task and example CSV, implements a POSIX shell CLI, and runs
its own checks. The runner then grades five exact-output cases: the example,
header-only input, blank lines, zero values, and a larger total. A failed case
exits nonzero. The runner exports the solution through captured guest stdout
before normal VM deletion; the host saves `invoice-total.sh`, its SHA-256,
`eval-result.json`, task, grading cases, transcripts and lifecycle records.
Success requires every case, a saved artifact, guest exit 0 and verified VM
removal. This small task proves the complete path; it is not a broad coding
benchmark. No guest downloads are needed to provision coding tools.
