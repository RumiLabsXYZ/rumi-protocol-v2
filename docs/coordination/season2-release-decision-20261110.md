# Season 2 release preparation — superseding the expired request

**EXECUTION HELD. Historical-credit acceptance is paused pending corrected reconstruction.** Orin owns execution; Dorian prepares and cross-checks evidence. This document does not authorize creation, installation, participant queries, imported credits or activation.

The fixed-19:00-UTC request at commit 28e0637e, SHA-256 d90a3e8e7504d8aee65405f4046b4f96f48f71943c00f887eb0de98e4fa5de44, is **expired and superseded**, not executable even if its old question is answered later. The old +650B payer question is closed: 4T was received. No additional funding is requested.

## Corrections before any credit decision

The proposed 20-point legacy component is **withdrawn**. It was based on an incorrect interpretation that fourteen older vault balances were zero-current. Independent Candid decoding of the saved 49-vault readback shows all fourteen still present with positive debt. The later liquidated vault is a separate loan; its zero debt does not make the older balances zero.

Twenty was the proposed total debt component for one participant, not twenty extra points. The prior model gave 19.01356466; rounding added 0.98643534. That was a discretionary proposal, not an earned amount verified from analytics or a protocol requirement. No such adjustment was approved or imported. A special compensation case is not justified by the corrected observation.

A mismatch between a canonical replay and live state is **not proof of missing history**. Analytics retains substantial history. Its aggregate series and approximate per-address chart are not themselves exact participant debt snapshots, but reconstruction from underlying events and other retained inputs remains unfinished. The [evidence note](season2-analytics-coverage-20261110.md) separates verified schema limitations, retained logs without public export, and unfinished checks.

The old 897,575.54043598-point sample was a proposed model, not awarded points or an exact history reconstruction. Do not use that transcript for an import. No upper-model or make-whole changes are accepted by this replacement.

## Continuous start and all eligible participants

The intended result remains one fresh Season 2 scorer and continuous historical accounting from **November 1, 2026 00:00 UTC through the actual prospective cutoff T0**. Finalized S1 remains preserved. Token allocation, claims, promotion and campaign end date are deferred.

The fifteen observed participants are the validated input cohort, **not an eligibility limit**. Re-scan qualifying actions through actual T0 and account for every eligible participant under inherited rules. Include supported new entrants with correct registration timing. If preparation cannot reconcile a new entrant or data case, finish that work before import/activation; do not omit a participant, turn unknown values into zero, truncate the interval or move the logical November 1 start.

Absent random A/B captures are a separate issue from the availability of balance history. Exact underlying balances, if reconstructed, still do not establish that those random snapshots were actually captured. The historical scoring convention must be stated accurately. That distinction is not permission to skip reconstruction or add compensation.

## Prepared approval-relative activation window

Once the historical decision is resolved, the final approval can use a reviewable relative window rather than a brittle short deadline:

1. One direct approval authorizes one release attempt within 24 hours after its recorded time.
2. After fresh preflight, derive T0 as the first UTC quarter-hour at least 60 minutes after that preflight, and no later than approval plus 24 hours.
3. The [local-only renderer](../../scripts/prepare_season2_release_window.py) records the approval reference/time, fresh preflight, T0, expiry and init hashes. It encodes an init but makes no RPC and cannot verify or grant human approval. Its binding explicitly grants no credit-import authority.
4. Freeze and publish the derived binding/init before installation. A marked example is never execution authority. No silent retiming or reset of an installed instance, automatic mutation retry, reinstall or substitute canister is included.
5. Begin import while T0 is future. Import the reconciled historical interval at/after T0. Before starting, verify the private first-capture time is still ahead; the current start API does not enforce this hold.
6. A missed cutoff/capture, expired window, uncertain mutation result, incomplete all-eligible coverage or unresolved data means HOLD. This expires initial execution authority, not an already running campaign.

The renderer was checked for quarter-hour rounding, current-preflight/approval ordering, a too-late window and Candid encoding. Its clearly labeled example uses synthetic approval/preflight times to produce T0 20:30 UTC; the example init is 139 bytes. That is preparation only, not a chosen live cutoff or approved award.

## Unchanged monetary and artifact scope

- Funding observed in the deploy account: **4,300,650,044,313 cycles**, including the received 4T. The separate earlier 1T canister top-up is not included.
- Proposed creation amount: **900,000,000,000 cycles**, on the pinned 13-node subnet. A 500B creation charge leaves **400B before installation**. The separate ledger fee is **0.1B**: total proposed source debit **900.1B**, expected remainder **3,400,550,044,313**, absent intervening changes. No new funding request, spending of the whole account, S1 funding or automatic top-up.
- One new selector/environment: rumi_points_season2 / mainnet-season2; only the newly returned ID may be installed. Sole controller/admin remains rumi_identity, principal fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae. Existing S1 and money-bearing canisters are outside scope.
- Optimized Wasm SHA-256: dc6b6fe48e79571d0bbe371b9fb8854bc77207abf3b5005c0eda7455388a9c17. Public H0: d050af464ea96252b37b21da99eaa7e7dd8291766627aee3eabd05610d0448b8. S0 stays private. No new build or seed is introduced by this timing preparation.
- Technical open end remains u64::MAX; it decides no campaign end date. Preserve source/asset bindings, protocol exclusions, normal freezing threshold, exact receipts/retries and cross-boundary accounting. Ordinary weekly awards are only claimed after actual close, not from timers alone.

Existing build/Candid, focused auth/state, calculator and one/fifteen-participant fixture results remain valid source evidence. The actual sample Candid encoded, but that does not prove its historical model correct. No matching hosted CI jobs are configured for this diff; no hosted-CI success is claimed.

The [operations preparation](../operations/season2-release-preparation-20261110.md) and [command plan](../operations/season2-creation-install-plan-20261110.md) contain target, funding and artifact details. [PR630](https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/630) remains source preparation. Participant-query and release denials persist. A funding receipt, coordination message, expired response or example render is not execution authority. Resolve the historical explanation before seeking acceptance of estimated credits; present one final necessary approval when its actual scope is ready.
