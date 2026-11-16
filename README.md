# randallb.com

Public source companion to Randall’s articles: applications, experiments, evaluations, and the tooling needed to reproduce the work. Articles explain the decisions and findings; this repository makes the implementation inspectable.

- [Code](code/README.md): publisher, experiments, shared dependencies and infrastructure.
- [Memos](memos/README.md): public plans, operating guidance and project context using PARAM.

## Start developing

Install Nix with flakes enabled and direnv with nix-direnv integration, then from the repository root:

```sh
direnv allow
```

The root flake supplies Rust/Cargo 1.95.0 from `code/rust-toolchain.toml`, Deno, Node 24/npm, Python, a native compiler, and supporting CLI tools. `flake.lock` pins dependencies. The shell uses the same Nixpkgs, Deno and Rust-overlay revisions as the wlp-mono reference at setup time; it has no dependency on that checkout.

Without direnv, enter the same environment with `nix develop`, or run a command with `nix develop --command <command>`. With direnv, scripts and nested project directories inherit the root environment:

```sh
direnv exec . deno --version
```

The `.envrc` stages only flake inputs into ignored `.direnv/shell-source/`, leaving source, build output and local secrets out of the Nix store snapshot. It does not load `.env` files automatically. This is a development shell; evaluation-run isolation is separate work.

Then run the publisher checks:

```sh
cd code/apps/publisher
deno task check
deno task setup:cwr
python3 scripts/cwr-verify.py
```

See the [publisher instructions](code/apps/publisher/README.md) for local preview and explicit publishing, and the [experiment index](code/projects/README.md) for independent setup and checks. Site pages and article drafts are edited in Drive; only explicitly mapped website files are published. Editing this repository does not deploy the website.

The repository root provides navigation and required repository configuration. Implementation and its tool configuration live under `code/`. Code-specific contracts and reproduction instructions stay alongside their implementation. Articles should link to exact source revisions.
