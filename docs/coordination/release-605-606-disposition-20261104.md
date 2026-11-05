# Release closeout: PR605 and PR606

Owner: Dorian, for Merritt. Read-only allocation, 2026-11-04. One native gpt-6-luna/medium worker; coordinator reviewed actual diffs and corrected the worker's mistaken claim that an implicit store read was equivalent to an explicit reactive dependency. No implementation, tests, builds, CI, product live calls, cleanup, PR closure or deployment.

Fresh `git fetch origin main` succeeded. Comparison pin: `4d9364b9335458ff97a248bf7287b1f3759ac493`. Later recorded frontend source: `5aca4853fd66cd40a615ed4eeb369ba540150e38`. PR605 head: `45af74a433c169c08e52bc1ad2bda23e1df215a9`; PR606 head: `0c520df74ef4d0609a0be93bea4bff0bd2974cbb`. GitHub metadata still reports both open/unmerged. No ancestry-based inclusion claim is used.

## PR605: included in main; missing change in recorded later frontend source

`git diff --quiet 45af74a4 origin/main -- src/vault_frontend/src/lib/components/swap/LiquidityInterface.svelte src/vault_frontend/src/lib/components/swap/LiquidityInterface.depositPolicy.spec.ts` exited 0: both production component and regression fixture are byte-identical to PR605. Main's helper takes balances explicitly at line108, and its reactive/render callers pass the store value.

At combined source5aca4853, the helper at108 reads the store internally; reactive add validation at143-148, rendered balance at502 and validation at520 call `getBalance(i)` without the explicit dependency. The diff against PR605 removes the mounted late-balance regression and its balance setter. This is the original defect mechanism, not equivalent implementation. This is a source finding; no current browser failure is claimed.

Next: existing frontend release owner should retain main's exact two-file fix in the next source composition, with the existing focused regression as acceptance. No redesign needed. Keep PR605 open until source/release reconciliation and disposition are accepted; do not close it merely because main includes its bytes.

## PR606: partial main coverage; missing pool integration

All13 changed files were inventoried against main and the later source. Main retains the backend notification exclusion (`lib.rs`), AdmissionPending and bot guards (`main.rs`), SP backend gating (`sp_liquidation.rs`), and XRP fresh-preflight guard/test (`vault.rs:4389`, test14174). Frontend EarnInfoCard, sunsetCollateralPolicy, liveApy, liquidity component and their changed fixtures match PR606; ProtocolStats differs for nICP claim accounting while retaining the sunset display filter and total-value distinction. No explorer edits are requested.

The precise missing main delta is all64 added lines in `src/stability_pool/src/liquidation.rs`: PR606 helper `verify_fresh_collateral_status` at6106-6124, pending-intent/fresh-admission checks at6130-6141,6178,6269, and regression near7874. Main's file is byte-identical to PR606's predecessor for this path and enters `execute_single_liquidation` at6106 without those additions. The helper and calls verify both local registry and authoritative backend status; recovery exceptions preserve existing intents. Backend guards elsewhere do not replace those pool-side admission checks before its spending path.

Next narrow integration card: original sunset/pool release owner integrates only those64 lines onto current main, preserving current journals and recovery exceptions; acceptance is fresh Sunset rejection plus existing-intent recovery and the existing focused regression. Do not cherry-pick or merge the entire stale branch blindly. Implementation and its checks require a subsequent allocation.

## Release evidence limits

The recorded November2 Oisy release was assets-only from5aca4853, after an earlier verified sunset publication. It does not establish a backend/pool installation from that entire source tree.5aca4853 lacks the pool64-line delta, XRP preflight guard/test, runtime-status sunset recognition and dashboard sunset presentation from PR606. Therefore it cannot establish preservation of those frontend changes; the backend/pool runtime disposition remains unresolved without separately allocated artifact/readback reconciliation. Historical deployment receipts remain valid historical evidence, not current-state proof. No funded liquidation or settlement proof is added; SUNSET-DEF-1 stays open.

Preserve original implementers and protected release worktrees. Communications remains parked, Sentinel background. Next deliverable is the two narrow integration/source-composition tasks for Merritt's allocation, followed by separate release evidence reconciliation. No live action authority follows from these findings.
