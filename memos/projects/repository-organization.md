# Repository organization

Owner: Randall. November 16, 2026. Implementation: Assistant. Status: file migration and local CWR verification complete; root direnv/Nix development shell verified on Apple Silicon macOS.

## Outcome

Make the public source companion easy to navigate and develop: root orientation, implementation under `code/`, public context under PARAM `memos/`, and the publisher as an app. Articles carry the explanation and findings.

## Direction and dependencies

Keep the old Drive checkout intact. Begin with current main, preserve analytics separately, and retain existing experiment source and external provenance. Shared code is centralized when actual reuse warrants it. Publisher behavior, source-export URLs, and independently runnable experiments must survive the move.

## Acceptance and next action

Verify publisher checks, CWR pass/failure/restoration, exporter committed-source behavior, CI paths, documentation links and ignored outputs. Document checks that could not be completed. Next: supply the pinned Rust toolchain and run CWR verification, then scope the Nix evaluation environment. No deployment or model run is part of this migration.

## Verification record

Publisher format/type checks, four Worker tests and six publisher tests pass from the relocated app. Whole-repository content exclusion, navigation links, shell syntax, Python compilation and generated-output ignore rules checked. All 1,668 project/vendor entries preserved byte for byte, including symlinks, except the intentionally updated project index and provenance location.

CWR setup was attempted through the relocated task and stopped at `cargo: command not found`. Pass/failure/restoration and Rust/Wasm experiment builds remain unverified in this session. CI paths are updated, but hosted CI has not run. Nix setup remains subsequent work. The existing Drive checkout remains untouched.

Actual committed-source export was exercised for all three experiments using temporary synthetic website content: GitHub links use `code/projects/`, public source URLs retain `/projects/`, repeated exports produce identical ZIP bytes, and a tracked source edit is rejected. The original source was restored and the working tree was clean afterward. No connected website content was modified.

Local commits separate analytics (`40f7794`) from layout migration (`e778964`). No remote push or deployment performed.

## Root direnv and flake — November 16

Root `.envrc`, `flake.nix` and `flake.lock` now provide the shared shell. Rust/Cargo 1.95.0 derives from the repository toolchain file; Deno is 2.9.7, Node is 24.19.0, Python is 3.13.15 and the native compiler is Clang 21.1.8. Nixpkgs, Deno and Rust-overlay lock revisions were taken from the inspected wlp-mono reference. The shell is standalone and does not load credentials or `.env` files.

Verified `direnv exec`, `nix develop --command`, inherited tool access in nested projects, and no-build flake evaluation on all four declared platforms. Actual builds tested only on Apple Silicon macOS. The direnv snapshot contains only the flake, lock and Rust toolchain file; `.direnv/` stays ignored.

Publisher checks pass inside the shell: four Worker and six Python tests. CWR builds successfully with `--locked`; real pass, intentional failure and restoration report runtime statuses 200, 500 and 200. This resolves the earlier missing-Cargo verification blocker. Hosted CI, other-platform execution, and isolated eval runs remain unverified. Next: scope the eval-run environment and school brief; no deployment, model run or remote publication occurred.

See [environment instructions](../../code/infra/development.md).
