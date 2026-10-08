# Fiat-stable points implementation verification

Source base: 4b4b1680a89b37ab82641ae1e1cb6addf98dcdb5. Authorized scope: historical per-original-row unmatched uplift; preserved earned matched 5x; next-boundary flat 4x fiat 3pool; merge and deployment. Repayment and holding verification are unchanged.

## Deterministic evidence

- Rust library: 187 tests passed, including bounded inline corrections, missing-state rejection, and positive future flat-4x accrual.
- Frontend: 44 focused Vitest tests passed; production build and frontend auth/domain asset verifier passed. Regenerated points Candid/JS/TS agree.
- Python reporting: 15 tests passed; actual live original prefix: 401 rows / 15 principals totals reconcile. Expected uplift: 215174119482 e8s.
- Full frontend check: 28 errors and 65 warnings in 24 files. All 28 errors are in unchanged files: config, explorer services, old stabilityPool service, docs/parameters and docs/redemptions. No errors in touched points files. This broader baseline debt is deferred.
- Raw candidate endpoint check passes with an explicit allowlist for existing CDK timer_executor, lifecycle exports, get_candid_pointer and main. No fixture-only seeder endpoint is exported by production.
- New PointSource output variants intentionally fail old-Candid output subtyping. Regenerated frontend assets are published before the first correction rows; old cached clients may require reload.

## Independent review round 1

Two independent Luna reviewers inspected source and release semantics. Confirmed fixes: regenerated stale declarations; reject active-without-cutover and other inconsistent policy records; fail closed on a missing principal in the inline path; distinguish scheduled, historical-prefix-complete and legacy-inline-pending progress. Coordinator also removed an unbounded final correction scan before review, fixed an inconsistent accounting test fixture, and made global banner wording specific to fiat deposits.

One reviewer objected that original epoch points_accrued excluded policy corrections. That objection was withdrawn against the accepted contract: original summaries retain original accrual, while adjustments are distinct ledger rows and included in principal/season totals. The 51-principal test now asserts original 1071 versus corrected 1428 explicitly.

## Live release boundary

Preflight refreshed the concurrently updated security artifact 5860109f5dde4bacc1e02480c563bd3f38ef7f6a07674029d32a475d944585a5. Its points source at f72d1459 equals our base. Epoch 18 is already security-held; driveroff, poll configured on but fenced. This release preserves that state, schedules 19 pending, and independently applies existing-prefix adjustments. No future 4x accrual claim and no legacy recovery override.

## Upgrade evidence

Backend implementation commits: `e03c3d5c` and `b75da626`. Raw candidate SHA-256: `8ae8b1fbb2ac7382f72a790b2de775d1b1d4e1085cd65460d3a14d6415fc273d`. Extracted Candid matches the committed interface exactly; Candid parity test passes.

The populated policy-absent fixture is reproduced from an exact `4b4b1680` archive and the committed test-only patch. Its Wasm SHA-256 is `fdc527cd8d1ed6c62e29808338fa3c22c79073dceba51314219b0efdf2acf4d9` (reproduction steps in the fixture README). It has neither MemoryId 14 nor the new public policy interface. The candidate upgrade initializes the policy, rejects anonymous mutations, credits a positive original unmatched row, preserves a partial migration cursor through another upgrade, and makes completion retries no-ops. That PocketIC test passed.

The existing epoch-18 hold upgrade test also passed against the pinned legacy Wasm SHA-256 `d3fc3919ab50d75f245c2ad9539fb5c83a8da38a25c22d36b8f97e82ba0a3fbc`. The test confirms the held epoch remains held through the upgrade.

Fresh independent round-2 verdicts and deployed reconciliation will be recorded separately as they complete. Future 4x remains pending while the existing epoch-18 security hold remains in place.

## Independent review round 2

Reviewer A passed the completed implementation with no blocking findings. Reviewer B identified two concrete gaps: saturated legacy amounts needed an explicit reconciliation exception, and frontend completion flags needed to agree with the historical cursor. Both were accepted for correction. The reporter now rejects both the u128 maximum and the observable time-scaling saturation sentinel. The frontend rejects a completion flag that contradicts the fixed prefix cursor. Focused frontend tests remain 44 passing; reporting tests are now 15 passing. Backend saturation rejection and final upgrade checks precede a fresh third reviewer pair.
