# Atlas publisher review

The publisher now accepts 100 MB of mapped Drive source and uploads file bytes
as Cloudflare static assets. Only the explicit route table is bundled in Worker
code. Requests to unmapped paths cannot reach uploaded assets. Content remains
in Drive; no Atlas essays, private research, or generated assets are committed.

Checks cover exact aggregate limit behavior, files larger than the former 4 MB
limit, stale asset removal, scoped indexing, binary bytes, source isolation,
GET/HEAD/404/405 responses, and content changes without code changes. The Atlas
bundle separately validates all 345 entries and passes its 30 existing tests.

The public export is mounted below /organizational-intelligence. It includes
full-text search and the eight existing visualizations. Existing site routes and
their indexing policy are preserved. Preview always discourages indexing; Atlas
production routes have canonical URLs and an Atlas sitemap.

No independent agent review is claimed. Release evidence and the exported content
receipt are maintained with the Drive Atlas publication project.
