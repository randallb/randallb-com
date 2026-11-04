# randallb.com

Standalone vanilla Deno application and Cloudflare Worker preview. The
production domain continues to serve the existing site. This repository contains
preview infrastructure, not the migrated archive or newsletter service.

## Setup

Install Deno 2.9.8 and Node.js 22 or later (including npm/npx). Wrangler is
pinned to 4.157.0 in the tasks. No application dependencies or Workloop checkout
are required.

```sh
git clone https://github.com/randallb/randallb-com.git
cd randallb-com
deno task dev
```

Open http://127.0.0.1:8000. The same request handler runs locally under Deno and
remotely under Cloudflare Workers; Deno itself is not deployed to Workers.

## Checks

```sh
deno task check
deno task bundle
```

The tests cover page/revision identity, preview indexing policy, health
responses, HEAD, missing paths, and unsupported methods. Bundle checks
Cloudflare compatibility without publishing.

## Preview deployment

The authorized target is Content Foundry, Inc.
(`b27198dafc75cca5de8a7c01be3cbd0f`), selected by Randall on November 4, 2026.
This supersedes the older planning reference to Workloop's account.

```sh
npx --yes wrangler@4.157.0 login
npx --yes wrangler@4.157.0 whoami
deno task deploy:preview
```

Use an existing authorized login; do not commit tokens. Deploy requires a clean
Git checkout, runs checks, and embeds the exact source revision in the page and
`/healthz`. Wrangler prints the deployed workers.dev URL. Inspect `/healthz`
there and compare `revision` with `git rev-parse HEAD`.

Configuration names the Worker `randallb-com-preview`, uses workers.dev, and has
no custom-domain routes. The public preview is discouraged from search indexing
but is not access-controlled; use public or fictional data only. Randall
approves production separately. Do not bind randallb.com as part of preview
work.

To remove only this preview, after confirming the selected account:

```sh
npx --yes wrangler@4.157.0 delete --name randallb-com-preview
```

## Source home

The editing checkout is `Memos/code/randallb-com` in the synced randallb.com
shared drive. GitHub is the shared source history. Drive synchronizes that one
checkout; do not edit a second independent Drive copy. Temporary fresh clones
are for verification only.

## Remaining infrastructure milestone

The broader launch brief also calls for a pinned vendored CWR check coworker,
success/failure evidence, collaborator access, and integrated review. This
preview bootstrap does not claim those outcomes. Archive migration,
email/subscription migration, enrollment, payments, and production release
remain separate work.
