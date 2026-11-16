# Code

| Home | Responsibility |
| --- | --- |
| [apps/eval-runner](apps/eval-runner/README.md) | Fresh Linux VM commands, pinned guest image/build route and recovery via `rbdc evals`. |
| [apps/invoice-cwr](apps/invoice-cwr/README.md) | Editable scenario folders, full workspace export/diff, and separate CWR invoice grading/regrading actions. |
| [apps/publisher](apps/publisher/README.md) | Website Worker, Drive publisher, checks, and site-check CWR declarations. |
| [projects](projects/README.md) | Independently runnable article experiments and their reproduction instructions. |
| [infra](infra/README.md) | Repository-wide supporting tools. |
| [vendor](vendor/CWR-PROVENANCE.md) | Pinned external source and provenance. |

Reusable first-party libraries belong in `packages/` when there is an actual shared dependency; no empty package hierarchy is required. Keep project-local helpers with the project. The repository’s Rust toolchain pin is here so descendant Cargo projects inherit it.

Run publisher checks from `apps/publisher/`; other experiments own their commands. Root GitHub workflows set their working directories explicitly. The root flake and `.envrc` provide the shared development shell. The eval runner owns synthetic VM execution; model harness integration remains separate work.
