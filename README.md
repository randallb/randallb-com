# randallb.com

Cloudflare Worker serving the visible text `randallb.com coming soon` on the
17 paths in `migration/url-catalog.json`. Unknown paths return 404 with the same
placeholder; GET and HEAD are supported, and other methods return 405. Queries
do not affect path matching. Responses are no-store and noindex.

Randall approved dropping Substack passthrough and deploying this narrower
placeholder milestone. It is live on `https://randallb.com`; this integration
preserves that behavior. The complete superbuild still requires CWR evidence and
ordered independent/hosted review. No merge approval is implied.

## Setup and checks

Use Deno, Node/npm, Rust/Cargo, and a native C/C++ compiler for SQLite.

```sh
deno task dev
deno task check
deno task bundle
deno task setup:cwr
deno task check:cwr
python3 scripts/cwr-verify.py
```

The local app listens on `127.0.0.1:8000`. The CWR task builds and invokes the
pinned real runtime and shell adapter; it runs `deno task check`, propagates a
failed runtime outcome as a nonzero caller exit, and stops its resident runtime.
No model account or Workloop source checkout is required. Toolchain verification
may reuse public dependency caches. See `vendor/CWR-PROVENANCE.md`.

## Deployment

`wrangler.preview.jsonc` has no production routes. To publish a review preview:

```sh
npx --yes wrangler@4.157.0 deploy --config wrangler.preview.jsonc
```

`wrangler.jsonc` describes the already-live Worker named
`randallb-com-placeholder-preview` in Content Foundry
(`b27198dafc75cca5de8a7c01be3cbd0f`), attached to `randallb.com` as a Worker
Custom Domain. The historical Worker name includes “preview” but it now serves
production. Production deployment is explicit:

```sh
npx --yes wrangler@4.157.0 deploy --config wrangler.jsonc
```

Randall's approval is required for a new production revision. This repository
integration alone does not redeploy. Observed live version:
`481339ba-209b-453c-bbe7-edac8e0197bd`. The deployed handler and this handler
are identical except for relocating the catalog import into `migration/`.

## URL replacement inventory

`migration/url-catalog.csv` and `.json` record 17 discovered/known paths,
including all five posts listed in the saved sitemap and RSS feed. The original ten Python page fetches failed with HTTP 403 and produced empty
files. Those files were subsequently recovered with curl from the retained
randallb.substack.com publication; see `migration/snapshot-recovery.json`. Ten public
pages are now preserved. `migration/public-pages/`, `feed.xml`, and `sitemap.xml`
preserve reference content. Service entry points were cataloged without submitting
forms. This is a public inventory, not proof of exhaustive private/unlinked or
historical coverage. Replace these paths as the real site is built. The deployed
placeholder does not expose the saved source pages or catalog as an API.

`migration/production-validation.json` records successful live GET checks for
all 17 paths after routing changed. It contains the production version identifier.
The older status fields in the catalog describe collection time, before cutover.

## Routing and rollback

The old apex CNAME was `@ -> target.substack-custom-domains.com`, DNS only,
TTL Auto. Its deletion and the placeholder cutover were explicitly approved.
Substack's custom-domain binding also had to be disabled before traffic reached
the Worker. The publication is retained at `https://randallb.substack.com`.
Mail DNS was not changed.

To restore Substack, remove the Worker's `randallb.com` Custom Domain; restore
the apex CNAME above; re-enable `randallb.com` in Substack publication Settings →
Domain. Check the exact configuration shown by Substack, allow routing/TLS to
settle, and verify the homepage plus a post. Restoring DNS alone is insufficient
because Substack's custom-domain binding is currently disabled. Do not delete
the Worker, publication, subscribers, or mail records.

## Source and review

GitHub repository: `randallb/randallb-com`. The former Drive checkout at
`Memos/code/randallb-com` is preserved but has incomplete Git metadata and stalled
file reads. The active integration checkout is the `integration-repo` folder in
the originating Workloop workspace. Avoid editing both copies. This branch is
based on remote main `85d04ca2c368e0cad8b8e64b079a9045a872ef7b` and carries the
pinned CWR source plus newly reconstructed task declarations; it does not pretend
the inaccessible earlier candidate history was recovered.
