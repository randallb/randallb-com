# Integrated placeholder review record

Scope: the deployed placeholder behavior, 17-path URL inventory and public reference content, rollback, pinned CWR site-check invocation, and reproducible checks. Substack passthrough is explicitly superseded. The approved narrower production release does not establish full superbuild convergence.

Base: `85d04ca2c368e0cad8b8e64b079a9045a872ef7b`. Active branch: `integrate-live-placeholder`. Draft PR: https://github.com/randallb/randallb-com/pull/1 . No merge or new production deployment performed by this integration.

## Outer cycle 1

Dogfood round 1 resumed on `e4515e41f5cbe4ad3e6ba38819569ad870853c55`. Deno tests, local HTTP root/query/post/HEAD/404/POST, Worker bundle dry-run and 1,458 manifest hashes passed. Real CWR build/invocation remained pending. Independent preliminary assessment found ten empty page snapshots resulting from HTTP403, with overstated documentation. This round was not clean and did not advance stages.

Round 2 candidate `4df5dfadcd01b5caa3e316ff46b79359ecf521c7`: recovered ten public pages from unauthenticated randallb.substack.com with curl, retaining URL/size/hash evidence; corrected provenance claims. Added real CWR success/failing-test/restoration automation and hosted CI. Site checks pass. CWR and independent follow-up remain pending at this checkpoint; hosted status is not inferred from source checks.

The original deployed HTTP record covers all 17 production paths at version `481339ba-209b-453c-bbe7-edac8e0197bd`; browser inspection showed exact root text. The integrated Worker retains those semantics with only its catalog import relocated. Source integration has not been redeployed.

Runtime binaries, databases and credentials are ignored. Local build may reuse public toolchain/dependency caches. Hosted CI provides the separate clean-checkout setup check when it completes. No recording of this integration's CLI workflow has been inspected; local HTTP output is the current dogfood evidence.

A clean local assessment and actual PR-head CI/code/security review remain necessary. A cycle containing an implementation/evidence repair cannot establish final no-fix convergence; continue the existing ordered sequence and preserve actual rounds. No allowance is exhausted.
