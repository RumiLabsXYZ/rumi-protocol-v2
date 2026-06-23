# Chains-Rail Liquidation — Increment 4: Tier-2 ICP Stability Pool fallback

Status: IN PROGRESS (Task 1 complete; plan corrected after burn-model review)
Branch: `feat/chains-liquidation-inc4` (off merged `main` @ 67851b9)
Spec: `docs/superpowers/specs/2026-06-19-chains-liquidation-design.md` §5–§6 + Appendix
Surface map: workflow `wf_89c8bba2-ef9` (6 readers; file:line anchors below are post-Inc-3).

## Goal

When the Tier-1 bot cannot clear an underwater Conflux vault, the ICP Stability Pool
absorbs the debt by burning real IC-native icUSD (PSM does NOT apply here — this tier
DOES burn), takes the seized CFX at the 12% liquidation penalty into the vault's tECDSA
custody, and credits opted-in SP depositors a pro-rata CFX claim they later pull to an
EVM address. Ships **disabled-by-default / dev-gated / experimental**, on the same gating
as Inc 0–3. NOT armed; no real funds; not on prod `tfesu`.

## Rob's resolved decision (2026-06-21)

**CFX is OPT-IN.** Tier 2 is inert until an SP depositor explicitly accepts CFX collateral.
No existing depositor's risk profile changes silently. Reversible (can flip to opt-out later).

## The two independently-consistent halves (spec §6.1)

1. **Debt + supply (IC-side, synchronous at absorb).** SP burns IC icUSD → backend writes
   down `debt_e8s` and moves the amount into `pending_chain_burn_e8s` (NOT `chain_supplies`,
   NOT `reserve_backing`). There is **no automatic foreign-chain burn in Inc 4**: the eSpace
   ERC-20 `burn(uint256,uint64)` can only burn the caller's own balance, and the protocol cannot
   forcibly burn icUSD from the liquidated user's eSpace wallet. `pending_chain_burn` is the
   durable backing/accounting term for "IC-side supply has already been burned; foreign
   representation retirement is a later reconciliation step." A later Inc 5 manual settlement
   can consume `pending_chain_burn` when an operator has acquired/bridged the foreign icUSD and
   retired that representation.
2. **Collateral (EVM-side, async).** Seized CFX stays in the vault's per-vault custody.
   Record a `ChainLiqClaimV1`; depositors pull via `claim_cfx` → backend `claim_chain_collateral`
   → a custody-signed payout op.

## The unified invariant (already fully wired — DO NOT touch the RHS math)

`sum(chain_supplies) == total_chain_vault_debt_e8s() + total_reserve_backing_e8s() + total_pending_chain_burn_e8s()`

`chain_backing_rhs_e8s` (supply.rs:86) centralizes the RHS; `apply_supply_delta`,
`check_invariant`, the Timer-B self-check (xrc.rs:367), and `clear_invariant_halt`
(main.rs:2213) all route through it. Term-3 (`pending_chain_burn_e8s`) has ZERO writers
today. Inc 4 is the first writer. EVERY pending-burn mutation must go through a guarded
helper (never a raw map insert) or a single tick of imbalance false-halts the chain → ReadOnly.

## State / candid discipline (two canisters, two different rules)

- **backend `MultiChainStateV6`** = ciborium CBOR. Additive `#[serde(default)]` fields are
  proven-safe on live kvg63 (Inc 2/3 added fields to V6 with no V7 bump; V6→V6 decode preserved
  Vault #3). Add `chain_liquidation_claims` as a `#[serde(default)]` field on V6 (mirror how Inc 3
  added `chain_bad_debt_e8s`). NO V7. Gate: a V5→V6 + V6→V6 round-trip decode test (existing pattern).
- **`SettlementOpKind`** is internal CBOR (NOT on the candid surface — confirmed by Inc 2's inert
  LiquidationSwap). Inc 4 does **not** activate the existing dead `Burn { amount_e8s }` variant and
  does **not** add an automatic `EspaceBurn` variant. Foreign representation retirement belongs to
  Inc 5's manual reconciliation flow, because the current eSpace `IcUSD.sol::burn` can only burn the
  caller's own balance.
- **stability_pool state** = Candid `Encode!/Decode!` (raw blob + `try_decode_state` V-fallback).
  Per the hard rule, serde(default) does NOT save Candid Decode! on a missing NON-Option field.
  Add new `DepositPosition` fields as `Option<...>` `#[serde(default)]` (the PROVEN additive pattern
  in this canister — `total_interest_earned_e8s` / `pending_refunds` are Option and survived upgrades).
  Gate: an old-bytes → new-struct Candid round-trip decode test. If it fails, add `StabilityPoolStateV2`
  + `DepositPositionV2` snapshots + `From` + a `try_decode_state` branch. NEVER `-y` past a candid warning.
- Expect the recurring owner-gated `--yes` candid wall (new backend entries' input records). Rob runs installs.

## Implementation decisions (grounded in spec; not user-facing forks)

- **SP burns IC-native icUSD** by `icrc1_transfer` to the icUSD ledger minting account
  (`icrc1_minting_account`) with the `RUMI-LIQ-004:<vault_id BE>` memo, capturing the block index
  for the `SpWritedownProof`. Draw restricted to **icUSD only** in v1 (spec §6.6 resolved: icUSD-only).
- **Trigger = dev-gated manual** for v1 (matches experimental/disabled posture; spec §7 "manual = a
  human invoking the endpoint"). `should_escalate_to_sp` + `sp_attempted_chain_vaults` membership are
  the GATE; auto-timer escalation is a follow-up (Inc 5).
- **Claim payout op** = a NEW `ChainCollateralPayout { recipient, amount_e18, vault_id }` (NOT a
  reused `NativeWithdrawal` — its revert wrongly re-credits vault collateral). Its revert re-credits
  the SP depositor's `cfx_claims` via a backend→SP callback.
- **SP-absorb event** = reuse `ChainVaultLiquidated { tier: StabilityPool }` (the variant already
  carries `tier`); claim-settle = existing `ChainCfxClaimSettled`. Do NOT add `ChainVaultLiquidatedBySp`
  (avoids 3 exhaustive-match edits + a candid variant-add warning).
- **No bot-marker-vs-SP double-settle window** by construction: `escalate_failed_swap` clears the
  marker + restores collateral + sets sp_attempted BEFORE the SP can observe it. SP absorb's
  precondition is `pending_liquidation.is_none()` + `sp_attempted` membership, re-checked inside its guard.

## Tasks (TDD, per-task commit, all behind disabled-by-default config)

Each task: write the failing test first, implement, `cargo test` green, commit.

**Backend — invariant + settlement primitives**
1. `apply_debt_to_pending_burn_shift(state, chain, vault_id, burned_e8s)` (supply.rs) — clone of
   `apply_debt_to_reserve_shift`; debt→pending_chain_burn, supply untouched; same halt/cap/pre-RHS-check.
   `apply_pending_burn_to_supply_shift(state, chain, amt)` is retained as the future Inc 5 manual
   settlement primitive (pending_burn-=amt AND chain_supplies-=amt together). Pure property tests:
   invariant conserved; underflow fail-closed. **DONE in commit `6f0b16f`.**
2. `deposit_watch` backstop no-debt gate: change `total_chain_vault_debt_e8s()==0` →
   `... == 0 && total_pending_chain_burn_e8s()==0` (failing test first — a pending-burn balance means
   foreign representation is still outstanding, so the burn watcher/reconciliation should not go idle).

**Backend — claim record + SP-facing entries**
3. `ChainLiqClaimV1 { vault_id, chain, custody_address, seized_native_total, paid_native }` +
   `chain_liquidation_claims: BTreeMap<u64, ChainLiqClaimV1>` (#[serde(default)] on V6) + V5→V6/V6→V6
   round-trip decode test. Custody-claim invariant helper (`paid_native <= seized_native_total`).
4. `ChainCollateralPayout { recipient, amount_e18, vault_id }` op + custody signer (reuse arm) +
   confirm (increment `paid_native`, do NOT close vault) + revert (re-credit SP via callback, NOT vault).
5. `stability_pool_liquidate_chain_vault(vault_id, icusd_burned_e8s, proof, depositor_evm_claims)`
   #[update] — clone main.rs:3537 SP-caller gate; chains gates (invariant_halted/reorg_halted reject +
   fresh manual CFX price via `fresh_chain_price_e8`, NOT `validate_freshness_for_vault`); ChainVault
   per-vault guard; precondition `pending_liquidation.is_none()` + `sp_attempted` membership; TOCTOU
   live-debt re-read; `burned=min(req, live_debt)` (un-refundable — burn EXACTLY capped);
   `apply_debt_to_pending_burn_shift`; reserve CFX; record ChainLiqClaimV1; emit
   ChainVaultLiquidated{tier:StabilityPool}; ONE shot
   (on any failure insert sp_attempted, NO retry); verify burn proof via `fetch_and_validate_block`.
6. `claim_chain_collateral(claimer, owed_wei, dest_evm)` #[update] — SP-caller gated; validate dest_evm
   (`is_valid_evm_address`) before mutation; enqueue ChainCollateralPayout; increment paid_native;
   Duplicate idempotency.
7. Discovery: add `sp_attempted: bool` (+ chain + sentinel) to `ChainLiquidatableVault` so the SP/UI
   distinguishes never-tried from bot-failed-needs-SP (get_chain_liquidatable_vaults already surfaces
   bot-failed vaults: marker None + CR<threshold).

**stability_pool canister**
8. `DepositPosition.cfx_claims: Option<BTreeMap<Principal, u128>>` (#[serde(default)]) + update
    `new()` + fix `is_empty()` to count cfx_claims (silent-loss landmine at state.rs:818 retain) +
    old→new Candid round-trip decode test (escalate to V2 snapshot only if it fails).
9. CFX opt-IN: `opted_in_chain_collateral: Option<BTreeSet<Principal>>` on DepositPosition +
    `chain_collateral_sentinels: BTreeSet<Principal>` on state + `is_opted_in_for_chain(sentinel)`
    predicate + `opt_in_cfx`/`opt_out_cfx` state methods + #[update] endpoints (SpLiquidationGuard
    busy-guard, mirror lib.rs:147-175). Branch the 3 consumers (`effective_pool_for_collateral`,
    `compute_token_draw`, the new u128 gains sibling) to use the opt-in predicate for sentinels;
    keep default-in/opt-out for all non-sentinel collaterals.
10. `process_chain_liquidation_gains_at` (u128 sibling of state.rs:678) crediting `cfx_claims` +
    wrapper + `mark_cfx_claimed` (u128). Property test: $1–3k CFX seizure (~1e22 wei) credited with NO
    truncation; dust to first opted-in depositor.
11. CFX sentinel: deterministic-from-chain_id principal helper (stable across upgrades, never collides
    with a real ledger) + register via `register_collateral` (CollateralInfo{symbol:"CFX", decimals:18,
    status:Active}) + add to `chain_collateral_sentinels`. Exclude the sentinel from `validate_state`'s
    aggregate==ledger check (SP never physically holds CFX).
12. SP icUSD-burn capability: burn IC-native icUSD (transfer to `icrc1_minting_account` with RUMI-LIQ-004 memo),
    capture block index. icUSD-only draw filter (v1).
13. `sp_absorb_chain_vault(vault_id)` dev-gated #[update] orchestrator: coverage gate
    `effective_pool_for_collateral(cfx_sentinel) >= debt`; compute icUSD draw; burn; build proof; call
    backend `stability_pool_liquidate_chain_vault` under SpLiquidationGuard (ONE shot, no retry);
    credit `cfx_claims` u128 pro-rata on success; on failure → vault stays sp_attempted → Tier-3 manual.
14. `claim_cfx(sentinel, dest_evm)` #[update] — clone `claim_collateral`; SP-102 busy guard; validate
    EVM addr before mutation; deduct-before-async on cfx_claims (u128, zero first); call backend
    `claim_chain_collateral`; rollback on Err except Duplicate. Backend revert callback re-credits.

**Integration + ship**
15. PocketIC e2e (bot→SP escalation): bot swap fails → vault bot-failed (marker cleared, collateral
    restored, sp_attempted set) → depositor opts into CFX → dev `sp_absorb_chain_vault` → debt→
    pending_chain_burn, supply unchanged, invariant holds, vault reopens → ChainLiqClaimV1 recorded → cfx_claims (u128)
    credited pro-rata → claim_cfx → ChainCollateralPayout → confirm → paid_native bumped. Assert NO
    false-halt at any tick. Plus: no-retry (failed absorb → manual), over-burn cap, double-settle reject,
    zero-opt-in coverage → clean escalate-to-manual (finding #18), claim-revert re-credit.
16. Candid regen + breaking-change check (additive) + full verification (lib/bin/all PocketIC suites)
    + plan/memory update + PR vs main. STOP. Ask before merge/deploy.

## Folded adversarial findings (spec Appendix)
#1/#16/#19 over-burn clamp to live debt; #18 zero-opt-in → clean manual escalation (Rob's opt-in makes
this explicit); #26/#36 sp_attempted set-on-dispatch + CR-prune (prune already CR-derivable, better than
ICP); double-settle window (marker precondition); pending_chain_burn-stuck-forever is handled by making
it a durable reconciliation obligation for Inc 5 rather than a retry loop; discovery gap (sp_attempted
flag on the DTO).

## What ships
All write paths dev-gated; CFX sentinel inert until opt-in; no chain liquidation config enabled.
The Tier-2 fallback is code-complete and PocketIC-proven against the mock, dormant in production.
