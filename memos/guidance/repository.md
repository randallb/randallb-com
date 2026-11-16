# Repository organization guidance

Owner: Randall. November 16, 2026. Scope: randallb/randallb-com.

Root README orients readers. Implementation lives under `code/`: deployable applications under `apps/`, independently reproducible experiments under `projects/`, shared first-party libraries under `packages/` when needed, infrastructure under `infra/`, and external pinned source under `vendor/`. Keep required GitHub configuration at root.

Public plans and operating guidance live under `memos/`; private planning, site content and article drafts retain their Drive editing homes. Keep one maintained statement and link to it. Code contracts, setup instructions and checks belong alongside the implementation. Create only directories the work needs.

Preserve authorship and provenance when moving source. Separate behavior changes from structural changes. Verify affected commands after moves and state incomplete checks plainly. Local edits and commits do not authorize remote publication or deployment.
