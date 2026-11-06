# CWR VCS

An experimental version-control workflow for humans and AI coworkers. It explores signed proposals, review, CI, and integration using Hyperdrive.

Requires Node 24 or newer. From this archive's root:

```sh
cd experiments/cwr-source
npm ci
npm test
npm run scenario
```

## Visual walkthrough

```sh
npm run web
```

Open http://127.0.0.1:8765. Create a repository, clone workspaces for Alice and Bob, edit files, submit proposals, review them, run CI, and integrate approved proposals. Use disjoint filenames to accept both; edit the same file to explore a conflict.

This is a local demonstration. CI checks empty files and conflict markers, not a project's build or test suite. Actor keys share the local filesystem, so the demo does not establish process isolation or secure key storage. Peer replication, interrupted integration recovery, and untrusted-path handling remain unverified. Do not run the demo as a public source-control service.

The public name is CWR VCS; the original `experiments/cwr-source` directory is retained so source paths remain stable. `ORIGIN.json` identifies the upstream revision. Original source and documentation remain unchanged. This README, the npm lockfile, and attribution files were added for standalone distribution.
