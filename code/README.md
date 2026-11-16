# Code

| Home | Responsibility |
| --- | --- |
| [apps/publisher](apps/publisher/README.md) | Website Worker, Drive publisher, checks, and site-check CWR declarations. |
| [projects](projects/README.md) | Independently runnable article experiments and their reproduction instructions. |
| [infra](infra/README.md) | Repository-wide supporting tools. |
| [vendor](vendor/CWR-PROVENANCE.md) | Pinned external source and provenance. |

Reusable first-party libraries belong in `packages/` when there is an actual shared dependency; no empty package hierarchy is required. Keep project-local helpers with the project. The repository’s Rust toolchain pin is here so descendant Cargo projects inherit it.

Run publisher checks from `apps/publisher/`; other experiments own their commands. Root GitHub workflows set their working directories explicitly. The root flake and `.envrc` provide the shared development shell. Eval harness and isolated-run environment development are separate work.
