# Conflux gated eSpace-mainnet soft-launch — operator runbook

Status: DRAFT for Rob. Local doc (docs/ gitignored). Anchor: `main`@`39a07cd` (PR #251 rail + #252 interest).
Posture: the 2026-06-18 chains-rail audit said **GO for a gated soft-launch with conditions** (`audit-reports/2026-06-18-39a07cd/`).
Goal: real CFX on **eSpace mainnet (chain 1030)**, tightly capped + dev-gated, manual risk management.

---

## ⚠️ Read first — three hard truths that shape every step

1. **There is NO liquidation on the chains rail yet** (deferred to its own spec). So an undercollateralized chain vault cannot be liquidated — there is no recovery path for bad debt. The ONLY protections are: conservative LTV, tiny caps, monitoring, and the fact that **`open_chain_vault` is developer-gated** (only you open vaults). → **Keep the launch dev-gated. Do NOT wire M2 self-serve (user-opened vaults) to mainnet until liquidation exists.** With dev-only opens you are managing your own positions; that is the safe shape of this soft-launch.

2. **The debt ceiling is NOT code-enforced** (`debt_ceiling_e8s` is carried in `ChainCollateralConfig` but the open path consumes only `min_cr_e4`). So the "small cap" is **operational** — it is you opening few small vaults, not a guardrail. Treat the cap as a discipline, not a safety net.

3. **The ECDSA key is hardcoded to `test_key_1`** (`chains/monad/config.rs::monad_ecdsa_key_name`). Real CFX would be custodied by tECDSA `test_key_1`. Moving to production `key_1` is a **code change** (M2's area) + redeploy + a full re-derivation of every custody/settlement/interest-treasury address + an IcUSD.sol redeploy with the new minter. See Decision 0.

---

## Decision 0 (REQUIRED — your call): which ECDSA key
| | `test_key_1` (current) | `key_1` (production) |
|---|---|---|
| Code change | none | edit `monad_ecdsa_key_name()` → `"key_1"` (chains source = M2 session) + redeploy |
| Addresses | already derived (testnet-proven) | ALL custody/settlement/treasury addresses change → IcUSD.sol must be redeployed with the new minter |
| Risk for real CFX | `test_key_1` is a real threshold key but designated "test" (weaker operational guarantees) | production-grade |
| Speed | fast (interim) | slower (code + coordination + key availability on the subnet) |

**Recommendation:** for an *initial, tiny, short-duration* gated launch (a few CFX, you-only), `test_key_1` is the fast path and is consistent with "soft-launch". **Migrate to `key_1` before holding non-trivial value or lifting caps.** If you want real value from day one, do `key_1` first (it is a hard pre-req, not optional, for meaningful TVL).

## Decision 1 (your call): which canister
- **kvg63 staging** (already runs the rail + interest, `test_key_1`): fastest. Adding chain 1030 here puts real eSpace-mainnet CFX on the staging canister. Fine for a tiny dev-only soft-launch; keep it small.
- **Production `tfesu`** (`key_1`): mixes the chains rail's real-CFX custody into the live ICP-CDP canister. Bigger blast radius; only after Decision 0 = `key_1` + a fresh audit pass of the combined surface. **Not recommended for the soft-launch.**
- **New production-keyed canister** for the chains rail: cleanest separation for scale; more setup.

**Recommendation:** kvg63 for the initial gated launch; a dedicated production-keyed canister when you go to `key_1`/scale.

---

## Pre-launch checklist (audit's go-with-conditions)
- [ ] Decision 0 (key) + Decision 1 (canister) made.
- [ ] **≥2 independent mainnet RPC providers** secured (NOWNodes / BlockPi / Validation Cloud) — NOT all Confura (audit F-04). Register with `min_quorum_providers ≥ 2`.
- [ ] CFX price-monitoring + auto-refresh running (audit F-01 — see §"Price monitoring" below). **This is the single most important condition** because the oracle is manual.
- [ ] Tiny initial cap agreed (e.g. ≤ a few hundred USD of debt total) and a written discipline that you (the only opener) won't exceed it.
- [ ] Monitoring dashboard live: `reconcile_chain_supply(1030)` gap = 0, `invariant_halted`/`reorg_halted` false, hot-wallet balance, `processed_burn_keys` size (audit F-02).
- [ ] Deployer key funded with real CFX for the IcUSD.sol deploy gas (~1.65M gas; eSpace meters high).
- [ ] (Optional, cheap) the F-05 defense-in-depth one-liner coordinated with the M2 session.

---

## Operator sequence (templates — fill the placeholders; use `--identity rumi_identity`)

> If Decision 0 = `key_1`: FIRST land the `monad_ecdsa_key_name()` → `"key_1"` change (M2 session) + redeploy the backend, because every address below changes with the key.

**1. Get the canister's chain-1030 settlement (minter) address** (this is what IcUSD.sol's MINTER_ROLE must be):
```
icp canister call <CANISTER> get_chain_settlement_address '(1030 : nat32)' -e <ENV> --identity rumi_identity
# -> 0x<minter>   (per-chain, key-dependent; derived from settlement_derivation_path(1030))
```

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

**4. Point the chain at the IcUSD contract + set the CFX price + seed the cursor:**
```
icp canister call <CANISTER> set_chain_contract '(1030 : nat32, "0x<icusd_mainnet>")' -e <ENV> --identity rumi_identity
icp canister call <CANISTER> set_manual_collateral_price '(1030 : nat32, "CFX", <price_e8> : nat64)' -e <ENV> --identity rumi_identity   # e.g. $0.15 -> 15_000_000
icp canister call <CANISTER> set_last_observed_block '(1030 : nat32, <current_eSpace_head - 1024> : nat64)' -e <ENV> --identity rumi_identity  # snappy burn detection
```

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

## Price monitoring (audit F-01 — the most important condition)

The oracle is a manual `set_manual_collateral_price` scalar with **no on-chain timestamp or staleness check**. A stale/unrefreshed price is the highest real risk. Mitigation = an **off-chain monitor** (a small cron/script or an IC canister timer; NOT chains source — I can build it):
- Poll real CFX/USD from **≥2 sources** (e.g. Coingecko + Binance) every N minutes; take the median.
- Compare to the on-chain price (query the canister); **auto-call `set_manual_collateral_price`** when |market − on-chain| > X% (e.g. 2%), and ALWAYS at least every Y minutes (since there's no on-chain age, the monitor owns freshness).
- Recompute every chain-1030 vault's *true* CR at the live price; **alert** if any vault < a warning band (e.g. 1.6×) so you can act before it goes underwater (no liquidation exists — your action is to ask the owner to add collateral / repay, or close your own vault).
- Alert on monitor downtime (a dead monitor = a frozen oracle).

---

## Live monitoring + manual risk playbook
- **Invariant:** `reconcile_chain_supply(1030)` gap must stay 0; `get_supply_audit` total == Σ vault debt. Alert on any drift, on `invariant_halted`, on `reorg_halted` (clear via `clear_reorg_halt` after verifying), on hot-wallet low.
- **F-02:** watch `processed_burn_keys` size; a prolonged reorg/halt grows it. Restart-from-snapshot is the escape hatch.
- **Risk (no liquidation):** keep LTV conservative; you are the only opener, so keep every vault well over-collateralized and be ready to repay/close it yourself. This is the substitute for liquidation.

---

## Go / no-go gate for the gated launch
GO when: Decisions 0/1 made · ≥2 independent RPCs · price monitor+auto-refresh live · tiny cap + discipline · invariant dashboard live · smoke test green · cycles topped. The audit does NOT otherwise block it.

## Before lifting the caps to a PUBLIC launch (the remaining gates)
1. **Liquidation** built (the #1 gap — no public launch without it).
2. **Production `key_1`** (Decision 0) on a dedicated canister.
3. **Automated/timestamped oracle** (audit F-01) — replace the manual price + monitor with on-chain staleness or XRC/Pyth.
4. **≥3 independent RPC providers** + `min_quorum ≥ 3` (audit F-04).
5. **`processed_burn_keys` eviction** (audit F-02) + the F-05 confirm-status guard.
6. **Debt-ceiling enforcement** in the open path (currently unused) before user-opened (M2) vaults.
7. **M2 self-serve UX** (other session) + a fresh audit of the combined surface.
