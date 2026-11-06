# Standalone experiments

These projects are maintained in randallb/randallb-com. Website pages at
https://randallb.com/projects explain the experiments and offer source
snapshots.

| Project                                  | Purpose                                                    | Setup and checks                                |
| ---------------------------------------- | ---------------------------------------------------------- | ----------------------------------------------- |
| [Bel in WebAssembly](bel-wasm/readme.md) | Rust-hosted Bel bootstrap and Bel-written Wasm compilers   | Rust 1.95.0; Python 3; Node/npm for Wasm checks |
| [Ink on Deno](ink-on-deno/README.md)     | React/Ink terminal chat interface and executable packaging | Deno 2.9+; macOS/tmux for terminal tests        |
| [CWR VCS](cwr-vcs/README.md)             | Signed proposals, review, CI, and integration demo         | Node 24+ and npm                                |

Each directory is independently usable. Follow its README from that directory.
ORIGIN.json, LICENSE, and NOTICE.txt preserve the original wlp-mono revision and
attribution; upstream experiment files are unchanged by this import. This import
establishes maintained Git history here; it does not recreate all upstream
commits. The older personal repositories remain private reference material
outside this repo.

## Website exports

Git is the editing home for these experiment sources. Drive remains the editing
home for website prose, pages, and routes. Site-hosted source files and ZIPs are
exports of committed Git content, not separate editing copies.

After committing source changes, export into the website content folder:

```sh
python3 scripts/export_projects.py --site /path/to/Website
```

The exporter refreshes source files, manifests, GitHub links, and deterministic
ZIPs for all three experiments. Existing ZIP URLs remain available. It reads
committed Git objects, so untracked build files cannot enter the exports. The
original upstream attribution remains alongside the new maintained revision.
Review the generated site changes, then publish through the existing Drive
publisher. Exporting alone does not upload or deploy anything.
