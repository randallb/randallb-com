# randallb.com

Public source companion to Randall’s articles: applications, experiments, evaluations, and the tooling needed to reproduce the work. Articles explain the decisions and findings; this repository makes the implementation inspectable.

- [Code](code/README.md): publisher, experiments, shared dependencies and infrastructure.
- [Memos](memos/README.md): public plans, operating guidance and project context using PARAM.

## Start developing

Use Deno 2, Python 3.9+, Node/npm, and Rust 1.95.0 where required. The Rust pin lives in `code/rust-toolchain.toml`. A Nix environment is planned, not yet implemented.

```sh
cd code/apps/publisher
deno task check
deno task setup:cwr
python3 scripts/cwr-verify.py
```

See the [publisher instructions](code/apps/publisher/README.md) for local preview and explicit publishing, and the [experiment index](code/projects/README.md) for independent setup and checks. Site pages and article drafts are edited in Drive; only explicitly mapped website files are published. Editing this repository does not deploy the website.

The repository root provides navigation and required repository configuration. Implementation and its tool configuration live under `code/`. Code-specific contracts and reproduction instructions stay alongside their implementation. Articles should link to exact source revisions.
