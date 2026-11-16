# Scenario workspaces and grading through standalone CWR

Version 0.2 adds an editable scenario folder, full workspace export and separate
grading/regrading actions. The original combined invoice action remains available.

## Run a scenario, inspect it, then grade it

After installing the 0.2 bundle, edit `scenarios/invoice-docs/` in the installation:

```text
invoice-docs/
  task.md                   Initial request
  workspace/                Agent-visible files
    README.md
    docs/invoice-export.md
    meeting-notes/billing-handoff.md
    examples/example.csv
  grading/contract.json      Host-side grading selection; never copied to agent
```

The editable folder is separate from the bundle's immutable `scenario-template/`.
Use the installed launcher:

```sh
/absolute/private/scenario-cwr/invoke run invoice-docs
# Take run_id from the returned JSON inside result.stdout:
/absolute/private/scenario-cwr/invoke grade <run_id>
# Repeat grade with that same scenario run_id to regrade, without another model turn.
```

| CWR action | JSON input | Result |
| --- | --- | --- |
| `@invoice_eval--run--scenario` | `{"scenario":"invoice-docs"}` | Completed agent run, exported workspace/diff/logs; no grade yet |
| `@invoice_eval--grade--result` | `{"run_id":"<32 hex digits>"}` | Independent grade of that saved workspace, with a new evidence record |
| `@invoice_eval--run--invoice` | `{}` | Original combined invoice smoke evaluation |

Each scenario run snapshots and hashes the agent-visible folder before VM
creation. The fresh VM starts in that copy, with no host mounts. The finished
folder is exported before VM deletion, including code, documents, tests and
binary files. Retained `evaluation/export/` contains `workspace/`, `manifest.json`,
`workspace.tar`, `diff.patch` and `changes.json`; the run also retains stdout,
stderr, starting snapshot and VM lifecycle record. Large/binary changes have a
summary in the diff and full content in the saved workspace. Failed commands
still attempt export. Cancellation or transport loss can leave an incomplete
export, explicitly reported as an error.

Grading checks the saved hashes and copies the finished workspace to a separate
fresh VM. That VM has **no Codex installation, sign-in copy or network access**.
The first supported contract is `invoice-total-v1`: five deterministic checks
of `invoice-total.sh`. Adding another task grader is a source change, rather
than executing arbitrary host commands from scenario documents. Missing or empty
solutions fail the checks; changed saved artifacts and infrastructure problems
are errors. Every grade records its source run/manifest and grader identity.

Keep saved originals intact for regrading; make a separate copy to edit or launch
the resulting app yourself. Scenario files can be changed between new runs;
each run retains its own starting snapshot. Workspace transfer supports regular
files/directories up to 64 MiB and 4096 entries. Links and special files are
rejected rather than followed onto the host.

Setup accepts optional `--tart-home /absolute/shared/cache` to reuse a local
Tart image cache; the runner's workload lock serializes users of that cache.

Some base-image boots can stall the GUI login agent that provides Tart RPC.
`prepare_base.py --tart-home /absolute/shared/cache --output /absolute/base.json`
creates a separate, stopped local base with the same guest RPC executable started
by a system service as `admin`, independently of GUI login. Preparation uses the
image's documented public `admin` login over SSH, installs no Codex and copies no
host sign-in. A fresh clone must pass an unattended RPC check before the helper
writes the descriptor. Install with `--prepared-base /absolute/base.json` and
the same `--tart-home`. Every run checks the stopped base's disk/config/NVRAM
hashes and clones it; scenario transfer and grading continue over the guest
agent. The saved base is a machine prerequisite, not a retained workload VM.

## Original invoice action

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
python3 code/apps/invoice-cwr/package.py --output /tmp/scenario-cwr-0.2.0
nix build .#evals-codex-payload --out-link /tmp/invoice-codex-payload
```

Transfer/extract the resulting `.tar.gz` into a fresh directory, then run setup
from the extracted bundle using Nix Python. Neither setup nor invocation reads
the repository:

```sh
python3 /tmp/scenario-cwr-0.2.0/install.py \
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
