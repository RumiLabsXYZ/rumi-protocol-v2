# Conflux gated eSpace-mainnet soft-launch — operator runbook

Status: DRAFT for Rob. Local doc (docs/ gitignored). Anchor: `main`@`39a07cd` (PR #251 rail + #252 interest).
Posture: the 2026-06-18 chains-rail audit said **GO for a gated soft-launch with conditions** (`audit-reports/2026-06-18-39a07cd/`).
Goal: real CFX on **eSpace mainnet (chain 1030)**, tightly capped + dev-gated, manual risk management.

---

## ⚠️ Read first — three hard truths that shape every step

1. **Liquidation on the chains rail EXISTS now** (Increments 0-13; the engine, the bot path and the SP escalation all landed after this runbook was first written). It is nonetheless a SEPARATE switch from everything else here: the `chain_liquidation_configs` row's `enabled` flag gates only the liquidation-swap worker. Staging the row (with `enabled = false`) is what starts the XRC price feed and claims the pair; flipping `enabled` is what turns liquidation on. Do not conflate them. Also note that the M2 self-serve `_evm` endpoints are signature-authenticated, NOT developer-gated: **binding the IcUSD contract is what makes the chain publicly openable** (see step 4), so "keep it gated" means "do not bind the contract yet", not "do not call `open_chain_vault`".

2. **The debt ceiling IS code-enforced.** `debt_ceiling_e8s` is no longer inert config: it is threaded into both risk-increasing paths. `open_chain_vault_in_state` and `borrow_chain_vault_in_state` (`src/rumi_protocol_backend/src/chains/vault.rs`) each add the requested amount to `total_chain_debt_including_pending_e8s(state, chain)` and reject with `DebtCeilingExceeded` if the result would breach the cap. That total counts pending/in-flight mints as well as confirmed debt, and the check runs inside one synchronous `mutate_state` with no `.await`, so concurrent opens cannot collectively slip past it. Chain 1030 is configured at **500 icUSD** (`chains/collateral_config.rs`, carried into the runtime `ChainDebtConfigV1` row and adjustable later via `set_chain_debt_config`). That number is deliberately small: it is depth-bound to what the eSpace DEX can absorb per liquidation swap. So the cap is a genuine guardrail rather than operator discipline, but keep the discipline too, and treat raising it as an explicit decision rather than a routine config tweak.

3. **The ECDSA key is now a runtime setter (PR #254 — was hardcoded `test_key_1`).** `set_chains_ecdsa_key_name("key_1")` flips the EVM rail to the production threshold key — **no code change/rebuild**. BUT switching re-derives every custody/settlement/interest-treasury address, so it is **rejected once any chain vault exists**: set `key_1` on a FRESH canister BEFORE registering any chain, then deploy IcUSD.sol pointed at the new `key_1`-derived minter. kvg63 staging keeps `test_key_1`. See Decision 0.

---

## Decision 0 (REQUIRED — your call): which ECDSA key
PR #254 made this a **runtime setter** — no code change either way.
| | `test_key_1` (default) | `key_1` (production) |
|---|---|---|
| How | nothing (default) | `set_chains_ecdsa_key_name("key_1")` on a fresh canister, before any chain is registered |
| Addresses | testnet-proven | all custody/settlement/treasury addresses derive from `key_1` → deploy IcUSD.sol pointed at the new minter |
| Risk for real CFX | a real threshold key but designated "test" (weaker operational guarantees) | production-grade |

**Recommendation: use `key_1` for anything holding real CFX you'd be sad to lose** (the setter makes it cheap now). `test_key_1` only for a throwaway dust-only smoke test. Either way the key MUST be set before the first vault (the setter refuses once vaults exist — orphan guard).

## Decision 1 (your call): which canister
- **kvg63 staging** (already runs the rail + interest, `test_key_1`): fastest. Adding chain 1030 here puts real eSpace-mainnet CFX on the staging canister. Fine for a tiny dev-only soft-launch; keep it small.
- **Production `tfesu`** (`key_1`): mixes the chains rail's real-CFX custody into the live ICP-CDP canister. Bigger blast radius; only after Decision 0 = `key_1` + a fresh audit pass of the combined surface. **Not recommended for the soft-launch.**
- **New production-keyed canister** for the chains rail: cleanest separation for scale; more setup.

**Recommendation:** kvg63 for the initial gated launch; a dedicated production-keyed canister when you go to `key_1`/scale.

---

## Pre-launch checklist (audit's go-with-conditions)
- [ ] Decision 0 (key) + Decision 1 (canister) made.
- [ ] **≥2 independent mainnet RPC providers** secured (NOWNodes / BlockPi / Validation Cloud) — NOT all Confura (audit F-04). Register with `min_quorum_providers ≥ 2`.
- [ ] Pricing model understood and the recovery loop rehearsed (see §"Pricing model" below): automatic XRC is authoritative once the liquidation config row is staged; `disable_chain` is the reversible emergency stop; manual rebaseline is available only while Disabled; `enable_chain` resumes.
- [ ] Tiny initial cap agreed (e.g. ≤ a few hundred USD of debt total) and a written discipline that you (the only opener) won't exceed it.
- [ ] Monitoring dashboard live: `reconcile_chain_supply(1030)` gap = 0, `invariant_halted`/`reorg_halted` false, hot-wallet balance, `processed_burn_keys` size (audit F-02).
- [ ] Deployer key funded with real CFX for the IcUSD.sol deploy gas (~1.65M gas; eSpace meters high).
- [ ] (Optional, cheap) the F-05 defense-in-depth one-liner coordinated with the M2 session.

---

## Operator sequence (templates — fill the placeholders; use `--identity rumi_identity`)

**0. (If Decision 0 = `key_1`) set the production key FIRST**, on the fresh canister, before any chain is registered (it refuses once a vault exists — and every address below derives from it):
```
icp canister call <CANISTER> set_chains_ecdsa_key_name '("key_1")' -e <ENV> --identity rumi_identity
icp canister call <CANISTER> get_chains_ecdsa_key_name '()' -e <ENV>   # verify -> "key_1"
```

**1. POST-UPGRADE read-back of the key, the RPC principal and every derived address, BEFORE the immutable token deployment.** IcUSD.sol bakes the minter address in permanently, so this read-back is not a formality: a pre-upgrade address report can be STALE (the per-chain address caches in `chains/evm/tecdsa.rs` were keyed only by `ChainId` and were not invalidated on a key flip). Run all of these on the upgraded canister, in this order, and use ONLY these values downstream:
```
icp canister call <CANISTER> get_chains_ecdsa_key_name '()' -e <ENV>                       # -> "key_1"
icp canister call <CANISTER> get_evm_rpc_principal '()' -e <ENV>                           # -> the official EVM-RPC canister, no mock override
icp canister call <CANISTER> get_chain_settlement_address '(1030 : nat32)' -e <ENV> --identity rumi_identity
icp canister call <CANISTER> get_chain_reserve_address '(1030 : nat32)' -e <ENV> --identity rumi_identity
icp canister call <CANISTER> get_chain_interest_treasury_address '(1030 : nat32)' -e <ENV> --identity rumi_identity
```
The settlement address is what IcUSD.sol's MINTER_ROLE must be (per-chain, key-dependent; derived from `settlement_derivation_path(1030)`). The reserve address is a SINK and needs no funding. If any value disagrees with a pre-upgrade note, the post-upgrade read is the truth.

**2. Deploy IcUSD.sol to eSpace MAINNET** (Foundry; mirrors the testnet deploy, see `foundry/DEPLOY.md`):
```
cd foundry
export CANISTER_SETTLEMENT_ADDR=0x<minter from step 1>
export DEPLOYER_PK=<funded eSpace-mainnet deployer key>
forge script script/DeployIcUSD.s.sol \
  --rpc-url https://evm.confluxrpc.com \
  --broadcast --gas-estimate-multiplier 400      # eSpace meters ~1.65M gas vs forge's ~1.16M estimate (testnet gotcha)
# -> IcUSD deployed at 0x<icusd_mainnet>; admin == minter == the canister settlement addr
```

**3. Register chain 1030** (mainnet params; independent RPCs; deeper finality):
```
icp canister call <CANISTER> register_chain '(record {
  chain_id = 1030 : nat32;
  display_name = "ConfluxESpaceMainnet";
  rpc_endpoints = vec { "https://<provider1>"; "https://<provider2>"; "https://<provider3>" };
  finality_depth = 400 : nat32;          # mainnet PoW reorg depth (security-review param; testnet used 100)
  gas_strategy = variant { EvmEip1559 = record { max_priority_fee_gwei = 1 : nat64; max_fee_gwei_ceiling = 200 : nat64 } };
  chain_native_decimals = 18 : nat8;
  min_quorum_providers = opt (2 : nat32);   # >=2 INDEPENDENT providers (audit F-04)
})' -e <ENV> --identity rumi_identity
```

**4. Seed the price, seed the cursor, then bind the contract, in that order.**

Binding the IcUSD contract (`set_chain_contract`) is the step that actually makes the chain PUBLICLY OPEN: `verify_intent_ctx` resolves the bound contract as the EIP-712 domain separator BEFORE it verifies any signature, so an unbound chain rejects every self-serve open regardless of registration or price. Do the price and cursor first so nothing is publicly openable against an unpriced chain.

```
icp canister call <CANISTER> set_manual_collateral_price '(1030 : nat32, "CFX", <price_e8> : nat64)' -e <ENV> --identity rumi_identity   # e.g. $0.15 -> 15_000_000
icp canister call <CANISTER> set_last_observed_block '(1030 : nat32, <current_eSpace_head - 1024> : nat64)' -e <ENV> --identity rumi_identity  # snappy burn detection
icp canister call <CANISTER> set_chain_contract '(1030 : nat32, "0x<icusd_mainnet>")' -e <ENV> --identity rumi_identity                  # <- the public-open gate
```

**4b. Stage the liquidation config row: this is what hands pricing to the automatic XRC feed.** The moment a `chain_liquidation_configs` row exists for a Registered chain, that chain's native pair is XRC-managed: the 300s XRC timer becomes its SOLE writer and `set_manual_collateral_price` refuses every caller, developer included. Staging the row also starts the XRC meter immediately (~1B cycles per call at 300s, roughly 288B cycles/day per symbol); budget from this moment, not from liquidation go-live. The row's `enabled` flag is a SEPARATE switch that gates only the liquidation-swap worker; leave it `false` here.
```
icp canister call <CANISTER> set_chain_liquidation_config '(1030 : nat32, record { ...; enabled = false })' -e <ENV> --identity rumi_identity
icp canister call <CANISTER> get_manual_collateral_price '(1030 : nat32, "CFX")' -e <ENV>   # after one timer tick: XRC is writing
```
Flip `enabled` to true only after its own on-chain validation (`set_chain_liquidation_config` re-derives the factory pair on enable).

**5. Turn the timers on** (currently floored to ~1yr/off on staging):
```
icp canister call <CANISTER> set_observer_tick_interval_secs '(60 : nat64)' -e <ENV> --identity rumi_identity
icp canister call <CANISTER> set_settlement_tick_interval_secs '(60 : nat64)' -e <ENV> --identity rumi_identity
# Interest: leave the harvest timer OFF; realize manually via harvest_chain_interest(1030) when desired,
# OR set_chain_interest_tick_interval_secs to a long cadence. Watch cycle burn (outcall freq x reservation).
```

**6. Smoke test (you-only, tiny):** open one small vault → deposit a few CFX → confirm mint → burn → withdraw → Closed, asserting `reconcile_chain_supply(1030)` gap = 0 at each step (same flow proven on testnet). THEN open the (still small) real positions.

**7. Cycles:** top kvg63/the launch canister well above the freeze threshold — outcalls fail (and silently halt the observer) before freezing. `icp cycles mint ... && icp canister top-up ...`.

---

## Pricing model (supersedes the original audit F-01 manual-oracle plan)

**The automatic XRC feed is authoritative while the chain is Registered AND carries a liquidation config row.** The 300s `xrc::fetch_chains_prices` timer is the ONLY writer of `(1030, "CFX")` in that state, and `set_manual_collateral_price` refuses every caller for that pair: the narrowly-scoped price pusher and the developer alike. Two writers on one cell is the defect the gate prevents; which of them holds the second key does not change that, because the timer fires from its own message on its own schedule with no ordering relationship to an operator's call. The superseded off-chain CFX monitor is an emergency-only fallback now.

What the automatic writer will and will not accept:
- It stamps the sample's **source** timestamp (`ExchangeRate.timestamp`), not arrival time, so a delayed result can never look fresh and the downstream staleness gate measures the age of the observation.
- A candidate must be **strictly newer at the source** than the stored sample, and **within the accepted LIQ-007 sanity band** of the last accepted price (0.7x to ~1.43x). A rejected sample writes NOTHING: not the price, and not the freshness timestamp.
- There is **no auto-confirmation escalation** here. A sustained out-of-band move stays rejected, the stored price ages out, and liquidation fails closed until an operator intervenes. That is deliberate: a genuine 40% move is rare and deserves a human; a fabricated one must never confirm itself.

**`disable_chain` is the reversible emergency risk/XRC stop, and it is a HARD FREEZE while it lasts.** Verified from source, not assumed: it blocks new opens and additional borrows, it unmanages the native pair (the timer stops fetching it and manual pricing reopens), and it ALSO removes the chain from the observer and settlement worker fan-outs (`registered_chains_and_solana_flag` filters to `ChainStatus::Registered`). So a Disabled chain gets no deposit verification, no liquidation detection and no settlement-queue draining. `withdraw_chain_collateral` / `close_chain_vault` still ACCEPT the call and enqueue a settlement op, but with the worker gated off that op is not broadcast until the chain is re-enabled; the only flow that completes unaided on a Disabled chain is repay via `submit_burn_proof`. **Keep the disabled window short and deliberate, and expect pending exits to sit until you re-enable.**

**Manual rebaseline is available only while Disabled**, and `enable_chain` resumes both automatic XRC authority and the workers (so any exit enqueued during the freeze then drains normally). The full recovery loop, which is also what the PocketIC fixtures exercise:
```
icp canister call <CANISTER> disable_chain '(1030 : nat32)' -e <ENV> --identity rumi_identity
icp canister call <CANISTER> set_manual_collateral_price '(1030 : nat32, "CFX", <verified_price_e8> : nat64)' -e <ENV> --identity rumi_identity
icp canister call <CANISTER> get_manual_collateral_price '(1030 : nat32, "CFX")' -e <ENV>     # VERIFY before re-enabling
icp canister call <CANISTER> enable_chain '(1030 : nat32)' -e <ENV> --identity rumi_identity
```
`enable_chain` is developer-gated, flips only `Disabled -> Registered`, refuses an unknown or already-Registered chain, and preserves every per-chain state entry (vaults, supply, contract binding, cursor, prices and their timestamps, liquidation config). It reopens the GATE only: an open after enable still has to satisfy the price presence/staleness prerequisites, which is why the verify step above is part of the loop and not optional.

Still worth an off-chain watcher, for what the canister cannot see:
- Recompute every chain-1030 vault's true CR at the live market price and **alert** on a warning band, so you can act before a vault goes underwater.
- Alert when the on-chain price stops advancing (a fail-closed feed is safe but it is also a frozen one, and it needs the recovery loop above).

---

## Live monitoring + manual risk playbook
- **Invariant:** `reconcile_chain_supply(1030)` gap must stay 0; `get_supply_audit` total == Σ vault debt. Alert on any drift, on `invariant_halted`, on `reorg_halted` (clear via `clear_reorg_halt` after verifying), on hot-wallet low.
- **F-02:** watch `processed_burn_keys` size; a prolonged reorg/halt grows it. Restart-from-snapshot is the escape hatch.
- **Risk (no liquidation):** keep LTV conservative; you are the only opener, so keep every vault well over-collateralized and be ready to repay/close it yourself. This is the substitute for liquidation.

---

## Go / no-go gate for the gated launch
GO when: Decisions 0/1 made · post-upgrade key/RPC/address read-back done (step 1) · ≥2 independent RPCs · the XRC feed observed writing a fresh price and the disable/rebaseline/enable loop rehearsed · tiny cap + discipline · invariant dashboard live · smoke test green · cycles topped. The audit does NOT otherwise block it.

## Before lifting the caps to a PUBLIC launch (the remaining gates)
1. ~~**Liquidation** built~~ DONE (Increments 0-13). Remaining: flip the liquidation config's `enabled` after its on-chain validation, and confirm the swap path against real Swappi depth.
2. **Production `key_1`** (Decision 0) on a dedicated canister.
3. ~~**Automated/timestamped oracle** (audit F-01)~~ DONE. The XRC feed is on-chain, source-timestamped, monotonic and LIQ-007-banded; `set_manual_collateral_price` is locked out while a pair is XRC-managed. See §"Pricing model".
4. **≥3 independent RPC providers** + `min_quorum ≥ 3` (audit F-04).
5. **`processed_burn_keys` eviction** (audit F-02) + the F-05 confirm-status guard.
6. **Debt-ceiling enforcement** in the open path (currently unused) before user-opened (M2) vaults.
7. **M2 self-serve UX** (other session) + a fresh audit of the combined surface.
