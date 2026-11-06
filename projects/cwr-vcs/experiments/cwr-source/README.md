# CWR source experiment

This local experiment gives one parent Hyperdrive an owner, gives each contributor a separate proposal Hyperdrive and Ed25519 keypair, and lets a code reviewer approve or request changes on an exact proposal checkpoint. A separate CI actor reads that checkpoint, checks changed files for empty contents or unresolved conflict markers, and signs a pass or fail. The integrator requires both signed reviewer approval and passing CI before accepting nonoverlapping changes. It checks the signatures and proposal bytes, returns a conflict if an accepted change touched the same path, then writes a new parent checkpoint and signs the integration record.

## Run

Requires Node 24. From this directory, run `npm install`, then `npm run scenario` or `npm test`. The scenario creates a temporary repository, submits two independent changes from the same parent, accepts both, checks a fresh clone, and returns an overlapping change as a conflict.

For a visual walkthrough, run `npm run web` and open `http://127.0.0.1:8765`. Create the repository, clone it for Alice and Bob, save edits, submit proposals, review them, run CI, and integrate proposals that have both approval and passing CI. Enter `<<<<<<< unresolved` as file contents to see CI fail. After acceptance, **Start next lap** clones the latest parent for another proposal by the same contributor. If the reviewer requests changes or CI fails, the contributor can revise the same workspace and submit a new signed proposal. If integration finds a conflict, the contributor can start from the latest parent, edit, and resubmit. The earlier proposals and feedback remain visible. Use different filenames to accept both initial proposals; use the same filename to see a conflict. The server uses temporary local data and removes it when stopped.

The commands can also be run individually:

```sh
node cli.mjs init /tmp/source-repo
node cli.mjs clone /tmp/source-repo alice /tmp/alice-workspace
# Edit files in /tmp/alice-workspace.
node cli.mjs submit /tmp/source-repo alice /tmp/alice-workspace
node cli.mjs review /tmp/source-repo /tmp/source-repo/proposals/<id>/manifest.json approve "Looks good"
node cli.mjs ci /tmp/source-repo /tmp/source-repo/proposals/<id>/manifest.json
node cli.mjs integrate /tmp/source-repo /tmp/source-repo/proposals/<id>/manifest.json
```

## Findings and limits

The local scenario passes for signed approval, feedback blocking integration, CI pass and fail, revision after failed CI, multiple accepted laps by one contributor, disjoint proposals, an overlapping conflict, revision after conflict, and fresh clones of accepted source. Each actor has its own keypair and proposal drive. This run uses one local filesystem for all stores and keeps all private keys there. The web interface separates reviewer, CI, and integrator actions, but it does not enforce process isolation or secret storage against someone with filesystem access. CI currently runs only the described source check; it does not run a project's build or tests. The experiment also does not test peer replication, recovery after a partial parent write, or safe handling of untrusted paths. The CLI is a disposable demonstration, not a source control service. The earlier source-network experiment lived in bfmono and is not part of this checkout.
