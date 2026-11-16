# Authenticated guest proof: scope and anti-goals

Agreed with Randall on 2026-11-16; updated after approval to reuse the current host Codex session.

## Goal and completion condition

Prove that one fresh macOS guest completes one authenticated model request without a sign-in prompt, using a one-way copy of the current `$CODEX_HOME/auth.json` (falling back to `~/.codex`). Use Tart shared networking and one guest/run at a time. Stop after one successful response and review the next increment.

## Credential handling

`--reuse-host-auth` enables the pinned Codex installation and shared networking. It validates the existing auth file before VM creation and transfers it on guest-agent stdin into a private, disposable guest CODEX_HOME. Never log tokens or commit credentials. The host auth file is unchanged; guest auth is deleted with the VM. No host directories are mounted.

## Anti-goals

- Credential refresh writeback, synchronization, account reconciliation or reusable guest login caches.
- Concurrent execution or credential locking across runs.
- Automatic retries or seamless recovery of invalid sessions.
- Privileged network helpers, custom filtering or shared DHCP changes.
- A general authentication framework or production readiness guarantees.

Ordinary owned-VM cleanup remains required. Shared networking permits host/LAN/internet access; default runs remain offline. The deferred auth/network framework under experiments/deferred-auth stays disconnected.

## Verification

The authenticated live check uses `codex exec --skip-git-repo-check` with the prompt `Reply with exactly AUTH_PROOF_OK.` A successful installation or HTTPS request alone does not satisfy the completion condition. Retain the run result and verify VM cleanup.

### Completed live proof — 2026-11-16

Fresh guest returned `AUTH_PROOF_OK` from model `gpt-6.1-sol` without a sign-in prompt. Guest exit status was 0 and cleanup verified VM removal. Host auth was copied one way, with no writeback. Evidence: `.rbdc-evals/run-ed42592a4d9442188311b961cab7a309/result.json` and `guest-stdout`/`guest-stderr`. All 31 runner tests passed; `git diff --check` passed. The standalone payload warned that its code-mode host executable is absent; this text-only model proof did not exercise coding tools.

### Completed coding eval — 2026-11-16

The payload now pins and provisions the same-version code-mode-host archive with host and guest archive/binary checksum verification. A fresh guest completed the invoice-total task via normal provisioning, passed all five independent grading cases, exported the solution, and was removed. Evidence: `../invoice-eval-acceptance/eval-result.json` relative to the checkout root. The reusable entry point is `acceptance_eval.py`. All 33 runner tests and the all-systems Nix flake evaluation passed. This supersedes the earlier missing-code-mode-host limitation.
