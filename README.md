# randallb.com publisher

Google Drive is the editing home for pages, assets, the route list and migration
references. This repository contains the generic Worker, publisher and checks.

Content folder: [Launch / Website](https://drive.google.com/drive/folders/1s4jHLVpQcLjKrF03fxejifkucdD8Oah8).
Edit `pages/coming-soon.html` there; `site.json` maps public URLs to files.
Only explicitly mapped files are published. The reference archive stays private.
The initial Drive content preserves the approved placeholder on 17 known paths.
GET/HEAD work, unknown paths return 404 and other methods return 405.

## Publish from Drive

Use Python 3.9+, Deno 2 and Node/npm. With the Website folder synced locally:

```sh
python3 scripts/publish.py --source '/path/to/Drive/Projects/Launch/Website'
python3 scripts/publish.py --source '/path/to/Drive/Projects/Launch/Website' --target dry-run
python3 scripts/publish.py --source '/path/to/Drive/Projects/Launch/Website' --target preview
python3 scripts/publish.py --source '/path/to/Drive/Projects/Launch/Website' --target production
```

The default builds only. Each publish rereads Drive source, validates all mapped
files, runs code checks, then deploys an ignored generated snapshot. Local preview
after building: `deno task dev` (127.0.0.1:8000). `deno task bundle` checks the
last generated snapshot; use the publisher's dry-run target to read fresh content.

If Drive desktop sync is unavailable, use the connected Google Drive tools to
read the folder's `site.json`, download every referenced raw file into a temporary
folder outside this checkout, preserving relative paths, and use that directory
as `--source`. Record Drive IDs in `drive-provenance.json` alongside it. Download
raw HTML, not a text extraction. Never substitute repository content for Drive.

No content commit is needed. Drive edits do not publish automatically; run the
publisher after editing. The publisher supports up to 100 MB (100,000,000 bytes) of mapped source,
including the manifest. Each file is limited to 25 MB. Content is uploaded through
Cloudflare Workers static assets; Worker code contains only the route table.
Repeated routes to the same file count once. `noindex` may be set per route;
preview deployments always send noindex.
Missing files, paths escaping the source folder, and invalid routes stop release.
`.drive-build/` contains generated files and a receipt with content hashes and the
revision returned in `X-Content-Revision`. Keep it out of Git. Private reference
files, notes and receipts are not automatically exposed by the Worker.

Cloudflare production: Worker `randallb-com-placeholder-preview`, account
`b27198dafc75cca5de8a7c01be3cbd0f`, custom domain `randallb.com`.
The historical name includes preview but this Worker serves production.
`wrangler.preview.jsonc` deploys a separate Worker with no production routes.
Initial migration to Drive publishing is authorized by Randall's correction and
"ok please do that" on November 4. Later production releases follow the user's
release instructions; a Drive edit alone is not a release request.

## Checks and CWR

```sh
deno task check
deno task setup:cwr
deno task check:cwr
python3 scripts/cwr-verify.py
```

Checks use synthetic fixtures so CI needs no access to private Drive content.
Tests cover content changes without code changes, unmapped files, source escapes,
missing files, route validation, binary assets, HTTP status and HEAD behavior.
CWR setup needs Rust/Cargo 1.95.0 (pinned), Node/npm, Deno and a native C/C++ compiler
for SQLite. The pinned real runtime/shell adapter invokes `deno task check`;
`cwr-verify.py` verifies pass, intentional failure and restoration. See
`vendor/CWR-PROVENANCE.md`. No model account or Workloop source checkout required.

## Rollback

Previous placeholder Worker version: `481339ba-209b-453c-bbe7-edac8e0197bd`.
Use Cloudflare Worker rollback to restore that known version if this publication
fails smoke checks. No DNS changes are needed for rollback between Worker releases.

For the older Substack configuration, remove the Worker's custom domain, restore
DNS-only apex CNAME `@ -> target.substack-custom-domains.com` with TTL Auto, and
re-enable `randallb.com` in Substack publication Domain settings. Restoring DNS
alone is insufficient. The publication remains at `https://randallb.substack.com`.
Do not delete publication, subscribers, Worker, or mail records.

## Repository history

Draft PR #1 initially placed the catalog and reference pages in Git. They have
been copied and verified in Drive and removed from the branch's current tracked
files. Existing Git history is preserved. `migration/` may remain locally as an
ignored historical copy; it is neither the editing source nor a publish input.
The original Drive code checkout is also preserved. The current publisher update is maintained in the Atlas publication workspace's
`publisher` checkout; the former editing checkout is preserved.
