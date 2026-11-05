# Drive publishing review state

Randall corrected the content location: Google Drive holds pages and assets;
Git holds publishing code. The current change implements that correction.

Checks exercise generic routing and binary assets, content edits changing the
build without code edits, exclusion of unlisted files, and rejection of missing
or escaped source paths. CI and the actual CWR shell adapter run those checks
without access to private Drive. Generated deployment files are ignored.

The recovered archive and catalog now live in the Drive Website/reference folder;
only files explicitly named by site.json are included in the public bundle.
Existing Git history is retained; the branch's current tracked content is removed.

Previous CWR pass/fail/restoration CI passed at dc61529. New CI and live release
verification must be recorded for the Drive publisher revision separately.
No independent agent review is claimed for this correction.
