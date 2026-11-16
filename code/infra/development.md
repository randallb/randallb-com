# Root development environment

The root `flake.nix`, `flake.lock` and `.envrc` define the shared development shell. Nix and direnv/nix-direnv are host prerequisites. Run `direnv allow` once per checkout after reviewing `.envrc`; use `direnv exec . <command>` for noninteractive execution. `nix develop` works without direnv.

Rust is read from `code/rust-toolchain.toml`. Nixpkgs supplies the native compiler, Python, Node 24/npm and supporting tools; a separate pinned Nixpkgs input supplies Deno 2.9.7. Initial dependency revisions match the wlp-mono reference, without importing its runtime, secrets or company configuration.

`.envrc` watches the lock, flake, Rust pin and explicit eval-runner Nix/Python imports, and copies only those inputs into ignored `.direnv/shell-source/`. A relative `path:` flake reference supports checkout paths containing spaces. Shell loading uses `--no-update-lock-file`; dependency changes belong in the tracked root lock, not the staged copy.

## Verify changes

```sh
nix flake check --all-systems --no-build
direnv exec . cargo --version
direnv exec . deno task --cwd code/apps/publisher check
direnv exec . deno task --cwd code/apps/publisher setup:cwr
direnv exec . sh -c 'cd code/apps/publisher && python3 scripts/cwr-verify.py'
```

Update a dependency deliberately with `nix flake update <input-name>` and rerun the checks. Change Rust through its toolchain file. Review the resulting lock diff; do not update dependency revisions merely by entering the shell.

The shell declares Apple Silicon and Intel macOS, and ARM64 and x86-64 Linux. Evaluation on all four platforms does not establish runtime compatibility; local execution has been checked on Apple Silicon macOS. First builds may download compiler packages and language dependencies. Reproducible development tools do not provide filesystem, credential, process or network isolation for eval runs.

The root shell also exposes `rbdc`, whose `evals` command owns fresh synthetic
Linux VM runs. See [eval-runner setup and recovery](../apps/eval-runner/README.md).
App-owned Nix definitions compose the CLI, `evals` shell and ARM64 Linux image
through root outputs and the single dependency lock.

Intel macOS retains the root shell and task runner, but the pinned SmolVM and
Deno inputs no longer provide Intel macOS packages. Those tools are omitted
there; eval VM and publisher execution remain unverified/unsupported on that
platform. Apple Silicon and Linux pins are unchanged. CI forces every declared
host package/shell derivation rather than relying only on lazy flake checks.
