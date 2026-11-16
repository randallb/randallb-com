# Invoice evaluation through standalone CWR

Ask the `invoice_eval` coworker's `run--invoice` action to run one invoice coding
evaluation. It returns PASS or FAIL, checks passed out of five, and paths/URIs to
the retained solution, evaluation result and VM lifecycle record. Runner,
transport and cleanup problems return ERROR through a failed CWR action.

This independent local pack bundles the maintained eval-runner and invoice task.
It does not require a development checkout or another pack after installation.
Desktop installation is outside this milestone.

## Build and install

On this Apple Silicon Mac, provide Tart 2.41.1, Nix, the existing Codex file
sign-in, and a working local CWR CLI with its matching `cwp-runtime` and
`cwr-adapter-sh` binaries installed beside it. Python 3.11+ is supplied by the
repository Nix shell for setup; the installed launcher pins its absolute path.
Keep these machine prerequisites installed. The installer retains Nix GC roots
for its Python and verified Codex payload. Credentials are never packaged.

From the source repository:

```sh
python3 code/apps/invoice-cwr/package.py --output /tmp/invoice-cwr-0.1.0
nix build .#evals-codex-payload --out-link /tmp/invoice-codex-payload
```

Transfer/extract the resulting `.tar.gz` into a fresh directory, then run setup
from the extracted bundle using Nix Python. Neither setup nor invocation reads
the repository:

```sh
python3 /tmp/invoice-cwr-0.1.0/install.py \
  --destination /absolute/private/invoice-cwr \
  --payload /tmp/invoice-codex-payload \
  --cwr /absolute/installed/cwr/bin/cwr
/absolute/private/invoice-cwr/invoke
```

The installer creates a new directory and refuses replacement. The launcher
invokes the action using an isolated local CWR workspace and stops that runtime
afterward. Action input is `{}`; there are no arbitrary commands or task inputs.
The Shell adapter returns JSON in `stdout`; parse that JSON for the concise
grade/evidence report. A failed evaluation is a completed action with
`status: "failed"`; infrastructure errors fail the action with `status: "error"`.

## Run behavior and evidence

One action at a time per installation. The existing runner creates a fresh Tart
guest, copies host auth one way into disposable guest storage, runs pinned Codex,
exports the solution and runs all five invoice checks, then verifies guest
removal. Shared networking is opt-in through this existing authenticated mode;
no host folders are mounted and no credential refresh is written back.

Each invocation retains a unique `runs/<id>/evaluation/` directory under the installation.
`eval-result.json`, `invoice-total.sh`, and the linked runner `result.json` are
the primary evidence. Raw model output and operational logs stay in that private
directory; the action returns only a concise summary and artifact locations.
The containing run directory retains `action-report.json` and supervisor logs.
If CWR force-stops its Shell action, a parent-pipe watcher requests normal runner
cancellation and keeps the lock until cleanup and the error report complete.
Overlapping launcher requests fail busy without stopping the active runtime.
Evidence paths are local to this Mac. Failed checks also retain the solution;
missing or incomplete execution evidence never produces a passing grade.

`bundle.json` records source hashes; setup and actions verify them. These hashes
detect changed bundle contents, not publisher identity. Install a reviewed local
bundle. Rebuild from the maintained runner instead of editing the installed copy.
