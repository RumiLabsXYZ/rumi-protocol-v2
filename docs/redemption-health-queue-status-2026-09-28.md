# Redemption health queue: verification status

**Updated:** 2026-09-28
**Branch:** `codex/redemption-health-queue`
**Baseline:** `origin/main` at `ac59a6519730744dd30c33958b959577dec12123`

This records the implementation and checks completed so far. It is not a deployment or payout claim.

## Intended behavior

The backend ranks eligible vaults globally by the same normalized health score used to drive the existing collateral-ratio color ramp, from weakest toward healthiest. It groups only consecutive vaults of the same collateral type. A redemption consumes one contiguous run and pays one collateral type; the same type can appear again later if another type ranks between its vaults. The first run's capacity is the eligible vaults' debt-backed capacity, not the total collateral balance of every vault holding that token. The queue is recomputed after each redemption, so a displayed run is a current quote, not a promise to exhaust every unit before another asset ranks first.

The ordering score is:

```text
headroom = (currentCR - liquidationCR)
           / (1.351 × borrowThresholdCR - liquidationCR)
```

Lower scores go first; tier is only a tie-breaker. For the example parameters, ICP at 150% CR scores about 0.244, while ckXAUT at 145% CR scores about 0.696. ICP is therefore first even though ckXAUT's face-value CR is lower. This makes the queue reflect proportional distance from each collateral's own danger threshold, rather than comparing raw CR percentages across assets.

## Backend evidence

In `/Users/robertripley/.codex/worktrees/redemption-health-queue/rumi-protocol-v2`, `git diff --check` passed. The focused redemption run exited 0: 37 passed, 0 failed. A fixture-only correction then made `state::tests::red001_consumed_capped_at_total_vault_debt` pass 1/1. The full bounded backend library suite exited 0 on the final frozen backend snapshot: **930 passed, 0 failed, 1 ignored**. The ignored test is the pre-existing `state::tests::test_borrow_fee_does_not_credit_liquidity_pool`; its comment explains that it calls `ic_cdk::api::caller()` and requires canister context.

The exercised cases include historical `None` replay compatibility, new `Some(empty)` events, per-vault actual payout and pending-claim parity across native decimal scales, underwater collateral clamping, checked overflow/minimum rejection before mutation, health ordering and consecutive grouping, reserve spillover conservation, stale-price rejection, legacy ICP fail-closed behavior, and debt-backed capacity boundaries. The full suite emitted existing unused-import/dead-code warnings and one new unused-variable warning; it completed successfully.

The final `RUMI_REGEN_DID=1` backend binary test exited 0 and regenerated `src/rumi_protocol_backend/rumi_protocol_backend.did`. It checks structural compatibility against the frozen live legacy service and asserts the old methods retain the 12-arm legacy `ProtocolError`, while quote and quoted-submit use the new `RedemptionError`. The no-regeneration compatibility rerun passed 1/1, and `scripts/regenerate-declarations.sh rumi_protocol_backend` exited 0 (`ok: rumi_protocol_backend`), refreshing generated JS/TS. The final partial-stable and zero-stable reserve accounting regressions are both present in the green library suite.

## Frontend verification in progress

With final canonical `RedemptionError` declarations generated, redemption DTOs use the generated Candid types and redemption-specific API casts are removed. The latest combined focused suite exited 0: **58 tests across five files**, including four real generated-IDL cases, 35 API boundary tests, and 19 helper/route cases. The final production build exited 0 and wrote `dist`.

The final generated-type `npm run check` exited 1 with 28 errors across six untouched files: `lib/config.ts`, explorer analytics/collateral config, `stabilityPool` service, docs parameters, and docs redemptions. It reported zero diagnostics in the changed redemption page, helpers, bindings, API, or protocol paths. These failures are outside touched files, not proven baseline because no same-dependency baseline check was retained. `git diff --check` exited 0.

## Remaining gates

The old generated-IDL check confirmed legacy success and `GenericError` replies remain decodable; new structured quote error tags are isolated from the old 12-arm service. Backend keeps `ProtocolError` unchanged on `redeem_icp`, `redeem_collateral`, and `redeem_reserves`, and uses `RedemptionError` only on the new quote and quoted-submit methods. Direct/quoted refunds reconstruct the unrounded effective tail from post-fee raw budget, RMR, and actual consumed collateral before flooring. Reserve refunds aggregate effective stable e6 payout plus committed vault fee and actual collateral, convert once by RMR, floor, and cap to the post-reserve-fee raw budget; pre-commit errors use zero vault fee/collateral. Regression cases cover raw input 100,000,111 at 90% RMR with no spillover e6 payment, returning the 111-unit stable-rounding remainder; a partial-stable/native reserve case with 44.55m raw refund; and direct/quoted 100,000,001 at 90% with 90,000,000 consumed, returning one raw unit. Backend tests, Candid compatibility, and declarations are green. Frontend focused tests/build are green; the global frontend check has 28 untouched-file diagnostics. Independent source review, local source commit, backend artifact, and release preflight remain to be completed. The repository has no `.github/workflows` directory or tracked CI workflow; `src/vault_frontend/package.json` exposes `check`, `build`, and Vitest scripts. Deployment remains pending the orchestrator's final readiness decision. No user funds have been moved.

Earlier approval and review-only artifacts are preserved in the evidence commits already on this branch; this status update does not alter those immutable proposal files.
