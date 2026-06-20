# Rumi Chains-Rail Liquidation Engine — Design Spec

Status: design, pre-implementation. Target: `rumi_protocol_backend` (chains/ tree). Conflux eSpace is the first row; chain-agnostic by construction. The whole chains tree is EXPERIMENTAL, dev-gated, timers idled to 1 year (banner `chains/mod.rs:1`); this engine ships disabled-by-default and respects that gating.

All numbers below are reconciled against the actual code (`chains/collateral_config.rs`, `chains/vault.rs`) and the locked decisions, not against the drafts' assumptions. Where a draft asserted a value the code contradicts, the code wins and the discrepancy is flagged.

---

## 1. Overview, goals, non-goals

### Goal
Build the missing liquidation path for foreign-chain CDP vaults (`MultiChainStateV5.chain_vaults`). Today there is zero liquidation logic in `chains/` (confirmed: no `liquidat*` anywhere in `chains/vault.rs`/`chains/mod.rs`; only the inert `liquidation_penalty_bps` config field). This plus an unenforced `debt_ceiling_e8s` is the explicit prod blocker per audit memory. The engine detects under-collateralized chain vaults, partially liquidates them to restore CR to target, and does so through a three-tier cascade.

### The three tiers (locked)
1. **Bot (PRIMARY, canister-as-bot).** The backend derives its own eSpace address via threshold-ECDSA and signs+submits a Swappi swap selling seized CFX → USDC. The USDC becomes protocol reserve. **The bot NEVER burns icUSD.** Total circulating icUSD is UNCHANGED; backing shifts CFX→USDC-reserve; vault debt drops.
2. **ICP Stability Pool (FALLBACK).** When the bot can't clear (min-out unmet / swap fail / timeout / DEX too thin), the canonical ICP SP BURNS icUSD (supply drops), absorbs the chain-vault debt, and takes the CFX at the liquidation discount. SP depositors later CLAIM their CFX share to an EVM address they provide. **One SP attempt, then manual. NEVER add SP retries** (standing rule, past incident).
3. **Manual (FALLBACK).** Anything bot+SP can't handle.

### Locked invariants this engine must honor
- PARTIAL liquidation only (sell just enough to restore CR to target, never full-close).
- Vault LOCKS when collateral is handed to the bot for swap; REOPENS (debt↓, collateral↓) the moment the bot confirms USDC in hand — decoupled from the slow USDC→Ethereum→cUSDT bridge leg (bridge is OUT OF SCOPE for automation).
- Chain-agnostic per-chain config so future EVM chains (Base/Arbitrum) plug in as data, not code.
- `reconcile_chain_supply` and the supply invariant MUST count bot-held USDC as backing, or the invariant false-halts.

### Non-goals
- The USDC→Ethereum→cUSDT bridge automation (manual, out of scope; represented as a durable reserve term).
- Conflux native/virtual SP (deferred).
- ICP↔chains debt unification (Phase 2; the foreign-chain supply invariant stays a separate pool).
- Solana/Monad swap encoders (the abstraction supports them; only Conflux/Swappi ships first).
- Recovery-mode (global TCR-driven) for chains (the engine uses a fixed per-vault threshold; chains debt is excluded from ICP TCR today and stays excluded).

---

## 2. Architecture: where code lives

```
chains/
  collateral_config.rs   risk params (min_cr_e4, penalty, recovery target) — EXTEND (wire inert fields)
  liquidation_config.rs   NEW: ChainLiquidationConfigV1 + DexKind (persisted, Tier-B, operator-set)
  liquidation.rs          NEW: generic state-machine orchestration + DexAdapter trait + dex_adapter_for()
  vault.rs                EXTEND: begin_liquidation_in_state() alongside open/borrow/withdraw/close;
                                  Liquidating status; pending_liquidation marker
  supply.rs               EXTEND: unified invariant (debt + reserve + pending_chain_burn);
                                  apply_supply_delta signature change; apply_debt_to_reserve_shift()
  multi_chain_state.rs    BUMP: MultiChainStateV6 (reserve maps, liq-config map, sp_attempted set)
  settlement_queue.rs     EXTEND: SettlementOpKind::LiquidationSwap variant
  evm/
    settlement.rs         EXTEND: build_tx_plan / resolve_op_signer / submit_op / confirm_op arms;
                                  apply_liquidation_settlement_in_state()
    tx.rs                 EXTEND: MonadTxKind::Swap arm; dynamic address[] ABI encoder
    evm_rpc.rs            EXTEND: get_reserves(), get_pair_token0(), get_logs Transfer decode
    tecdsa.rs             EXTEND: reserve_derivation_path / cached_reserve_address
    conflux/swappi.rs     NEW: SwappiV2Adapter (UniswapV2 calldata + getAmountsOut/getReserves)
  event.rs                EXTEND: ChainVaultLiquidated, ChainReserveCredited, ChainCfxClaimSettled,
                                  ChainLiquidationDeferred
  xrc.rs                  EXTEND: Timer-B self-check operand → unified invariant; liquidation scan tick

stability_pool/           EXTEND (Tier 2): cfx_claims u128 map; claim_cfx; CFX sentinel collateral;
                                  stability_pool_liquidate_chain_vault entry
```

**Generic vs per-chain split (the seam).** Generic (chain-agnostic): the liquidation state machine, CR/trigger/partial math, reserve accounting, the settlement-op lifecycle, the per-vault guard. Per-chain (the ONLY thing a new chain writes): the swap calldata encoder + on-chain quote read + address format, behind the `DexAdapter` trait. A same-family DEX (another Uniswap-V2 fork) reuses the V2 adapter verbatim — it is data (a config row), not code.

---

## 3. State + versioning (MultiChainStateV6)

Every persisted change below rides ONE `MultiChainStateV6` snapshot bump. The recipe (proven V1→V5, documented `multi_chain_state.rs:8-30`): keep `MultiChainStateV5` byte-verbatim, add `MultiChainStateV6` carrying V5's fields unchanged + new fields with `#[serde(default)]`, rebind `pub type MultiChainState = MultiChainStateV6`. The four original V1 fields (`chain_configs`, `chain_supplies`, `settlement_queues`, `invariant_halted`) MUST NOT carry `serde(default)`. Ciborium decodes in place by field name; new fields/enum-variants come up empty/absent in old snapshots, so NO `post_upgrade` migration is needed. A bare field/variant add WITHOUT the bump triggers the AMM-style UPG-002 state wipe.

### 3.1 New `ChainVaultStatus` handling — marker field, NOT a new status (RESOLVED)

The drafts forked on whether to add a `Liquidating` status variant or reuse `Open` + a marker. **Decision: reuse `Open` + a `pending_liquidation` marker field; do NOT add a `Liquidating` status variant.** Rationale:
- The marker is strictly less invasive: no new enum variant to thread through every `match ChainVaultStatus` site (avoiding the exhaustiveness-audit risk the state-machine draft flagged), and no risk a wildcard arm mishandles a new status.
- A non-zero `pending_liquidation` marker is exactly the existing Design-B precedent (`pending_mint_e8s`, `pending_interest_mint_e8s`): "this much is mid-settlement, may revert." The owner-write guards already key off non-zero pending markers, so locking comes for free.
- `count_owner_active_vaults` (filters `!= Closed`) already counts a marked vault as active — no change.

New field on `ChainVaultV1`:
```rust
#[serde(default)]
pub pending_liquidation: Option<PendingLiquidationV1>,

pub struct PendingLiquidationV1 {
    pub op_id: u64,
    pub debt_to_clear_e8s: u128,      // debt this seize is sized to retire
    pub collateral_reserved_native: u128, // CFX (wei) decremented from collateral_amount_native
    pub tier: LiquidationTier,         // Bot (others reserve collateral differently — see §6)
    pub started_at_ns: u64,
}
```
While `pending_liquidation.is_some()`, the vault is "in the liquidating window." Owner write-ops (`withdraw_collateral_in_state`, `borrow_chain_vault_in_state`, `close_chain_vault_in_state`) reject it the same way they reject `MintInFlight` — a new `LiquidationInFlight` rejection arm, checked right beside the existing `pending_mint_e8s != 0` check (`vault.rs:154`). Status stays `Open` throughout; the vault never leaves `Open` for a bot liquidation (it either stays `Open` with reduced debt/collateral, or short-circuits to `Closed` if drained).

### 3.2 New persisted fields on `MultiChainStateV6`

```rust
// --- reserve accounting (bot/PSM path) ---
#[serde(default)] pub reserve_backing_e8s: BTreeMap<ChainId, u128>,
//   icUSD (e8s) whose backing shifted CFX->USDC, no longer vault debt but not yet burned.
//   RHS term-2 of the unified invariant. Grows on bot-liquidation confirm; shrinks ONLY
//   when a human bridges + the foreign icUSD is burned (settle_reserve_burn).
#[serde(default)] pub reserve_usdc_native: BTreeMap<ChainId, u128>,
//   Physical USDC (18-dec base units on eSpace) the reserve address holds. Bookkeeping of
//   the asset, distinct from the icUSD-denominated backing; gap reveals realized slippage.

// --- SP (burn) path ---
#[serde(default)] pub pending_chain_burn_e8s: BTreeMap<ChainId, u128>,
//   icUSD burned IC-side by the SP but not yet burned on eSpace. RHS term-3 of the unified
//   invariant. Moves to a chain_supplies decrement when the eSpace Burn op confirms.
#[serde(default)] pub chain_liquidation_claims: BTreeMap<u64, ChainLiqClaimV1>,
//   CFX custody-claim ledger (per liquidation event). Backend = custodian; SP = apportioner.
#[serde(default)] pub sp_attempted_chain_vaults: BTreeSet<u64>,
//   Chains analog of sp_attempted_vaults; one SP shot per vault. (Transient routing state.
//   It rides V6 because it is in the persisted root, but it is reset/pruned on resolution;
//   surviving an upgrade is harmless — worst case a vault waits one extra tick for manual.)

// --- chain-agnostic liquidation config (Tier B, operator-settable) ---
#[serde(default)] pub chain_liquidation_configs: BTreeMap<ChainId, ChainLiquidationConfigV1>,
```

`SettlementOpKind::LiquidationSwap` (a new enum variant inside `SettlementQueueV1.pending`) and the `ChainVaultV1.pending_liquidation` field are BOTH inside the V6 root, so they are covered by the same bump. Required: a round-trip decode test against a live V5 snapshot proving the bump is non-destructive.

### 3.3 New invariant accessors (mirror `total_chain_vault_debt_e8s`)
```rust
pub fn total_reserve_backing_e8s(&self) -> u128 { self.reserve_backing_e8s.values().copied().sum() }
pub fn total_pending_chain_burn_e8s(&self) -> u128 { self.pending_chain_burn_e8s.values().copied().sum() }
```

---

## 4. Tier 1 — Bot (PSM) liquidation

### 4.1 Confirmed parameters (reconciled against code + locked decisions)

The locked decision says "150% min CR, 133% liquidation threshold, 155% recovery." The code (`collateral_config.rs:30-39`, Conflux/`ICP_MIRROR`) carries:

| Concept | Field | Value | Status in code | This spec |
|---|---|---|---|---|
| Open/borrow/withdraw gate | `min_cr_e4` | 13_300 (133%) | **LIVE** (vault.rs:275/504/706) | see §4.1.1 — flag |
| Mint headroom / restore source | `borrow_threshold_e4` | 15_000 (150%) | INERT | open-gate candidate |
| Liquidation discount | `liquidation_penalty_bps` | 1_200 (12%) | INERT | **bonus = 12%, see §4.1.2** |
| Interest | `interest_apr_bps` | 200 (2%) | LIVE | used (interest-aware CR) |
| Restore target | `recovery_target_cr_e4` | 15_500 (155%) | INERT | **restore target = 155%** |
| Min vault debt | `min_vault_debt_e8s` | 10_000_000 (0.10 icUSD) | INERT | enforce on open (§9) |
| Debt ceiling | `debt_ceiling_e8s` | None | INERT | **set + enforce (§9)** |

The chains rail has **no distinct liquidation-threshold field** today; only `min_cr_e4` is a live gate. The engine introduces the liquidation threshold by reading `min_cr_e4` (133%) as the trigger and `recovery_target_cr_e4` (155%) as the restore target.

#### 4.1.1 RESOLVED: open gate stays 133%, trigger = 133%, restore = 155%
The two state-machine drafts noted the chains rail opens at 133% (`min_cr_e4`), which is *looser* than ICP's 150% open gate, and asked whether to introduce a separate `liquidation_ratio_e4` distinct from the open gate.

**Decision: do NOT introduce a separate `liquidation_ratio_e4`. Trigger liquidation at `CR < min_cr_e4` (133%), restore to `recovery_target_cr_e4` (155%), keep the open gate where it is — but raise the Conflux open gate to 150% (`borrow_threshold_e4`).** This is the single concrete change that reconciles the code with the locked "150% min CR" and gives a sane buffer:
- Open/borrow gate → 150% (use `borrow_threshold_e4`). This matches the locked "150% min CR" and ICP.
- Liquidation trigger → CR < 133% (`min_cr_e4` repurposed as the liquidation threshold).
- Restore target → 155% (`recovery_target_cr_e4`).

This gives the ICP-identical 150%-open / 133%-liquidate / 155%-restore profile with real headroom (a fresh 150% vault is not instantly liquidatable). The implementation: `evm_vault_params` returns `borrow_threshold_e4` (150%) as the open/borrow gate; the liquidation engine reads `min_cr_e4` (133%) as the trigger. **This is the ONE genuine fork that needs Rob's sign-off** — it changes the open gate for existing/new Conflux vaults from 133%→150%. If Rob prefers to keep the experimental rail's looser 133% open gate, then trigger must be set *below* 133% (e.g. a new 125% threshold) so opens aren't instantly liquidatable, but that diverges from ICP. **Recommendation: raise to 150% to mirror ICP.** Either way the partial math and restore target are unaffected.

#### 4.1.2 RESOLVED: discount = 12% (penalty), not 15% (bonus)
The drafts split on 12% (`liquidation_penalty_bps`) vs ICP's 15% (`liquidation_bonus`). **Decision: use the chains config's own value, `liquidation_penalty_bps = 1_200` (12%).** The chains config is the source of truth for the chains rail; ICP's 15% is a different rail. So `bonus_e4 = 10_000 + liquidation_penalty_bps = 11_200` (a repaid icUSD releases 1.12× its value in collateral). This is parameterized on the config field — never hardcode 1.12 vs 1.15.

### 4.2 CR computation (interest-aware)

Reuse `collateral_ratio_e4(collateral_native, native_decimals, price_e8, debt_e8s)` (`vault.rs:180`) verbatim — saturating, returns `u64::MAX` for zero debt. Feed it interest-adjusted debt so a vault underwater only by accrued interest does not escape:
```
effective_debt_e8s = debt_e8s
                   + pending_interest_mint_e8s          // reserved, not yet in debt
                   + accrued_chain_interest_e8s(debt_e8s, apr_bps,
                         now_ns.saturating_sub(last_interest_accrual_ns))
```
`accrued_chain_interest_e8s` already exists and is overflow-safe (`interest.rs`). The accrual window (`now − last_interest_accrual_ns`) MUST be byte-identical to `harvest_chain_interest_in_state`'s window or the CR is wrong. Do NOT include `pending_mint_e8s` (an unconfirmed borrow) — vaults with `pending_mint_e8s != 0` are excluded entirely (§4.4).

`collateral_value_e8s = collateral_amount_native × price_e8 / 10^native_decimals` — factor into a shared helper so the CR check and the sizing use byte-identical math.

### 4.3 Price source + staleness gate (audit F-01 dependency, MANDATORY)

CFX is priced from `manual_prices[(71, "CFX")]`, written by the off-chain CFX monitor (price-pusher-scoped `set_manual_collateral_price`). Confirmed gap: `manual_price_set_at_ns` is recorded but enforced NOWHERE (open/withdraw read `manual_prices` raw). Liquidation is the most dangerous consumer of a stale price (stale-high hides an underwater vault → protocol loss; stale-low liquidates a healthy vault → unjust seizure). **Liquidation MUST add a fail-closed staleness gate**, and should be the first consumer (factor a shared helper for open/withdraw to adopt later):
```
fn fresh_chain_price_e8(state, chain, symbol, now_ns) -> Result<u64, PriceError> {
    let (price_e8, set_at_ns) = state.multi_chain.get_manual_price(chain, symbol).ok_or(NoPrice)?;
    if price_e8 == 0 { return Err(ZeroPrice); }
    if set_at_ns == 0 { return Err(NoTimestamp); }            // pre-V5 price -> fail closed
    if now_ns.saturating_sub(set_at_ns) > max_price_age_ns { return Err(Stale); }
    Ok(price_e8)
}
```
`max_price_age_ns` is a per-chain `ChainLiquidationConfigV1` field (§8). **Fail-closed semantics:** a stale price defers *chain* liquidations only (skip vault, emit `ChainLiquidationDeferred{reason: StalePrice}`, alarm) — it does NOT latch the whole protocol to ReadOnly (asymmetric with the ICP oracle breaker, deliberately). This makes the off-chain monitor's uptime a hard production SLO; monitor staleness must be alarmed. **Recommended default: `max_price_age_ns ≈ 2–3× the monitor's push interval` (start 30 min, tune to the actual F-01 push cadence).**

### 4.4 Guards that block liquidation (honor existing invariants)
- `pending_mint_e8s != 0` → `MintInFlight`: a borrow mint could confirm and increase debt after seize → debt unbacked. **Exclude.**
- `pending_interest_mint_e8s != 0` → `InterestRealizationPending`: same exclusion.
- `pending_liquidation.is_some()` → already mid-liquidation. **Exclude** (no double-lock).
- status must be `Open`.
- `invariant_halted || reorg_halted[chain]` → state untrusted, do NOT liquidate (the settlement worker already skips on these — `settlement.rs:293-297` — so a halted chain can't execute a swap; the trigger must skip too).

### 4.5 Trigger architecture
Detection runs on the **existing chain observer/settlement tick** (not a new timer), so it inherits the dev-gating and the 1-year idle floor; when an operator sets the interval < 1yr, detection runs each tick. Per chain, per tick:
```
price_e8 = fresh_chain_price_e8(state, chain, "CFX", now)?   // Err -> defer ALL vaults this chain
if invariant_halted || reorg_halted[chain]: continue
for v in chain_vaults where collateral_chain==chain && status==Open
                         && pending_mint_e8s==0 && pending_interest_mint_e8s==0
                         && pending_liquidation.is_none():
    eff_debt = effective_debt_e8s(v, apr_bps, now)
    cr_e4    = collateral_ratio_e4(v.collateral_amount_native, native_decimals, price_e8, eff_debt)
    if cr_e4 < min_cr_e4 (133%):
        plan = size_liquidation(v, eff_debt, price_e8, cfg)   // §4.6
        route(plan)   // bot first; on bot-fail/timeout -> SP (once); manual always
cap candidates routed to bot this tick at max_liq_per_tick (e.g. 3; queue is one-op-in-flight)
```
A permissionless/manual `#[update] liquidate_chain_vault(vault_id)` runs the IDENTICAL gate and routes to the manual/SP path; while dev-gated it is `developer_principal`-only, matching every chain write endpoint. Identical gates mean a manual caller can never liquidate a vault the timer wouldn't.

### 4.6 Partial-liquidation amount math (restore to 155%)

Port the ICP formula (`compute_partial_liquidation_cap`, `state.rs:3877`) to integer e8s/e4, all saturating, restore-target = `recovery_target_cr_e4` (155%), bonus = `10_000 + liquidation_penalty_bps` (11_200):
```
fn sized_repay_e8s(eff_debt_e8s, collateral_value_e8s, target_cr_e4, bonus_e4) -> u128 {
    let numerator = eff_debt_e8s.saturating_mul(target_cr_e4 as u128) / 10_000;
    if numerator <= collateral_value_e8s { return 0; }       // at/above target
    if target_cr_e4 <= bonus_e4 { return eff_debt_e8s; }     // denom<=0 -> full
    let deficit = numerator - collateral_value_e8s;
    let denom_e4 = (target_cr_e4 - bonus_e4) as u128;
    (deficit.saturating_mul(10_000) / denom_e4).min(eff_debt_e8s)
}
```
Truncating division rounds `repay` DOWN → restored CR lands slightly ABOVE target (protocol-favorable, never over-liquidates). **Mandatory property test: `restored_cr_e4 >= target_cr_e4` across fuzzed inputs.**

Collateral to sell = `repay_e8s` grossed up by the penalty so the swap output covers the debt:
```
collateral_in_native = (repay_e8s × bonus_e4 / 10_000) × 10^native_decimals / price_e8
                       capped at collateral_amount_native
```
If the cap binds (collateral can't cover even the penalty-grossed partial), size down to all available collateral and let residual debt fall to SP/manual.

### 4.7 DEX-depth cap (the genuinely novel piece)

On ICP, `sized_repay` is always executable (protocol custodies + transfers instantly). On Conflux the bot must SELL CFX into a ~$91k Swappi WCFX/USDC pool; at >~$1–3k per swap slippage exceeds any sane cap ($10k ≈ 15%). So `sized_repay` is frequently larger than the DEX can absorb. Two layers:

- **Trigger layer (sizing):** clamp to the config policy ceiling `max_swap_value_e8s` (USD e8s, ≈$2k start): `effective_repay = min(sized_repay, value_cap_to_repay(max_swap_value_e8s))`.
- **Executor layer (submit time):** read live Swappi reserves (`getReserves`/`getAmountsOut`), compute `amountOutMin = getAmountsOut × (1 − max_slippage_bps)`, and if `effective_repay`'s collateral can't meet `amountOutMin`, **do NOT swap → escalate to SP**.

**Partial-of-partial loop.** When `sized_repay > effective_repay`, the vault can't restore to target in one swap. The bot sells `effective_repay` this tick, CR climbs part-way, and the NEXT tick re-scans and sizes another chunk. Naturally idempotent and self-terminating: each swap reduces debt+collateral, CR climbs monotonically until it crosses 133% and the vault drops out. The per-vault `pending_liquidation` marker prevents two in-flight swaps for the same vault, so no explicit loop state is needed. Large vaults restore over many ticks — acceptable for the tiny-debt experimental rail, and the reason the debt ceiling (§9) matters.

**Min-out unmet → escalate.** If live depth can't absorb even a minimum economical chunk at the slippage cap, the bot does NOT swap (locked: hard min-out). Mark `bot_failed`, escalate to SP (one shot), then manual.

### 4.8 The tECDSA Swappi swap (bot execution)

**Swap kind decision: `swapExactETHForTokens(uint256 amountOutMin, address[] path, address to, uint256 deadline)`, native CFX carried in the EIP-1559 `value` field. NO pre-wrap, NO token-in path.** The custody address holds native CFX; `swapExactETHForTokens` wraps internally (router requires `path[0] == WCFX`). The token-in path (`swapExactTokensForTokens`) forces two sequential txs (approve+swap) against a strictly one-op-in-flight queue, doubling failure surface and gas — rejected. `path = [WCFX 0x14b2d3bc…, USDC 0x6963EfED…]`; settle to USDC (deeper pool, ~$91k). All eSpace tokens are 18-dec — a 1e10 scale conversion at the e8s boundary.

**Calldata encoder (new, `tx.rs`).** Needs the dynamic `address[]` ABI encoding the current helpers lack (`abi_word_address`/`abi_word_u128` only do static head words). New `encode_swap_exact_eth_for_tokens_calldata(amount_out_min, path, to, deadline)`:
- selector from the LITERAL deployed router signature. **CRITICAL: confirm the on-chain Swappi router signature before shipping.** Canonical Uniswap-V2 is `swapExactETHForTokens(uint256,address[],address,uint256)` (selector `0x7ff36ab5`). A wrong selector reverts every swap (caught by confirm, but wastes attempts and makes the bot tier look broken). Verify against the deployed router ABI (the router, NOT the factory `0xe2a6f7…`).
- head: `amount_out_min`, offset `0x80`, `to`, `deadline`; tail: `path.len()=2`, `path[0]`, `path[1]`.
- returns `Err` on any malformed address (defense-in-depth; never panic the worker after the guard is held).
- `to` = the per-chain reserve recipient (USDC lands there in one tx, no second transfer).

Also needed: a `getReserves()` decoder (selector `0x0902f1ac`, returns `(uint112,uint112,uint32)`), `token0()` (`0x0dfe1681`, V2 pairs sort by address so ordering must be read not assumed), and `getAmountsOut`. Reuse the `eth_call` quorum template (`erc20_total_supply_at`, `parse_eth_call_u128`) for the reads.

**amountOutMin (safety-critical, just-in-time).** Computed inside `submit_op` from a live reserves read in the SAME tick that signs — never cached. New pure fn `compute_amount_out_min(amount_in_wei, reserve_in, reserve_out, fee_bps, slippage_bps)`:
1. V2 constant-product with fee (Swappi 0.25% → `fee_bps=25`, CONFIRM on mainnet): `amount_in_with_fee = amount_in*(10_000-25)`; `expected_out = amount_in_with_fee*reserve_out / (reserve_in*10_000 + amount_in_with_fee)`.
2. haircut: `amount_out_min = expected_out * (10_000 - slippage_bps) / 10_000`.
3. **oracle cross-check (the DEX-depth gate):** convert `expected_out` (18-dec USDC) to e8s and compare to oracle-implied USD value of the seized CFX. If `expected_out_usd_e8 < oracle_value_e8 × (10_000 − max_dex_oracle_divergence_bps)/10_000` (e.g. 500 bps), the pool prices CFX far below oracle (thin/manipulated) → **do NOT swap; escalate to SP.**

Any of {reserves read errors / returns 0, `amount_out_min`==0, oracle cross-check fails, custody can't cover gas} → do NOT sign, mark op `Failed`, emit `ChainLiquidationSwapFailed`, escalate. **Fail-closed: no swap without a satisfiable, oracle-corroborated min-out.**

**Deadline, nonce, gas.** `deadline = ic_cdk::api::time()/1e9 + deadline_secs` (config, ~180s). **Swap ops are EXCLUDED from `resubmit_if_stuck` (no replace-by-fee).** A replace-by-fee minutes later would re-use the original min-out and could execute into a moved price — exactly the "no auto-retry into worse slippage" the locked decision forbids. A stuck swap simply hits its on-chain `deadline` and reverts; the IC confirm-timeout then marks it Failed → escalate. **IC confirm-timeout MUST be set > deadline + finality** so a never-mined swap reverts on-chain before the IC gives up (else the SP could absorb debt the swap later settles — a double-spend). Nonce via `get_transaction_count(custody_addr)`, stored as `submit_nonce`. Gas: reuse `fetch_fees` (eth_gasPrice 90/10, `max_fee = 2*base + prio`). **`gas_limit` hard-coded 250_000 for the swap kind** (measure on eSpace mainnet before shipping; consider 400_000 headroom — it's a ceiling, only used gas is charged; the Mint arm hit OOG at 120k before bumping to 300k). Net the CFX `value` against gas via a `fundable_swap_value` mirroring `fundable_withdrawal_value`: `amount_in.min(custody_balance − gas_limit*max_fee)`; if that goes to 0, do NOT submit → escalate.

**Signer + destination.** Signer = the vault's OWN custody address (`custody_derivation_path(chain, owner, vault_id)`, reusing `resolve_op_signer`'s `NativeWithdrawal` arm) — it holds the CFX paid as `value`, never commingled to the hot wallet. Destination (USDC `to`) = a deterministic per-chain RESERVE address: new `reserve_derivation_path(chain) = [chain_le, b"liquidation-reserve"]` + `cached_reserve_address(chain)`, modeled on `cached_settlement_address`. This reserve address is the PSM sink; the bridge sweeps FROM it (out of scope).

**Confirm.** Reuse `confirm_op`'s shape (receipt, `status_ok`, `block <= finalized`). On success, decode the USDC `Transfer(→ reserve_recipient)` log in the receipt block (`get_logs`, `TRANSFER_TOPIC0`) to read the REALIZED `usdc_out_native` — do NOT trust min-out as the actual amount. That realized amount drives reserve credit + debt-wipe (§5). On revert, restore reserved collateral under the existing CAS (§5b) and escalate.

### 4.9 The two-phase bot accounting (claim → swap → confirm)

**Phase 1 — `begin_liquidation_in_state` (lock, reserve, enqueue). No invariant move.** A new helper in `vault.rs` alongside open/borrow/withdraw/close, parameterized on the same `address_validator: fn(&str)->bool` + `price_symbol: &str` seams. Steps (enqueue-then-flip ordering, no mutation on any reject, mirroring `withdraw_collateral_in_state:518-545`):
1. Gate (§4.4): status `Open`; `pending_mint==0`; `pending_interest_mint==0`; `pending_liquidation.is_none()`.
2. Fresh price (§4.3, fail-closed).
3. CR check (§4.2): `cr_e4 < min_cr_e4` else `NotLiquidatable`.
4. Size (§4.6, §4.7): `debt_to_clear_e8s`, `collateral_in_native` clamped to depth cap + price-impact bound.
5. Validate `router` via `address_validator` BEFORE enqueue (avoid deep tx-builder panic).
6. Enqueue `LiquidationSwap` op (idempotency key `liquidate-{chain}-{vault}-{now_ns}`).
7. ONLY after successful enqueue: `collateral_amount_native -= collateral_in_native` (reserve), set `pending_liquidation = Some(PendingLiquidationV1{op_id, debt_to_clear_e8s, collateral_reserved_native, tier: Bot, started_at_ns})`.
8. Caller acquires `ChainVaultLiquidationGuard::new(vault_id)` (held across the async swap; see §10).

Debt/supply are UNCHANGED at trigger (Design B). Collateral is reserved (decremented) but logically still owned until confirm.

**Phase 2 — `apply_liquidation_settlement_in_state` (USDC in hand). The single invariant move.** Called from `confirm_op`'s `LiquidationSwap` success branch, modeled on `confirm_interest_mint_in_state`:
1. Validate read-only (no mutation on fail): vault present, `pending_liquidation.is_some()`, op matches, `usdc_out_native` valued in USD `>= debt_to_clear_e8s` (else §4.10 shortfall — but strict min-out should make this impossible).
2. Re-read LIVE `debt_e8s` (a permissionless burn may have shrunk it). `actual_cleared = min(pending_liquidation.debt_to_clear_e8s, live_debt_e8s)` (saturating).
3. **Move debt → reserve** via the single guarded helper `apply_debt_to_reserve_shift` (§5). `debt_e8s -= actual_cleared`; `reserve_backing_e8s[chain] += actual_cleared`; `reserve_usdc_native[chain] += usdc_out_native`. **`chain_supplies` is NOT touched** (no icUSD burned).
4. Clear `pending_liquidation`. Collateral already debited in Phase 1.
5. If `debt_e8s==0 && collateral_amount_native==0` → `Closed`; else stays `Open` (debt↓, collateral↓) — REOPENS immediately, decoupled from the bridge.
6. Emit `ChainVaultLiquidated` + `ChainReserveCredited`. Mark op `Succeeded`; drop guard.

---

## 5. Reserve accounting + the unified supply invariant

### 5.1 The problem
The bot reduces `debt_e8s` WITHOUT burning icUSD, so `chain_supplies` stays put while debt drops. That breaks today's `sum(chain_supplies) == total_chain_vault_debt_e8s()` and would flip `invariant_halted=true` → GA→ReadOnly on the next Timer-B check (`xrc.rs:377`), freezing BOTH observer and settlement worker — bricking the rail. **This is the single highest-risk integration point.**

### 5.2 The unified invariant (RHS split into three terms)
```
sum(chain_supplies)  ==  total_chain_vault_debt_e8s()
                       +  total_reserve_backing_e8s()      (bot/PSM path)
                       +  total_pending_chain_burn_e8s()   (SP path, pre-eSpace-burn)
```
Every circulating foreign icUSD is backed by EITHER an open vault's collateral (debt term), OR protocol-held USDC reserve (reserve term), OR an IC-side SP burn awaiting its eSpace burn (pending-burn term).

- **Bot path:** moves an amount debt → reserve. LHS unchanged. Conservation: `sum` unchanged.
- **SP path (§6):** moves debt → pending_chain_burn at IC-burn time (LHS unchanged); then pending_chain_burn → (chain_supplies decrement) when the eSpace `Burn` op confirms (real burn, both drop).
- **Existing mint/burn helpers:** still move `chain_supplies` and `debt_e8s` together; they pass the reserve+pending terms through unchanged.

### 5.3 Mutation gates
Keep the discipline that NOTHING writes `chain_supplies`/the reserve maps directly.
- `apply_supply_delta` (`supply.rs:65`) — **signature change**: take the explicit RHS components, not a single `total_debt`. Recommended (drafts agreed): pass `(debt_total, reserve_total, pending_burn_total)` or a combined `total_rhs_e8s` computed by each caller, and compare `sum_after(chain_supplies)` against it. **Recommendation: explicit components**, so a future caller cannot forget the reserve/pending term. No-mutation-on-rejection preserved.
- New `apply_debt_to_reserve_shift(state, chain, amount, pre_debt_total)` — the single gate for the reserve term (bot Phase 2). Validates the FULL invariant post-move, rejects (no mutation) on divergence/halt. Does NOT touch `chain_supplies`.
- SP IC-burn writedown uses a `apply_supply_delta`-routed move into `pending_chain_burn`; the eSpace-burn confirm uses `apply_supply_delta(Decrease)` on `chain_supplies` paired with a `pending_chain_burn` debit.

### 5.4 The four consumers MUST change in lockstep (one PR)
If any one disagrees it FALSE-HALTS (→ ReadOnly) or FALSE-PASSES a real unbacked mint:
1. `apply_supply_delta` (`supply.rs:99`)
2. `check_invariant` (`supply.rs:116`)
3. Timer-B self-check operand (`xrc.rs:367`) → pass `debt + reserve + pending_burn`
4. `reconcile_chain_supply` (`main.rs:2347-2348`) + the observer totalSupply backstop (`deposit_watch.rs` divergence alarm) → treat `reserve_backing` (and the in-flight pending-burn) as legitimate backing so a bot liquidation does NOT look like an unbacked mint / trigger a catch-up burn sweep. Add `reserve_backing_e8s`, `reserve_usdc_native`, `pending_chain_burn_e8s` to `ChainSupplyReconciliation` so the operator sees the breakdown.

A test must exercise a bot liquidation AND an SP liquidation on the same chain and assert no halt + correct `reconcile` attribution.

### 5.5 The slow bridge leg (reserve retirement)
`reserve_backing_e8s` is the durable, invariant-safe representation of "backing in transit." It shrinks ONLY via a developer-gated manual `settle_reserve_burn(chain, amount_e8s)`, called after a human bridges USDC and the corresponding foreign icUSD is BURNED on eSpace.

**RESOLVED (bridge settlement semantics): the bridge BURNS the reserve-backed foreign icUSD.** `settle_reserve_burn` then drops BOTH `chain_supplies` (real eSpace burn observed at finality, via the existing `apply_burn`-style path) AND `reserve_backing_e8s` together — structurally identical to `apply_burn_to_state` except it debits the reserve term instead of a vault's `debt_e8s`. This keeps the foreign invariant self-contained and is the only model under which the conservation table closes. (The alternative — icUSD stays circulating with backing re-homed to ICP-side reserves — defers to the Phase-2 unified ledger and is explicitly out of scope.) `reserve_usdc_native` is reconciled against real on-chain USDC custody via an `erc20 balanceOf` read (same machinery as `erc20_total_supply_at`); a `get_chain_reserves(chain)` getter surfaces `{reserve_backing_e8s, reserve_usdc_native, onchain_usdc_balance}` so the operator verifies books==custody before bridging.

### 5.6 Penalty surplus
The gap between `reserve_usdc_native` (USD-valued) and `reserve_backing_e8s` is the realized 12% penalty surplus (USDC received beyond debt cleared). **Decision: surplus accrues as protocol reserve** — it stays in `reserve_usdc_native` and is NOT credited as additional `reserve_backing_e8s` (backing tracks only the debt actually retired). The surplus is protocol revenue realized at the bridge leg; it does not reduce future backing obligations.

---

## 6. Tier 2 — ICP Stability Pool cross-chain fallback

Reached only when the bot can't clear. The SP burns IC-native icUSD (it can prove this), absorbs the chain-vault debt, takes the CFX at the 12% discount; the CFX stays in the vault's tECDSA custody; SP depositors claim their share to an EVM address they provide.

### 6.1 Two halves, kept independently consistent
1. **Debt + supply (IC-side, synchronous).** SP burns IC icUSD with a `SpWritedownProof`; backend writes down `debt_e8s`. Because an IC-ledger burn does NOT reduce eSpace `totalSupply()`, the writedown moves the amount into `pending_chain_burn_e8s` (NOT `chain_supplies`) and enqueues an eSpace `Burn` settlement op. When that eSpace burn confirms at finality, `pending_chain_burn → chain_supplies` decrement. **RESOLVED: option A (decouple).** This preserves "vault reopens the moment debt is absorbed, decoupled from the slow leg" and reuses the existing `Burn` op + finality observer. (Option B — burn on eSpace first — couples the SP to EVM finality and violates the decoupling principle; rejected.)
2. **Collateral (EVM-side, async).** The seized CFX never moves at liquidation time; it stays in custody. Record a synthetic CFX claim; depositors pull it via a tECDSA-signed `NativeWithdrawal` later.

### 6.2 New backend SP entry
`stability_pool_liquidate_chain_vault(vault_id, icusd_burned_e8s, proof: SpWritedownProof, depositor_evm_claims)` — SP-caller-gated (clone of `stability_pool_liquidate_debt_burned`/`main.rs:3299`). Steps (saga-ordered):
1. Verify the burn proof against `icusd_ledger_principal` (reuse `fetch_and_validate_block`).
2. Chains gates: reject if `invariant_halted`/`reorg_halted`; require a FRESH manual CFX price (same staleness gate §4.3).
3. Acquire the chains per-vault guard; re-read LIVE `debt_e8s`/`collateral_amount_native` (TOCTOU). Honor `MintInFlight`/`InterestRealizationPending`.
4. `burned_e8s = min(icusd_burned_e8s, live debt)`. **The cap MUST be enforced by the SP re-quoting inside its guard and burning EXACTLY the capped amount** — IC icUSD cannot be un-burned, and unlike the ckStable path there is no proportional refund here. Over-burn = un-refundable bad debt against depositors.
5. Writedown: `debt_e8s -= burned_e8s` + `pending_chain_burn_e8s[chain] += burned_e8s` via the guarded helper (§5.3); enqueue the eSpace `Burn` op.
6. Seized CFX = `restore_repay_native × (1 + liquidation_penalty_bps/10_000)` capped at collateral; restore-to-`recovery_target_cr_e4` (155%) using the §4.6 math.
7. Reserve the collateral (`collateral_amount_native -=`, the withdraw-reserve pattern) and record a `ChainLiqClaimV1{vault_id, chain, custody_address, seized_native_total, paid_native: 0}`. Set `pending_liquidation = Some(..tier: Sp..)`. Do NOT enqueue any payout op yet.
8. Emit `ChainVaultLiquidatedBySp`. Vault reopens immediately.

### 6.3 SP-side accounting (mostly free, two real changes)
- **CFX as an SP collateral via a sentinel principal.** `collateral_gains`/`opted_out`/`compute_token_draw`/`process_liquidation_gains_at`/`effective_pool_for_collateral` are all `Principal`-keyed and collateral-agnostic — reuse verbatim. Register CFX as a `CollateralInfo{ledger_id: <deterministic per-chain sentinel>, symbol:"CFX", decimals:18, status:Active}`. The sentinel MUST be deterministic-from-chain_id, stable across upgrades, and never collide with a real ledger principal. The one rule: never treat the sentinel as a transferable ICRC ledger.
- **u128 CFX entitlements (MANDATORY, not optional).** `process_liquidation_gains_at`'s `collateral_gained` is `u64`, but a meaningful CFX seizure ($1–3k at ~$0.048 ≈ 1e22 wei) **overflows u64** and would silently truncate depositor entitlements. Add a PARALLEL field `DepositPosition.cfx_claims: BTreeMap<Principal, u128> (#[serde(default)])` and a CFX-aware sibling of `process_liquidation_gains_at` that credits `cfx_claims[sentinel]` in u128 (apportionment math identical: pro-rata to `user_consumed_e8s / total_consumed_e8s`; only the accumulator type + target map change). Depositors supply the EVM address at claim time (no stored address).
- **Coverage gate unchanged.** `effective_pool_for_collateral(cfx_sentinel) >= debt` asks "do opted-in depositors hold enough stables to cover this debt?" — correct regardless of collateral.

### 6.4 CFX claim path (`claim_cfx`)
Replaces `claim_collateral`'s `icrc1_transfer` with a backend-authorized EVM settlement. `claim_cfx(sentinel, dest_evm_address)`:
1. SP-102 busy guard (reject if `liquidation_in_progress()`).
2. Validate `dest_evm_address` (EVM `0x`+40hex) BEFORE any mutation.
3. **Deduct-before-async:** read `owed = cfx_claims[sentinel]`, zero it FIRST.
4. Call backend `claim_chain_collateral(claimer, owed_wei, dest_evm_address)` → enqueues a `NativeWithdrawal{recipient: dest, amount_e18: owed_wei}` signed by the seized vault's custody key, increments `paid_native` on the claim.
5. Rollback: on backend error, restore `cfx_claims[sentinel] = owed`. Handle `Duplicate` like `claim_collateral` (do NOT restore). If the EVM settlement later REVERTS, the backend's revert path restores custody under the CAS AND must signal the SP to re-credit `cfx_claims` (a backend→SP callback or depositor re-claim) — else the depositor's deduct-before-async loses the entitlement.

**Custody-claim invariant** (replaces `validate_state`'s aggregate==ledger for CFX, which does NOT apply — the SP never physically holds CFX): per claim `paid_native <= seized_native_total`, and `sum_depositors(entitlement − claimed) == seized_native_total − paid_native`.

### 6.5 No-retries, two phases
- **Phase A (absorb): ONE shot.** Burn + `stability_pool_liquidate_chain_vault`, under `SpLiquidationGuard` (SP-102). On any failure (proof fail, busy guard, stale price), the SP does NOT retry; backend marks `sp_attempted_chain_vaults`; vault falls to Tier 3. This is the no-retry boundary.
- **Phase B (depositor claims): retryable, depositor-driven, NOT a liquidation retry.** A failed `claim_cfx` can be re-attempted by the depositor. The standing no-retry rule is about re-attempting to ABSORB a vault, which Phase A does exactly once. **Document loudly in code** that no retry may ever wrap the absorb or the backend `Burn` op.

### 6.6 Open decisions for Tier 2 (flagged, recommendations given)
- **Non-icUSD draws.** For a chain-vault burn the SP must produce a REAL icUSD burn. ckUSDT/ckUSDC/3USD draws are not icUSD; the ICP path lets the backend net the debt against on-chain collateral it ships back, but here there is no on-chain collateral to ship. **Recommendation: the SP converts/holds the drawn stable as IC-side reserve backing the absorbed chain debt, then burns the equivalent icUSD — or restrict chain-vault SP absorption to icUSD draws in v1** (simplest; deepen later). Needs Rob's call.
- **Per-vault vs pooled custody for claims.** Each seized vault's CFX sits in its OWN custody address (`owner+vault_id`-derived). A depositor accruing claims across many liquidations needs N `NativeWithdrawal` ops (one per source vault). **Recommendation: per-source payout (one op per source vault) in v1** — avoids an extra trust/timing sweep hop; revisit a sweep-into-pool if dust-claim economics bite.
- **Unclaimed CFX.** If a depositor never claims (lost key), the CFX is reserved out of custody indefinitely. **Recommendation: no expiry/sweep in v1; document the custody-address lifecycle implication** (a fully-claimed seized vault can close; an unclaimed remainder keeps it pinned).

---

## 7. Tier 3 — Manual

The permissionless/dev-gated `#[update] liquidate_chain_vault(vault_id)` is the manual entry. It runs the IDENTICAL gate as the timer (fresh price, CR < 133%, guards) and routes to the SP path or, if SP coverage fails / was already attempted, leaves the vault liquidatable for a human. While the rail is dev-gated this endpoint is `developer_principal`-only; at soft-launch it becomes the permissionless fallback. **Manual on the chains rail = a human invoking this endpoint** (driving the SP absorb or, if SP can't, a direct developer-driven seize). It is NOT a human burning icUSD + claiming CFX off-chain outside the canister (that would bypass the invariant). The trigger must NOT assume Tier 2 succeeds — if the SP lacks coverage or the price is stale, the vault simply sits liquidatable until conditions allow, surfaced via `get_chain_liquidatable_vaults`.

---

## 8. Chain-agnostic LiquidationConfig

**Tier-B (persisted, operator-settable, versioned).** DEX addresses/slippage/depth-cap must be tunable live (Swappi could redeploy a router, liquidity moves, the safe per-swap cap is re-measured against live depth) without a canister upgrade, and addresses MUST be validated at set-time (chain-aware) so a malformed address can never panic the settlement worker.

```rust
pub enum DexKind { UniswapV2 /*, future: UniswapV3 { fee_tier_bps: u32 } */ }

pub struct ChainLiquidationConfigV1 {
    pub enabled: bool,                  // master switch; defaults false (stage addresses, flip last)
    pub dex_kind: DexKind,
    pub router: String,                 // the swap target (Swappi ROUTER, not the factory)
    pub factory: String,                // for a getReserves/pair sanity read
    pub wrapped_collateral_token: String, // WCFX (path[0] + getAmountsOut)
    pub settle_stable_token: String,    // USDC (settle into / hold as reserve)
    pub settle_stable_decimals: u8,     // 18 on eSpace (NOT a constant; a future chain may be 6)
    pub pair: String,                   // pinned deep pool (WCFX/USDC 0x0736…), not factory-derived
    pub reserve_recipient: String,      // the tECDSA reserve address (USDC `to`)
    pub max_slippage_bps: u32,          // hard slippage cap (start 200–300 bps)
    pub max_dex_oracle_divergence_bps: u32, // pool-vs-oracle gate (e.g. 500 bps)
    pub max_swap_value_e8s: u128,       // depth-bound: max USD value per single swap (~$2k)
    pub max_price_age_ns: u64,          // staleness ceiling for the CFX manual price (§4.3)
    pub deadline_secs: u64,             // on-chain swap deadline (~180s)
}
```
Deliberately NOT in this struct (single source of truth elsewhere): **restore-target CR** (read from `ChainCollateralConfig.recovery_target_cr_e4`), **liquidation threshold + penalty** (read from `ChainCollateralConfig`), **swap gas limit** (per-`DexKind` constant, not a footgun knob), **swap deadline value** (derived from `ic_cdk::time()` at submit; `deadline_secs` is the horizon).

**Setter** `set_chain_liquidation_config(chain, cfg)` — dev-gated, all-or-nothing validation (mirror `set_chain_contract`): caller is `developer_principal`; chain registered + has an `EvmChainConfig`; every address valid EVM `0x`+40hex; `max_slippage_bps <= 10_000`; `settle_stable_decimals in 1..=36`; `max_swap_value_e8s > 0`. No mutation on any failure. Getter `get_chain_liquidation_config(chain)`. The new types enter the `.did` — the mod.rs banner warns chains `.did` sync is unverified, so add them to the candid file and run the breaking-change check; **never `-y` past a warning on this stable-memory canister.**

**The DexAdapter seam.** The only per-chain code. `DexAdapter` (DEX analogue of `ChainAdapter`): `encode_seize_swap(cfg, collateral_native, min_out, now_ns) -> SwapCalldata` and `async quote_amount_out(cfg, collateral_native) -> u128`. `SwappiV2Adapter` (`DexKind::UniswapV2`) is the first impl (the §4.8 encoder + `getAmountsOut`). `dex_adapter_for(chain, dex_kind)` is the factory. A new same-family V2 chain reuses the adapter verbatim (rename to `UniswapV2Adapter`); only a new DEX family (V3) needs a new variant + impl. The state machine never matches on `dex_kind`.

**`SettlementOpKind::LiquidationSwap`** (rides the V6 bump):
```rust
LiquidationSwap { vault_id: u64, collateral_in_native: u128, min_usdc_out_native: u128,
                  debt_to_clear_e8s: u128, router: String, pair: String,
                  path: Vec<String>, reserve_recipient: String, deadline_secs: u64 }
```
`min_usdc_out` stored on the op is advisory only — recomputed JIT at submit from the live reserves read. Wiring: `build_tx_plan` gains a `LiquidationSwap` arm (calls `DexAdapter::encode_seize_swap`); `resolve_op_signer` returns the custody arm; `build_eip1559_fields` gains a `MonadTxKind::Swap` arm (gas 250_000); `confirm_op` gains success (Transfer decode → §4.9 Phase 2 / §6 SP-burn) and revert (CAS restore → escalate) arms; swap ops EXCLUDED from `resubmit_if_stuck`.

**Onboarding Conflux:** `set_chain_liquidation_config(71, {dex_kind: UniswapV2, router: <Swappi router>, factory: 0xe2a6f7…, pair: 0x0736…, wrapped_collateral_token: 0x14b2d3bc…, settle_stable_token: 0x6963EfED…, settle_stable_decimals: 18, reserve_recipient: <reserve addr>, max_slippage_bps: 250, max_dex_oracle_divergence_bps: 500, max_swap_value_e8s: <≈$2k>, max_price_age_ns: <30min>, deadline_secs: 180, enabled: false})`, register `SwappiV2Adapter`, dry-run on mainnet-fork/mock-router, then flip `enabled: true`. **Operator MUST supply the real Swappi ROUTER address** (the liquidity facts give the FACTORY `0xe2a6f7…`, which cannot execute swaps). A set-time sanity check that the factory-derived pair for `[wrapped_collateral, settle_stable]` equals the pinned `pair` would catch a misconfig that silently routes through a shallow pool.

---

## 9. Oracle/price dependency, gating, depth-bound debt cap

- **Oracle:** the off-chain CFX monitor (F-01) is a HARD production dependency. Monitor down → fresh-price gate fails → all chain liquidations defer (fail-closed). Monitor staleness MUST be alarmed (an underwater vault can sit unliquidated). The staleness gate (§4.3) is the ONLY thing standing between a stale price and an unjust seizure.
- **Gating:** the engine ships with `ChainLiquidationConfigV1.enabled=false` and refuses to run on a chain with no config row. Detection rides the existing dev-gated observer tick (1-yr idle floor). The settlement worker already skips `ReadOnly`/`invariant_halted`/`reorg_halted`. All write endpoints are `developer_principal`-only while experimental.
- **Debt-ceiling enforcement (prod blocker — MUST ship WITH liquidation).** `debt_ceiling_e8s` (None today) and `min_vault_debt_e8s` (0.10 icUSD) are defined but enforced NOWHERE. The debt ceiling bounds max vault/chain debt → bounds how much collateral a liquidation must ever sell → makes the depth-bound model tractable. Add to `open_chain_vault_in_state` AND `borrow_chain_vault_in_state`, after the CR check:
  ```
  if let Some(ceiling) = chain_collateral_config(chain).debt_ceiling_e8s {
      if per_chain_debt_sum(chain) + new_debt_e8s > ceiling { return Err(DebtCeilingExceeded); }
  }
  if new_total_debt_e8s < chain_collateral_config(chain).min_vault_debt_e8s { return Err(BelowMinDebt); }
  ```
  Set Conflux `debt_ceiling_e8s` to a small soft-launch value (a few hundred icUSD) sized so the ENTIRE chain's debt is liquidatable within a handful of depth-capped swaps. Enforce `min_vault_debt_e8s` so dust vaults (uneconomical to liquidate given gas) can't be created. **Recommendation: Conflux `debt_ceiling_e8s = ~300 icUSD` soft-launch, keep `min_vault_debt_e8s = 0.10 icUSD`** — both tunable later (a future follow-up may promote these to Tier-B persisted config).
- **Circuit breaker / bad-debt accounting.** The ICP mass-liquidation breaker (`record_liquidation_for_breaker`) and `protocol_deficit_icusd` are ICP-vault-scoped. **Recommendation: chains liquidations get their OWN per-chain counters in v1** (do not pollute ICP counters); a chains bad-debt deficit (swap delivered less USDC than debt cleared — which strict min-out should prevent) gets a per-chain home. This is a follow-up, not a v1 blocker, because strict min-out makes the bot-path shortfall case unreachable.

---

## 10. Error handling + async/interleave safety

- **Per-vault guard.** Reuse the `VaultLiquidationGuard` PATTERN (`guard.rs:210`, keyed on `u64`, collateral-agnostic). **Recommendation: a separate `ChainVaultLiquidationGuard` set** to keep ICP and chain id-spaces clean; the mechanism is identical. Held by the async caller from trigger through settle. It blocks liquidation-vs-liquidation (two ticks, or bot-vs-SP racing one vault).
- **Durable lock = the `pending_liquidation` marker (status gate).** The heap guard does NOT survive an upgrade; the persisted `pending_liquidation` marker does. The marker (not the guard) is the load-bearing lock against owner interleaving: owner write-ops reject while it is `Some`. Verify the marker + reserved collateral are fully reconstructable from persisted state alone, so settle/revert works post-upgrade. An in-flight `LiquidationSwap` op stays in the persisted queue and resumes on the next tick.
- **TOCTOU / re-cap.** Sizing is computed at scan time; the swap confirms async later. The sized `repay` is an UPPER-BOUND intent. Phase 2 (§4.9) and the SP entry (§6.2) MUST re-read LIVE `debt_e8s`/`collateral_amount_native` + re-accrue interest at commit (AR-B-001 pattern). A concurrent permissionless burn shrinking debt is handled by `saturating_sub` + `min(intent, live_debt)`. The claim-time tolerance band mirrors `get_bot_claim_max_ratio_for` (accept a vault up to `min_cr_e4 + tolerance_bps`, recommend 200 bps, so a vault that drifted just above threshold isn't rejected).
- **Revert restore under CAS.** The `LiquidationSwap` revert branch reuses the EXACT `NativeWithdrawal` revert CAS (`still_inflight`, `settlement.rs:879`): `collateral_amount_native += collateral_reserved_native`, clear `pending_liquidation`, mark op `Failed`, escalate. The CAS prevents a double add-back on overlapping ticks (which would let the owner withdraw collateral the protocol no longer holds).
- **MintInFlight/InterestRealizationPending.** Both `begin_liquidation` and the swap submit path defensively re-check `pending_mint==0 && pending_interest_mint==0` before signing (cheap read-only), deferring if either is nonzero.
- **No `read_state`/`mutate_state` borrow across `.await`** (established discipline). Each phase is a single synchronous state mutation with no-mutation-on-rejection; the async EVM-RPC calls live in the settlement worker BETWEEN phases.
- **Failure matrix (all → escalate to SP, never auto-retry into worse slippage):** reserves-read error/0, min-out==0, oracle cross-check fail, custody can't cover gas → do NOT sign. On-chain revert / deadline passed → CAS restore → escalate. Never-mined past `IC-timeout > deadline+finality` → mark Failed (no replace-by-fee), escalate.

---

## 11. Testing strategy

- **Pure unit tests (no DEX needed):** `sized_repay_e8s` (property: `restored_cr_e4 >= target`), `compute_amount_out_min` (known V2 reserves vector + a min-out==0 → do-not-swap assertion), the dynamic `address[]` calldata encoder against a `cast`/Foundry-generated reference (the only place the codebase ABI-encodes a dynamic type — highest encoder-bug risk), `fundable_swap_value`, the interest-aware `effective_debt_e8s` window alignment.
- **PocketIC happy path:** open CFX vault, push price down, bot swap succeeds (mock router) → assert `debt_e8s` ↓, `chain_supplies` UNCHANGED, `reserve_backing_e8s` += cleared, `reserve_usdc_native` += realized, invariant holds, no halt, vault reopened.
- **PocketIC bot→SP escalation:** mock router min-out unmet → bot Failed → SP absorbs → assert `debt_e8s` ↓, `pending_chain_burn_e8s` += burned, eSpace Burn op enqueued; on Burn confirm, `pending_chain_burn → chain_supplies` decrement; `ChainLiqClaimV1` recorded; depositor `cfx_claims` (u128) credited pro-rata.
- **Invariant cross-check:** a bot (reserve) AND an SP (burn) liquidation on the same chain → assert `sum(chain_supplies) == debt + reserve + pending_burn` and `reconcile_chain_supply` does NOT false-alarm.
- **Burn-proof gate, no-retry, over-burn cap, u128 overflow (seize > u64::MAX wei), claim revert+re-credit, V5→V6 round-trip decode, staleness defer.**
- **Testnet-no-liquidity gap (hard constraint).** eSpace TESTNET (chain 71, where the rail runs) has NO real Swappi liquidity. The swap leg is exercisable only on eSpace MAINNET (chain 1030) or a mainnet-fork / mock-router. For PocketIC, extend the EVM-RPC mock/override (`evm_rpc_override`) with canned `eth_call getReserves`/`token0`/`getAmountsOut` and an `eth_getTransactionReceipt` carrying a USDC `Transfer` log, so the submit do-not-swap branches and the confirm realized-out decode are coverable without a live DEX. **Rebuild canister wasms after any rebase** (PocketIC `include_bytes!`s prebuilt wasm).

---

## 12. Phasing (first mergeable increment vs follow-ups)

**Increment 0 — debt ceiling + min-debt enforcement (ship FIRST, independently).** Wire `debt_ceiling_e8s` + `min_vault_debt_e8s` on open/borrow, set Conflux ceiling. This is the prod blocker's other half and is a small, isolated, independently-testable change that makes the depth-bound model tractable. No V6 bump needed (compile-time config + open-path check).

**Increment 1 — state + invariant foundation (the V6 bump).** `MultiChainStateV6` with all new maps + `pending_liquidation` marker + `LiquidationSwap` op variant; the unified invariant across all four consumers; `apply_debt_to_reserve_shift`; `apply_supply_delta` signature change; events; `set/get_chain_liquidation_config`; staleness-gate helper; raise Conflux open gate to 150% (pending Rob's §4.1.1 sign-off). Ships with NO liquidation execution yet — purely the accounting + config scaffolding, with the V5→V6 round-trip test. This de-risks the highest-blast-radius piece (the false-halt) before any swap exists.

**Increment 2 — Tier 1 bot, sizing + detection (no swap execution).** `begin_liquidation_in_state`, the detection tick, `sized_repay_e8s`, the depth cap, `get_chain_liquidatable_vaults`. Routes intents but stops at enqueue — coverable by pure + PocketIC tests against the mock without a live DEX.

**Increment 3 — Tier 1 bot, swap execution.** The `DexAdapter`/`SwappiV2Adapter`, the calldata encoder, `get_reserves`/`getAmountsOut`, `compute_amount_out_min` + oracle cross-check, the `LiquidationSwap` submit/confirm/revert arms, reserve address. The bot tier is now end-to-end on a mock router; mainnet-fork validates the real DEX leg before `enabled=true`.

**Increment 4 — Tier 2 SP fallback.** The CFX sentinel + u128 `cfx_claims`, `stability_pool_liquidate_chain_vault`, `pending_chain_burn` + eSpace Burn op, `claim_cfx`, the escalation routing (`sp_attempted_chain_vaults`).

**Increment 5 — follow-ups.** `settle_reserve_burn` + bridge reconciliation getter, per-chain circuit-breaker/bad-debt counters, promoting `debt_ceiling_e8s`/`min_vault_debt_e8s` to Tier-B persisted config, factoring the staleness gate into open/withdraw.

---

## Decisions — RESOLVED (Rob, 2026-06-19)

1. **Raise Conflux open gate 133%→150%: YES** (mirrors ICP per Rob's "same params as ICP" decision: 150 open / 133 liquidate / 155 restore).
2. **Tier-2 non-icUSD draws: restrict chain-vault SP absorption to icUSD draws in v1** (conservative default).

Original fork write-up retained below for context.

### (original) Decisions needing sign-off
1. **Raise Conflux open gate 133%→150%?** (§4.1.1) — Recommendation: YES, mirror ICP (150% open / 133% liquidate / 155% restore). The only alternative keeps the looser 133% open gate and pushes the liquidation trigger below 133%, diverging from ICP. This changes behavior for existing Conflux vaults, so it needs explicit approval.
2. **Tier-2 non-icUSD draws** (§6.6) — Recommendation: restrict chain-vault SP absorption to icUSD draws in v1, or hold the drawn stable as IC-side reserve. Needs a call.

Everything else (12% penalty not 15% bonus, restore to 155%, marker field not new status, decouple-and-pending-burn for SP, burn-model bridge settlement, penalty-surplus-as-reserve, per-source CFX payout, separate chain guard/counters) is resolved with a concrete recommendation above and does not need to block implementation.

---

Spec file references (all absolute): the engine touches `/Users/robertripley/coding/rumi-protocol-v2/.worktrees/chains-liquidation/src/rumi_protocol_backend/src/chains/{collateral_config.rs, vault.rs, supply.rs, multi_chain_state.rs, settlement_queue.rs, event.rs, xrc.rs, evm/{settlement.rs, tx.rs, evm_rpc.rs, tecdsa.rs}}`, new files `chains/{liquidation_config.rs, liquidation.rs, evm/conflux/swappi.rs}`, and `/Users/robertripley/coding/rumi-protocol-v2/.worktrees/chains-liquidation/src/stability_pool/src/{lib.rs, liquidation.rs, deposits.rs, state.rs, types.rs}`.

---

## Appendix A — Adversarial review findings (fold into implementation)

A 4-lens adversarial review of this spec surfaced 40 findings. Each MUST be addressed (or consciously waived) during the increment that touches it. Condensed:


**1. [HIGH] Interest-realization vs reserve-shift conservation gap (the unified invariant double-counts or under-counts pending interest)**
- Issue: The unified invariant RHS is defined as debt + reserve_backing + pending_chain_burn, where debt = total_chain_vault_debt_e8s() = sum(v.debt_e8s). But the existing interest path (confirm_interest_mint_in_state, settlement.rs:147-187) grows BOTH v.debt_e8s AND chain_supplies together by observed_e8s — interest mints NEW foreign icUSD. The spec's effective_debt_e8s (4.2) deliberately adds pending_interest_mint_e8s + freshly-accrued interest on top of debt_e8s purely for the CR/sizing decision, which is fine. The danger is sizing-vs-clearing: sized_repay is computed against effective_debt (which includes un-realized interest), but Phase 2 (4.9 step 2) clears actual_cleared = min(debt_to_clear_e8s, live_debt_e8s) against the LIVE debt_e8s only. If a vault has pending_interest_mint_e8s != 0, the spec already excludes it (4.4). But there is NO exclusion for a vault whose accrued (not-yet-harvested) interest pushed it underwater: liquidation sizes debt_to_clear to cover interest-inclusive deficit, yet can only move REALIZED debt_e8s into reserve. The over-sized collateral seizure produces USDC reserve crediting reserve_backing_e8s += actual_cleared (capped at live debt), but the EXTRA collateral sold to cover the interest portion yields USDC with NO matching debt to retire and NO matching supply — it silently inflates reserve_usdc_native above reserve_backing_e8s and is bucketed as 'penalty surplus' (5.6). That mislabels seized-principal-covering-unrealized-interest as protocol revenue, and worse, the interest is never realized (debt_e8s never grew for it, supply never grew for it), so the protocol seized collateral against debt that does not exist on either side of the invariant.
- Fix: Before sizing, MANDATORILY harvest+realize (or at minimum exclude) any vault with non-zero accrued interest: either (a) require pending_interest_mint_e8s == 0 AND accrued_since_last_accrual == 0 (force a harvest tick first, defer liquidation one tick), or (b) realize accrued interest into debt_e8s + supply via the existing confirm_interest_mint path BEFORE begin_liquidation so debt_to_clear only ever targets real, on-both-sides debt. Add a property test: for any liquidated vault, (reserve_usdc_native_delta valued in USD) - (reserve_backing_e8s_delta) == exactly the 12% penalty on actual_cleared, never more. Any excess means interest leaked into 'surplus'.


**2. [HIGH] Four-consumer false-halt: apply_supply_delta strict-equality check will reject the reserve shift unless ALL consumers change atomically, and the spec's signature change is under-specified for the interest path**
- Issue: apply_supply_delta (supply.rs:65-105) and check_invariant (supply.rs:111-120) both enforce STRICT equality sum(chain_supplies) == total_debt_e8s. The bot path (apply_debt_to_reserve_shift, 4.9/5.3) reduces debt_e8s WITHOUT touching chain_supplies. The Timer-B self-check (verified at xrc.rs:367: chain_debt_e8s = total_chain_vault_debt_e8s(); check_invariant(&s.multi_chain, chain_debt_e8s)) passes ONLY total_chain_vault_debt_e8s(). The instant the first bot liquidation lands, sum(chain_supplies) > total_chain_vault_debt_e8s() by exactly actual_cleared, check_invariant returns Divergence, invariant_halted=true, GA->ReadOnly — bricking the rail (settlement worker skips on ReadOnly/invariant_halted, settlement.rs:293-297). The spec identifies this (5.1/5.4) but the failure mode is more fragile than stated: this is not a 'change four things' refactor, it is a HARD ordering/atomicity requirement. apply_supply_delta is ALSO called by confirm_mint_in_state and apply_burn_to_state which currently pass post_mint_total / post_burn_total computed as total_chain_vault_debt_e8s()-derived deltas; changing its signature to take (debt_total, reserve_total, pending_burn_total) means EVERY existing caller must now also pass reserve+pending terms or the FIRST normal mint after the bump false-halts (mint adds debt but reserve/pending default to 0 in old callers => sum_after = debt only, but RHS now expects debt+reserve+pending => still equal only if reserve=pending=0, which holds at first but the callers must still thread the new args).
- Fix: Specify the apply_supply_delta migration as a single mechanical pass with a compile-time forcing function: make the new signature take an explicit struct InvariantTotals{debt, reserve, pending_burn} (no Default) so the compiler flags every un-updated caller. Add a regression test that runs a bot liquidation followed immediately by a Timer-B tick and asserts mode stays GeneralAvailability and invariant_halted stays false. Critically, the check must be DERIVED from the same state read (read all three totals in one read_state closure) — never pass a stale debt snapshot from before the reserve shift, or the strict-equality check false-halts on a benign interleave. Ship Increment 1 (the bump + all four consumers + a deliberately-injected reserve delta in the test) BEFORE any swap code, exactly as phased.


**3. [HIGH] Depth-bound cap is NOT enforced before the swap commits — only at submit, and the sized cap can be stale relative to live reserves**
- Issue: The lens question 'is the depth-bound cap actually enforced before a swap' has a subtle gap. There are two layers (4.7): the TRIGGER/sizing layer clamps to max_swap_value_e8s (a static config USD ceiling), and the EXECUTOR layer (4.8 compute_amount_out_min) reads LIVE reserves at submit and refuses if min-out unmet. But begin_liquidation_in_state (Phase 1, 4.9) RESERVES collateral and enqueues the op based on the SIZING-time clamp, which uses ONLY the static max_swap_value_e8s, NOT live reserves. The live-reserves depth check happens later inside submit_op. Between enqueue and submit, the op holds reserved collateral and the vault is locked. If live depth has collapsed (other swaps drained the pool, or the pool was always thinner than max_swap_value_e8s implies — $2k cap on a $91k pool is ~2.2% of depth, fine, but a $300 chain debt ceiling with a $2k swap cap means a single vault can be the whole ceiling and still exceed prudent single-swap impact if the pool moved), the submit refuses and escalates — but the static cap NEVER guaranteed the swap was executable. The cap is a USD-value ceiling, not a depth-fraction ceiling. Nothing in the config bounds the swap to a FRACTION of live reserve_in; max_swap_value_e8s is a fixed dollar number that does not track liquidity moving. The locked liquidity facts say $10k = 15% slippage on a $91k pool; if liquidity halves to $45k, the same $2k cap is now ~4.4% impact, still passing a 2.5% slippage cap's min-out only marginally, and a $2k notional could blow the 250 bps slippage gate — forcing escalation, which is correct, but the SIZING never knew.
- Fix: Move a CHEAP depth read into the sizing/trigger path, OR make the cap a fraction of live reserve_in rather than a static USD value: add max_swap_reserve_fraction_bps (e.g. 200 bps of reserve_in) and clamp collateral_in_native = min(value_cap, getReserves-derived reserve_in * fraction) at sizing time, re-validated at submit. This makes the depth-bound cap a function of live liquidity, not a guessed dollar figure that goes stale as the pool moves. At minimum, document that max_swap_value_e8s MUST be re-tuned whenever pool depth changes materially, and add a Timer-driven alarm if getReserves shows the pinned pair's reserve_in dropped below N x max_swap_value_e8s (the cap is now too large a fraction of depth). The submit-time gate is the real safety; the sizing cap is advisory and the spec should say so explicitly so an operator does not treat max_swap_value_e8s as a guaranteed-executable size.


**4. [HIGH] SP burn-then-eSpace-burn (Option A decouple): pending_chain_burn double-spend window if the eSpace Burn op never lands**
- Issue: Tier 2 (6.1, option A) burns IC-native icUSD synchronously, writes the amount into pending_chain_burn_e8s, decrements debt_e8s, and enqueues an eSpace Burn op; chain_supplies drops ONLY when that Burn confirms at finality. The invariant during the window: sum(chain_supplies) == debt + reserve + pending_chain_burn, which balances. BUT: the IC icUSD is irreversibly burned at step 1 (4 in 6.2), while the eSpace foreign icUSD is STILL CIRCULATING (totalSupply unchanged until the Burn op confirms). If the eSpace Burn op fails permanently (revert, custody can't cover gas, chain halted, reorg_halted), pending_chain_burn_e8s is stuck non-zero FOREVER and the eSpace icUSD is never burned — yet the standing rule forbids SP retries. The spec routes the Burn op through the existing settlement queue which CAN resubmit_if_stuck for non-swap ops (only swaps are excluded, per 8/10). So the Burn op is retryable at the settlement layer, which is fine (it is not an SP absorb retry). However, the spec never specifies what happens if the Burn op exhausts its retries / dead-letters: the eSpace supply stays inflated, pending_chain_burn stays booked, and now there is foreign icUSD circulating that is backed by NOTHING (the IC burn already destroyed the matching IC icUSD, the CFX went to SP depositors as claims). That is genuine under-backing on the eSpace side masked as 'pending' on the IC books indefinitely.
- Fix: Specify a terminal reconciliation for a permanently-failed eSpace Burn op: either (a) the Burn op MUST be infinitely retryable by the settlement worker (acceptable — it is idempotent and not an SP absorb retry) with a loud alarm and an operator getter surfacing aged pending_chain_burn entries, OR (b) if a Burn op dead-letters, it must escalate to a developer-gated manual burn endpoint, never silently sit. Add an invariant-monitor alarm: pending_chain_burn_e8s[chain] older than T (e.g. 1h) => alarm, because that is circulating-but-unburned foreign icUSD. Add a PocketIC test where the eSpace Burn op fails N times then succeeds, asserting pending_chain_burn -> chain_supplies decrement eventually completes and the invariant holds throughout. Without a terminal path, the 'decouple' choice trades the SP's EVM-finality coupling for an unbounded under-backing tail on eSpace.


**5. [HIGH] Upgrade safety / durable lock reconstruction (pending_liquidation marker vs prune_terminal)**
- Issue: The spec makes the persisted `pending_liquidation` marker the load-bearing durable lock and stores `op_id` on it, with Phase 2 confirm asserting "op matches". But the existing settlement worker calls `q.prune_terminal()` at the END OF EVERY `run_settlement` tick (settlement.rs:327-331), which REMOVES every `Succeeded`/`Failed` op from `pending` (settlement_queue.rs:194-219). So: (a) a LiquidationSwap op that the worker marks `Failed` (revert / min-out unmet / oracle-divergence) is GONE from `pending` by the next tick, yet the vault still carries `pending_liquidation{tier:Bot, op_id}`. The escalation-to-SP routing the spec describes ("mark bot_failed, escalate") has no Failed op to observe on the next tick — it must read the marker, not the op. If escalation logic instead looks for the Failed op (as the prose implies), the vault wedges locked forever with no op. (b) After an upgrade mid-swap, the marker references an op_id that may have been pruned, so any "op matches" assertion in `apply_liquidation_settlement_in_state` (§4.9 step 1) can spuriously fail and strand the reserved collateral. The interest-mint confirm path (confirm_interest_mint_in_state) survives this only because the op is still Inflight (not terminal) when confirm reads it.
- Fix: Make the `pending_liquidation` MARKER the single source of truth for routing and settlement, never the op's presence/status. Phase 2 must match on `pending_liquidation.op_id == op_id` AND tolerate the op already being pruned (treat a missing op as "resolve from marker"). Escalation (bot->SP) must be driven by the marker + an explicit terminal-outcome record (e.g. a `bot_failed_at_ns` field on the marker or a `sp_attempted_chain_vaults` insert at fail time), set inside the SAME mutate_state that marks the op Failed, BEFORE prune_terminal can reap it. Add a PocketIC test: bot swap reverts, tick completes (prune runs), assert the vault is still marked, the SP route fires exactly once, and an upgrade between revert and escalation preserves the marker + reserved collateral.


**6. [HIGH] Cross-canister double-settle: SP absorb vs bot swap confirm on the same vault**
- Issue: The two liquidation tiers run in DIFFERENT canisters with DIFFERENT, non-shared locks, and the spec's escalation timing opens a double-settle window. Tier 1 (bot) holds the backend heap `ChainVaultLiquidationGuard` only for the synchronous `begin_liquidation_in_state` call; the actual swap then lives as a `LiquidationSwap` op drained asynchronously by `run_settlement` on LATER ticks, which acquire only the per-CHAIN `SETTLEMENT_INFLIGHT` guard (settlement.rs:236-288), NOT any per-vault lock. Tier 2 (SP) is a separate canister whose `stability_pool_liquidate_chain_vault` -> backend `claim_chain_collateral` flow acquires the backend per-vault guard fresh. Because the heap guard does not span the async swap, the ONLY thing preventing the SP from absorbing a vault whose bot swap is still mined-but-not-final is the persisted `pending_liquidation` marker. The spec's §6.2 gate lists `MintInFlight`/`InterestRealizationPending` and a fresh guard, but does NOT explicitly list `pending_liquidation.is_some()` as an SP-side reject. If the SP entry doesn't hard-reject a Bot-tier marker, the SP can burn icUSD + seize CFX for a debt the bot swap then ALSO clears at confirm — double-settle, debt driven negative via saturating_sub but icUSD double-removed and CFX seized twice. The spec even notes the IC-timeout-must-exceed-deadline+finality constraint for exactly this reason but doesn't wire the marker check into §6.2.
- Fix: §6.2 MUST reject when `pending_liquidation.is_some()` regardless of tier (a Bot-tier marker means a swap is in flight or awaiting confirm; SP cannot touch it). Symmetrically, the trigger must never route a vault that already carries any-tier marker (already in §4.4 — keep it). State the invariant explicitly: at most ONE tier may hold `pending_liquidation` for a vault at a time, and the marker is the cross-tier mutex. Add a PocketIC test interleaving a mined-not-final bot swap with an SP absorb attempt on the same vault and assert the SP rejects, the bot confirm settles once, and `sum(chain_supplies)==debt+reserve+pending_burn` holds throughout.


**7. [HIGH] Unified-invariant consumer set is incomplete — clear_invariant_halt omitted**
- Issue: §5.4 enumerates "four consumers MUST change in lockstep" (apply_supply_delta, check_invariant, Timer-B operand, reconcile_chain_supply). It MISSES `clear_invariant_halt` (main.rs:1999-2011), which re-runs `check_invariant(&s.multi_chain, total_chain_vault_debt_e8s())` against the 1-TERM total and refuses to clear unless `sum(chain_supplies) == total_debt`. Once the bot path makes `sum(chain_supplies) == debt + reserve_backing + pending_chain_burn`, this developer-gated recovery endpoint can NEVER succeed after any bot liquidation (sum will exceed bare debt by the reserve term), permanently bricking the operator's only un-halt path exactly when they most need it. `check_invariant` is shared between Timer-B and this clear path, so changing its signature touches both, but the spec only accounts for the Timer-B caller.
- Fix: Add `clear_invariant_halt` to the lockstep consumer list. When `check_invariant`'s signature changes to take the 3-term RHS, update BOTH callers (Timer-B self-check AND clear_invariant_halt) to pass `debt + reserve + pending_burn`. Add a test: halt the invariant, perform a bot liquidation, then assert clear_invariant_halt succeeds against the unified RHS (and still refuses on a genuinely-diverged state).


**8. [HIGH] SP cross-chain claim custody invariant (`claim_cfx` deduct-before-async + revert re-credit, spec 6.4)**
- Issue: The claim path can lose or double-pay a depositor's CFX entitlement, and the spec's own mitigation is incomplete. The flow is: SP zeroes `cfx_claims[sentinel]` (deduct-before-async), calls backend `claim_chain_collateral`, which enqueues a `NativeWithdrawal` signed by the SEIZED VAULT's custody key. Two real holes: (1) The backend `NativeWithdrawal` is itself a two-phase async settlement op (submit -> confirm) that can REVERT minutes later in the settlement worker (verified: settlement.rs:876-937 CAS revert path). At that point the SP has ALREADY returned `Ok` to the depositor and zeroed the claim. The spec hand-waves this as 'a backend->SP callback or depositor re-claim' but specifies neither mechanism. Unlike `claim_collateral` (deposits.rs:236-360) where the ledger transfer resolves synchronously within the same call and rolls back inline, here the payout outcome is unknown when the SP call returns, so the SP cannot roll back. The depositor loses the entitlement on a revert with no recovery path defined. (2) The per-source-vault custody model means CFX for vault V lives ONLY in V's custody address. If two depositors both hold claims against vault V's seizure, and one claims the full custody balance first (the backend can't know the SP-side apportionment), the second depositor's `NativeWithdrawal` will fail for insufficient custody funds even though their `cfx_claims` entry says they are owed. Nothing in the spec reconciles the SP-side u128 apportionment sum against the actual per-vault custody balance at claim time.
- Fix: Make the backend the authoritative claim ledger, not the SP. Move the per-claim accounting (`ChainLiqClaimV1.paid_native <= seized_native_total`) to be checked inside the backend `claim_chain_collateral` BEFORE enqueueing the withdrawal, and have the backend hold a synchronous reservation that is only released on confirm/revert under the same CAS as `NativeWithdrawal` (settlement.rs:879). On revert, the backend MUST emit a typed event the SP consumes to re-credit `cfx_claims` (define this callback explicitly; do not leave it as 'or depositor re-claim'). Add the mandatory invariant test the spec names but does not bind to code: `sum_depositors(cfx_claims) == sum_vaults(seized_native_total - paid_native)` AND each per-vault `paid_native <= on-chain custody balanceOf(custody_addr)`. Without the second clause the SP can authorize more CFX than any single custody address holds.


**9. [HIGH] SP position garbage-collection destroys unclaimed CFX entitlements (new `cfx_claims` map vs `is_empty()`)**
- Issue: Verified concrete bug the spec misses. Section 6.3 mandates a PARALLEL `DepositPosition.cfx_claims: BTreeMap<Principal, u128>` field to avoid the u64 overflow. But `process_liquidation_gains_at` Phase 6 runs `self.deposits.retain(|_, pos| !pos.is_empty())` (state.rs:818), and `DepositPosition::is_empty()` (types.rs:130-133) only checks `stablecoin_balances` and `collateral_gains` (both u64). A depositor whose stablecoins were fully consumed in a CFX liquidation but who has not yet claimed their CFX has `stablecoin_balances` all zero and `collateral_gains` empty (CFX went to the NEW map), so `is_empty()` returns true and the position is RETAINED-OUT (deleted) on the very next liquidation pass. The depositor's u128 CFX claim is permanently destroyed. This is the exact mirror of the SP-001 class of double-deduction bugs the code already fences against, reintroduced by adding a value-bearing map that the GC predicate does not know about.
- Fix: Add `&& self.cfx_claims.values().all(|&v| v == 0)` to `is_empty()` (types.rs:130) in the SAME PR that introduces the `cfx_claims` field, and add a regression test: open position, fully consume its stablecoins via a CFX liquidation, run a second liquidation, assert the position still exists and the CFX claim is intact. Also audit every other `pos.is_empty()` / `deposits.retain` call site for the same gap. List this in spec section 6.3 as a hard requirement, not an afterthought.


**10. [HIGH] Bot->SP escalation timing model is undefined for the canister-as-bot (spec 4.5, 6.5)**
- Issue: The locked decision is bot-PRIMARY, SP-FALLBACK 'when the bot can't clear'. On the ICP rail this escalation is a concrete state machine (verified lib.rs:1075-1131): `bot_pending_vaults[vid] = now` on first route, and only after `now - ts >= bot_timeout_ns` (5 min, lib.rs:1006) does the vault fall to the pool. The chains spec describes routing ('bot first; on bot-fail/timeout -> SP (once)') but never defines the bot-timeout analog. This matters MORE for chains because the 'bot' is the backend canister's own settlement queue, where a `LiquidationSwap` op can sit Inflight across many ticks (deadline 180s + finality + the IC-confirm-timeout the spec itself says must be > deadline+finality). There is no `bot_pending_chain_vaults` timestamp map in the V6 state list (section 3.2), so the trigger has no durable signal for 'the bot has had its shot, escalate now.' Without it, either (a) the trigger never escalates because the swap op is perpetually 'in flight,' stranding underwater vaults the bot can't clear (e.g. min-out permanently unmet on a thin pool) at Tier 1 forever, or (b) the trigger escalates to SP while the swap op is still live, and BOTH the SP writedown AND a late-confirming swap reduce the same debt = double-counted debt reduction / the double-spend the spec warns about in section 4.8.
- Fix: Add a durable `bot_pending_chain_vaults: BTreeMap<u64, u64>` (vault_id -> first-routed-ns) to MultiChainStateV6 and port `prune_recovered_routing_state` semantics. Define the escalation predicate explicitly: a vault escalates to SP only when its `LiquidationSwap` op is in a TERMINAL failed state (min-out unmet, oracle-divergence, revert, or confirm-timeout) OR `now - bot_pending_since >= chain_bot_timeout_ns`, AND the `pending_liquidation` marker is cleared. Critically, the SP entry (`stability_pool_liquidate_chain_vault`) MUST reject if `pending_liquidation.is_some()` with `tier == Bot` (an in-flight bot swap), so the SP can never absorb a vault whose swap might still settle. The spec's section 4.4 excludes `pending_liquidation.is_some()` from the TRIGGER but does not state the SP entry enforces the same exclusion against a live BOT marker.


**11. [HIGH] Over-liquidation of collateral when a burn shrinks debt during the swap window (spec 4.9)**
- Issue: Chains debt reduction is observer-driven and UNGUARDED relative to the liquidation marker. Verified: `apply_burn_to_state` (deposit_watch.rs:170-219) decrements `debt_e8s` at burn finality and does NOT check `pending_liquidation`; the burn-watch observer holds only its own per-chain re-entrancy guard, not the per-vault liquidation guard. The bot's two-phase accounting reserves collateral in Phase 1 sized to `debt_to_clear_e8s`, then sells it in the async swap. The spec's Phase 2 correctly caps the DEBT reduction to `min(debt_to_clear, live_debt)` via saturating_sub. But the COLLATERAL was already sold at the full pre-burn size. If a user repays (burns) most of their debt on eSpace mid-swap, the vault is left with the debt correctly reduced but with FAR more collateral seized than the residual debt justified, and the excess USDC silently accrues to `reserve_usdc_native` as 'penalty surplus' (section 5.6). The owner is over-liquidated and the protocol pockets the difference. This is unjust seizure, the exact failure class the staleness gate is meant to prevent, arriving through a different door.
- Fix: Either (a) block the burn observer from decrementing a vault while `pending_liquidation.is_some()` (defer the burn application until the marker clears, since `apply_burn_to_state` already treats over-debt burns as skippable/retryable InvalidBurn), or (b) in Phase 2, when `live_debt < debt_to_clear`, refund the over-seized collateral: return `collateral_reserved_native` proportional to the unrealized debt portion back to `collateral_amount_native` rather than booking all USDC as surplus. Option (a) is cleaner and matches the locked 'vault LOCKS when collateral handed to bot' intent: the lock should also freeze the burn-driven debt path for that vault, not just owner writes. Add a PocketIC test: open vault, trigger bot swap, apply a burn that drops debt below `debt_to_clear` before confirm, assert the vault is not over-liquidated.


**12. [HIGH] Settlement reuse / op lifecycle (the IC confirm-timeout the spec depends on does not exist)**
- Issue: §4.8 and §10 repeatedly rely on an "IC confirm-timeout" that marks a stuck swap op `Failed` and escalates to SP: "the IC confirm-timeout then marks it Failed → escalate", "IC confirm-timeout MUST be set > deadline + finality". No such timeout exists in the settlement worker. In `evm/settlement.rs` `confirm_op`, the not-mined branch only advances `tries` and, once `is_stuck` (hardening.rs:9, `tries >= finality_depth*2`), calls `resubmit_if_stuck` — a replace-by-fee on the SAME nonce, FOREVER. There is no code path that transitions an Inflight op to `Failed` on age/timeout (only an on-chain REVERT, via `status_ok==false`, marks Failed). A swap that hits its on-chain `deadline` and reverts WOULD mark Failed — but only if the SAME nonce's tx actually gets mined-and-reverted; a never-mined tx (e.g. underpriced, or replaced) is resubmitted indefinitely and never escalates. So the bot→SP escalation on timeout is built on a mechanism the codebase lacks.
- Fix: The spec must add a NEW op-level timeout-to-Failed path (e.g. an `Inflight { tries, last_attempt_ns, enqueued_at_ns }` age check that marks LiquidationSwap Failed after `> deadline + finality + margin`), and must EXCLUDE swap ops from `resubmit_if_stuck` AS WELL (the spec says this, but without the timeout the exclusion just freezes the op forever). State this as net-new worker logic, not a reuse of an existing pattern. Add a PocketIC test for never-mined swap → Failed → SP.


**13. [HIGH] EVM-RPC quorum vs volatile getReserves (just-in-time min-out is unreachable on mainnet)**
- Issue: §4.8 says compute `amountOutMin` from a LIVE `getReserves`/`getAmountsOut` read "in the SAME tick that signs — never cached" and to "reuse the eth_call quorum template (erc20_total_supply_at...)". But `call_evm_rpc` (evm_rpc.rs:634) requires `max(floor, strict_majority)` DISTINCT providers to return a BYTE-IDENTICAL `result` field (`response_consensus_key` clones the raw `result`, evm_rpc.rs:569). `erc20_total_supply_at` only gets consensus because it pins a FINALIZED block (`"0x{:x}"` block arg). A `getReserves` at "latest" moves every block and every swap, so independent providers at different heights return DIFFERENT reserves, fail the majority, and return Err. Conflux MAINNET mandates ≥3 INDEPENDENT providers (conflux/config.rs RELAXED to 1 only for the single-operator Confura testnet; the mod note says mainnet must raise the floor). So the just-in-time reserves read will fail-closed on mainnet exactly when the bot tier is supposed to work, permanently routing everything to SP.
- Fix: Either (a) pin the reserves read to a recent FINALIZED block (matching erc20_total_supply_at) and accept that min-out is computed against slightly-stale-but-consensus reserves (then widen the slippage/divergence margins to cover drift between the finalized read and the swap landing), or (b) special-case swap-quote reads to a non-quorum single-provider path with the oracle cross-check as the integrity backstop. Option (a) is more consistent with the codebase's reorg-safety discipline. Either way the spec's "reuse the quorum template verbatim, read live at latest" is not viable — call it out and pick a model.


**14. [HIGH] Single-queue head-of-line blocking (a stuck LiquidationSwap blocks all user ops on the chain)**
- Issue: There is exactly ONE `SettlementQueueV1` per chain, and `select_next_op` (settlement.rs:66) enforces strict one-op-in-flight: if ANY op is Inflight, ONLY that op is actionable; otherwise the lowest op_id Queued. Combined with the missing confirm-timeout (finding 1), a `LiquidationSwap` that goes Inflight and never resolves (RPC flakiness, perpetual resubmit, deadline-revert-then-stuck) will block EVERY subsequent op on that chain — user borrow mints, withdrawals, closes, interest mints, AND other liquidations — indefinitely. The spec's §4.7 "one-op-in-flight queue" and "partial-of-partial loop" treat serialization as benign, but it means a single misbehaving swap bricks the whole rail's settlement for that chain (the same class of incident the rail already guards against for mints, but now with a swap that has far more failure modes: thin DEX, moved price, deadline expiry).
- Fix: Make the timeout-to-Failed (finding 1) mandatory and tight so a swap cannot hold the queue head. Consider whether liquidation swaps should ride a SEPARATE per-chain queue (or a priority lane) so they cannot starve user collateral returns. At minimum, the spec should explicitly analyze head-of-line blocking and bound the worst-case time a stuck swap can hold the queue.


**15. [HIGH] MultiChainState V6 bump scope (the "one snapshot bump" understates a wide mechanical change)**
- Issue: §3 frames V6 as "rebind `pub type MultiChainState = MultiChainStateV6`" plus additive fields. But the concrete type `MultiChainStateV5` is hard-referenced by name in dozens of function signatures across the codebase, NOT via the `MultiChainState` alias: `vault.rs` (open/borrow/withdraw/close `state: &mut MultiChainStateV5` at lines 229/325/428/572/662/750), `interest.rs:55`, `recovery.rs:92/217/280`, `supply.rs` (`apply_supply_delta`/`check_invariant`/`stamp_chain_interest_accrual_start` all take `&mut MultiChainStateV5`), `mod.rs:52/55`. Every one must change to V6 for the bump to compile. This is a large blast-radius rename, not a localized snapshot add, and the spec's phasing (Increment 1) should budget for it explicitly. It also means `apply_supply_delta`'s signature change (§5.3) and the V6 type change land in the same compile unit and must be staged together.
- Fix: Rewrite §3/Increment-1 to enumerate the ~15 concrete `MultiChainStateV5` signature sites that must move to V6, OR refactor those signatures to use the `MultiChainState` alias FIRST (a separate no-op PR) so the V6 bump is genuinely one-line. The alias-refactor-first approach de-risks the bump and should be the recommended path.


**16. [MED] Partial-fill / realized-out shortfall path is declared 'unreachable' but the code still must handle it, and the gross-up rounding can under-cover**
- Issue: 4.9 step 1 validates usdc_out_native valued in USD >= debt_to_clear_e8s 'else 4.10 shortfall — but strict min-out should make this impossible.' Relying on 'should make this impossible' is exactly the kind of assumption that produces bad debt. The realized usdc_out is read from the on-chain Transfer log (4.8 confirm) and can be LESS than min-out is intended to guarantee only if min-out was computed correctly; but min-out is computed from getReserves at submit, and the swap executes against reserves that may have moved between the eth_call read and the tx mining (sandwich, or just another swap in the same block). UniV2 guarantees out >= amountOutMin on-chain or it reverts, so a SUCCESSFUL receipt does guarantee realized >= min-out. The real gap: min-out itself is debt_to_clear's collateral grossed up by 12% then haircut by slippage_bps. If slippage_bps haircut (250 bps) exceeds the 12% penalty cushion on a given vault... it cannot here (1200 bps > 250 bps), so realized USDC >= debt. Fine. BUT the gross-up (4.6) collateral_in_native = repay * bonus / 10000 * scale / price, then min-out = expected_out * (1 - slippage). The chain is: seize collateral worth repay*1.12 at ORACLE price, sell it, accept down to (1-slippage)*pool-expected. If pool price < oracle by up to max_dex_oracle_divergence_bps (500), the realized USDC can be below repay even with the 12% gross-up: 1.12 * (1 - 0.05 pool-vs-oracle) * (1 - 0.025 slippage) = 1.12 * 0.95 * 0.975 = 1.037, still > 1.0. Margin is only 3.7%. Tighten any of those knobs (penalty drops, or divergence+slippage allowed to widen) and realized USDC < debt_to_clear => the 4.9 step-1 validation FAILS, and the spec does not say what happens then beyond '4.10 shortfall'.
- Fix: Fully specify 4.10: when realized usdc_out valued < debt_to_clear_e8s, do NOT credit reserve_backing for more than the realized-USD-value (clamp actual_cleared = min(debt_to_clear, usdc_out_valued_e8s, live_debt)), leave the residual debt on the vault (it stays Open, under-restored), and record the gap as a per-chain bad-debt counter (9 already wants per-chain counters). NEVER let reserve_backing_e8s exceed the realized USD value or the invariant claims backing that does not exist. Add a property test that asserts penalty_bps - (slippage_bps + dex_oracle_divergence_bps) > 0 as a CONFIG INVARIANT enforced in set_chain_liquidation_config (reject configs where the slippage+divergence budget exceeds the penalty cushion), so a misconfig cannot create structural under-cover.


**17. [MED] reconcile_chain_supply and the deposit_watch backstop compare against chain_supplies only and do NOT need reserve added — the spec's claim here is partly wrong and could drive a WRONG fix**
- Issue: The spec (5.4 item 4) asserts reconcile_chain_supply and the observer totalSupply backstop must add reserve_backing as backing 'or a bot liquidation looks like an unbacked mint / triggers a catch-up burn sweep.' I verified the actual code: reconcile_chain_supply (main.rs:2306-2360) compares on-chain totalSupply against recorded=chain_supplies[chain] (+ in_flight mints), and the backstop (deposit_watch.rs:659-689) compares on-chain totalSupply against recorded_supply=chain_supplies. On the BOT path, chain_supplies is UNCHANGED (no burn) and on-chain totalSupply is ALSO unchanged (no eSpace burn) — so onchain == recorded still holds and NEITHER reconcile nor the backstop fires. Adding reserve_backing to the 'recorded' side of these comparisons would be WRONG: it would make recorded > onchain artificially and could MASK a real unbacked-mint divergence (the very thing the backstop exists to catch). The place reserve actually matters is the internal Timer-B check_invariant (debt-vs-supply), NOT the on-chain-supply reconciliation. The spec conflates two different invariants: (debt+reserve+pending == supply) is internal bookkeeping; (chain_supplies == onchain totalSupply +/- in_flight) is the on-chain truth check. Reserve belongs ONLY in the first.
- Fix: Do NOT add reserve_backing_e8s to the recorded side of reconcile_chain_supply's or the backstop's on-chain-totalSupply comparison. Leave those comparing chain_supplies (IC's belief about circulating foreign icUSD) against on-chain totalSupply — that relationship is unaffected by the reserve shift and adding reserve would blind the unbacked-mint alarm. DO surface reserve_backing_e8s, reserve_usdc_native, pending_chain_burn_e8s as ADDITIONAL informational fields on ChainSupplyReconciliation for operator visibility (the spec wants this and it is correct), but they must NOT enter the unbacked_excess boolean. Add a test: an unbacked eSpace mint (onchain > recorded) while reserve_backing is non-zero STILL trips unbacked_excess. The spec's 5.4-item-4 wording should be corrected before implementation or it will produce a masking bug.


**18. [MED] SP coverage gate uses effective_pool_for_collateral(cfx_sentinel) but a brand-new sentinel collateral has ZERO opted-in depositors, so coverage is always 0 and Tier 2 silently never fires**
- Issue: 6.3 registers CFX as a sentinel CollateralInfo and reuses effective_pool_for_collateral(cfx_sentinel) >= debt as the coverage gate. But effective_pool_for_collateral sums the stable balances of depositors who have OPTED IN to that specific collateral (process_liquidation_gains_at filters pos.is_opted_in(&collateral_type), state.rs:688-691). For a freshly-registered CFX sentinel, NO existing SP depositor has opted in (the collateral did not exist when they deposited). So effective_pool_for_collateral(cfx_sentinel) == 0 on day one, the coverage gate fails for every chain vault, Tier 2 NEVER absorbs, and everything falls to Tier 3 manual — silently defeating the entire SP fallback. This is a real hole: the spec treats the sentinel as drop-in but the opt-in semantics mean the SP is empty for CFX until depositors explicitly opt in, and there is no UX/endpoint mentioned to let them.
- Fix: Specify the opt-in story for the CFX sentinel: either (a) make chain-vault SP absorption opt-OUT (default all icUSD-holding depositors opted into CFX) like the ICP base pool, or (b) add an explicit opt-in endpoint + frontend and document that Tier 2 is INERT until depositors opt in, so operators do not believe they have an SP fallback they do not have. Add a test asserting effective_pool_for_collateral(cfx_sentinel) reflects opted-in depositors and that with zero opt-ins the router cleanly escalates to manual (not a panic, not a false 'absorbed'). This also interacts with 6.6's 'restrict to icUSD draws in v1' open decision — clarify that even icUSD-funded coverage requires opt-in to the CFX collateral key.


**19. [MED] actual_cleared = min(debt_to_clear, live_debt) handles debt SHRINKING but the collateral was already over-reserved in Phase 1 against the larger debt — reserved collateral can exceed what the now-smaller debt justifies**
- Issue: 4.9 Phase 2 step 2 re-reads live debt and clamps actual_cleared = min(pending.debt_to_clear_e8s, live_debt_e8s) for the case where a concurrent permissionless burn shrank debt between sizing and confirm. Debt accounting is saturating-safe. BUT the COLLATERAL was reserved in Phase 1 (collateral_reserved_native, sized against the LARGER debt_to_clear), already swapped to USDC, and that USDC is now reserve. If live_debt shrank, actual_cleared < debt_to_clear, so reserve_backing_e8s += actual_cleared (the smaller amount) — correct. But reserve_usdc_native += usdc_out_native is the FULL realized USDC from selling collateral sized to the LARGER debt. So reserve_usdc_native now overshoots reserve_backing_e8s by more than the 12% penalty: the gap = penalty + (collateral sold for the debt that got burned away). That excess USDC corresponds to collateral seized from a vault for debt that was ALREADY repaid by the concurrent burn. The vault owner was over-liquidated: their collateral was sold to back debt that no longer existed, and the proceeds are bucketed as protocol 'surplus' (5.6). This is an over-seizure / unjust-loss to the owner, masked as revenue.
- Fix: This is the partial-of-partial / TOCTOU over-seizure case. Since the swap already executed (collateral already sold, irreversible), the only honest remedy is to credit the over-seized USDC value back to the VAULT, not to protocol surplus: when actual_cleared < debt_to_clear due to a concurrent burn, compute the over-seized portion (debt_to_clear - actual_cleared) and either (a) credit its USD-equivalent USDC into a per-vault claimable reserve the owner can withdraw, or (b) at minimum book it to a clearly-labeled over_seizure_refund_owed bucket, NEVER to penalty surplus. Better: hold the per-vault guard from sizing THROUGH confirm (the spec says the guard is held across the async swap, 4.9 step 8) — verify that the guard actually blocks the concurrent permissionless burn so debt CANNOT shrink mid-swap. If the guard already serializes liquidation-vs-burn for the same vault, this case is unreachable and should be ASSERTED (debt_to_clear == actual_cleared always); if it does not (burns are a different code path not covered by ChainVaultLiquidationGuard), the over-seizure is live and needs the refund path. Confirm which, and add the test.


**20. [MED] Reserve term not counted by reconcile_chain_supply / unbacked-excess backstop**
- Issue: `reconcile_chain_supply` (main.rs:2298-2361) computes `unbacked_excess = onchain > recorded + in_flight`, where `in_flight` counts only Queued/Inflight Mint amounts. The bot path leaves `chain_supplies` (=`recorded`) UNCHANGED while reducing vault debt, so on-chain icUSD totalSupply stays the same and `recorded` stays the same — reconcile is fine for the bot path on the supply side. BUT the observer's totalSupply backstop (deposit_watch divergence alarm, referenced in §5.4 item 4) scans on a supply DROP to catch an unbacked burn; the SP path's `pending_chain_burn` mechanism burns IC-side FIRST and only later drops eSpace totalSupply at the eSpace-Burn confirm. Between the IC burn and the eSpace burn, IC-side accounting shows debt moved to pending_chain_burn but on-chain totalSupply has NOT yet dropped, so `recorded`(=chain_supplies, unchanged until eSpace burn confirms) still equals on-chain — consistent. The real gap: after the eSpace Burn confirms and `chain_supplies` is decremented, if the operator runs reconcile, `recorded` dropped but the spec's new `pending_chain_burn`/`reserve_backing` breakdown is only ADDED to the ChainSupplyReconciliation struct for display, not folded into the `unbacked_excess` predicate. The predicate itself is unchanged and stays correct ONLY if reserve/pending terms never affect on-chain totalSupply — which holds, but this must be asserted, not assumed.
- Fix: Explicitly document and test that `reserve_backing_e8s` and `pending_chain_burn_e8s` do NOT enter the `unbacked_excess` arithmetic (they are backing reclassification, not on-chain supply), and that the only on-chain-supply effects are: bot path = none, SP path = a single decrement at eSpace-Burn confirm. Add the three new fields to ChainSupplyReconciliation for operator visibility (spec already says this). Add a test that runs reconcile after each of {bot confirm, SP IC-burn, SP eSpace-Burn confirm} and asserts `unbacked_excess == false` at every step.


**21. [MED] SP deduct-before-async claim revert re-credit is a cross-canister callback with no durable record**
- Issue: §6.4 `claim_cfx` zeroes `cfx_claims[sentinel]` before calling backend `claim_chain_collateral`, then enqueues a `NativeWithdrawal` signed by the seized vault's custody key. The spec acknowledges that if the EVM settlement later REVERTS, the backend revert path must "signal the SP to re-credit cfx_claims (a backend->SP callback or depositor re-claim)." This is the single weakest async seam: the existing NativeWithdrawal revert handler (settlement.rs:897-905) only adds collateral back to `collateral_amount_native` on the BACKEND vault — but a claim_cfx withdrawal is paid from a vault that may already be `Closed`, and there is no vault-side collateral field to restore for an SP claim, and crucially no mechanism today for the backend confirm/revert path to call back into the SP canister. A backend->SP callback that itself can fail/trap reintroduces exactly the kind of cross-canister state-divergence the SP-102/AR-S work fought. The depositor's entitlement is silently lost on revert if the callback fails.
- Fix: Do NOT rely on a backend->SP push callback for re-credit. Make the SP claim PULL-verified: the backend records the claim payout op outcome durably (a per-claim `ChainLiqClaimV1.paid_native` increment ONLY on confirm, and a `claim_failed` marker on revert), and the SP's `claim_cfx` becomes idempotent + re-queryable — on a failed/timed-out claim the depositor re-calls claim_cfx, which queries the backend claim record and only re-credits/re-enqueues if the backend shows the prior op terminal-failed and unpaid. Never zero `cfx_claims` until the backend reports the payout op `Succeeded`. Write the failing test (claim enqueues, EVM reverts, depositor re-claims, entitlement preserved exactly once).


**22. [MED] Bot swap excluded from resubmit_if_stuck but IC-confirm-timeout marking is unspecified in the existing worker**
- Issue: §4.8 mandates LiquidationSwap ops be EXCLUDED from `resubmit_if_stuck` (correct — replace-by-fee into a moved price violates the no-retry-into-worse-slippage rule). But the existing confirm path has NO concept of a terminal IC-side confirm-timeout: a never-mined op stays `Inflight` forever, with `confirm_op` advancing `tries` via `on_not_mined_tick` and (for other op kinds) eventually resubmitting. If LiquidationSwap is simply excluded from resubmit, an op whose on-chain `deadline` passed and reverted will be caught by the revert branch (status_ok=false) — fine. But an op that was DROPPED by the mempool (never mined, no receipt ever) will sit Inflight indefinitely, holding the vault's `pending_liquidation` marker and its reserved collateral hostage, with NO path to Failed. The spec asserts "IC confirm-timeout MUST be set > deadline + finality" but no such timeout mechanism exists in confirm_op today — it must be BUILT, and only for the swap kind, or the vault locks permanently.
- Fix: Build an explicit per-op confirm-timeout for the LiquidationSwap kind: when `enqueued_at_ns`/Inflight `last_attempt_ns` exceeds `deadline_secs + finality_margin` and no receipt has appeared, mark the op `Failed` (NOT resubmit), run the CAS collateral-restore, clear the marker, and escalate to SP. Add this as a distinct branch since no other op kind needs it (mints/withdrawals safely replace-by-fee). Test: enqueue a swap, simulate a never-mined tx past the timeout, assert op->Failed, collateral restored under CAS, marker cleared, SP routed once.


**23. [MED] Conflux collateral config only resolves for chain 71 (testnet); all liquidity/DEX facts are chain 1030 (mainnet)**
- Issue: `chain_collateral_config` (collateral_config.rs:42-54) returns `Some(ICP_MIRROR)` ONLY for chain id 71 and Monad 10143; chain 1030 (Conflux mainnet, where ALL the spec's DEX liquidity, WCFX/USDC pair, and the ONLY testable swap path live) returns `None`. The CR/interest/penalty/recovery reads in begin_liquidation, the partial-math, and the SP absorb all call `chain_collateral_config(chain)` and silently fall back (interest apr_bps 0, etc.) or fail for 1030. The spec's §8 onboarding example even uses `set_chain_liquidation_config(71, ...)`. So as written the engine is wired to testnet-71 (no DEX liquidity) while the swap leg can only run against mainnet-1030 (no collateral config). The price-staleness gate (§4.3) likewise hardcodes `(71, "CFX")`. This is an internal inconsistency that will surface as a runtime `None`/NoPrice on the only environment where the swap can actually execute.
- Fix: Decide the target chain id explicitly and make it consistent end to end. If mainnet-1030 is the real deployment, add a `1030 => Some(ICP_MIRROR)` row to `chain_collateral_config` (and migrate the manual-price key + liquidation-config + EvmChainConfig to 1030) BEFORE wiring the swap; if 71 stays the dev rail, the spec must stop citing 1030 liquidity as executable and gate the swap behind a mock router only. Either way, the chain id used by collateral_config, manual_prices, liquidation_config, and the DEX pair addresses must all agree. Flag for Rob alongside the open-gate sign-off.


**24. [MED] apply_supply_delta signature change is a wide blast radius touching every existing mint/burn caller**
- Issue: §5.3 changes `apply_supply_delta` to take explicit RHS components `(debt_total, reserve_total, pending_burn_total)` instead of a single `total_debt_e8s`. Every existing caller passes a single pre/post `total_chain_vault_debt_e8s()`: `confirm_mint_in_state` (settlement.rs:123), `confirm_interest_mint_in_state` (settlement.rs:173), and `apply_burn_to_state` (deposit_watch). Each computes its post-total as `pre +/- amount` and passes THAT. If the signature gains reserve/pending terms, EVERY one of these callers must now also fetch and pass the (unchanged) reserve + pending_burn sums, or the invariant comparison `sum_after == debt+reserve+pending` will be wrong for a plain mint (which leaves reserve/pending untouched but must still include them on the RHS). A missed caller silently FALSE-HALTS the chain on the next mint after any reserve exists. This is mechanical but high-blast-radius and easy to get wrong under the "one PR" constraint.
- Fix: Have `apply_supply_delta` (and `check_invariant`) compute the reserve + pending_burn sums INTERNALLY from `state` (they already take `&mut state` / `&state`), so callers keep passing only the debt-delta component they own. This makes the change additive at every existing call site (confirm_mint/confirm_interest/apply_burn pass debt as before; the function adds the other two terms itself), eliminating the forgot-a-term FALSE-HALT class entirely. Only the bot's `apply_debt_to_reserve_shift` needs the new term knowledge. Property test: a plain mint, a burn, and a bot shift each leave the unified invariant exact with no caller passing reserve/pending explicitly.


**25. [MED] Un-refundable SP over-burn is real on the chains path AND already on the ICP path (spec 6.2 step 4)**
- Issue: The spec correctly flags that over-burn is un-refundable bad debt and recommends 'the SP re-quotes inside its guard and burns EXACTLY the capped amount.' Verified the hazard is real: the burned-debt path (`liquidate_vault_debt_already_burned`, vault.rs:3859 and the re-cap at 3934-3940) caps the writedown to live debt via `liquidation_amount.min(vault.borrowed_icusd_amount)`, but the icUSD was ALREADY burned by the SP before the call, so any excess is genuinely gone. However, the spec's mitigation is not fully achievable: the SP must burn BEFORE it can supply the burn-proof block index to the backend, and it cannot know the backend's live debt at burn time without a round-trip that itself races the burn (and races a concurrent burn-watch decrement, see the over-liquidation finding). So 're-quote inside its guard' reduces but cannot eliminate the over-burn, because live debt on the backend can drop AFTER the SP's quote and BEFORE the backend caps. The SP's own `SpLiquidationGuard` does not span the backend's state.
- Fix: Quantify and bound the residual: the SP should burn the amount it quoted, and the backend should return the actual-applied amount; the DIFFERENCE (quoted - applied) is irrecoverable SP-depositor loss. Make the backend return this delta explicitly and have the SP record it as a realized-loss metric (not silently swallow it). Bound the blast radius by keeping the chain debt ceiling tiny (section 9 already does this). Also enforce the no-retry rule here precisely: a partial-burn over-shoot is NOT a reason to re-attempt; the vault is marked `sp_attempted_chain_vaults` and falls to manual regardless. The spec's section 6.5 phrasing 'ONE shot... on any failure the SP does NOT retry' must explicitly include 'a successful-but-capped (over-burn) absorb is still a completed shot, never retried.'


**26. [MED] `sp_attempted_chain_vaults` set/clear semantics and persistence (spec 3.2, 6.5)**
- Issue: On the ICP rail the SP-attempted marker is set when the NOTIFICATION DELIVERS (`record_sp_notification_result` Ok arm, lib.rs:752-758), NOT when absorb succeeds, precisely so a legitimate SP decline (insufficient coverage) still consumes the single shot and the vault falls to manual instead of looping. The chains spec adds `sp_attempted_chain_vaults` but section 4.5/6.5 describe it being set on 'failure' and via the backend marking it; it never states the marker is set on a SUCCESSFUL-but-coverage-declined attempt. If the chains SP entry returns an InsufficientPoolBalance-style error and the marker is only set on hard failure, the trigger will re-route the same vault to the SP every tick (a retry loop), violating the no-retries rule. Separately, the spec says this set 'rides V6 because it is in the persisted root but is reset/pruned on resolution; surviving an upgrade is harmless.' That is only true if a prune analog to `prune_recovered_routing_state` exists for chains; section 3.2 asserts pruning but no such function is named in the file map.
- Fix: Mirror the ICP semantics exactly: set `sp_attempted_chain_vaults` whenever the SP attempt is DISPATCHED/completed for any reason other than a transport Err (so coverage-declines consume the shot; transport Errs do not, matching `record_sp_notification_result`). Define and name the chains `prune_recovered_routing_state` analog that clears `sp_attempted_chain_vaults` (and any `bot_pending_chain_vaults`) once a vault recovers above its liquidation floor, and call it unconditionally every trigger tick, including quiet ticks (the 2026-05-17 incident referenced in lib.rs:777-786 is exactly the bug of pruning only on the non-empty branch). Add a test: SP declines for coverage, assert the vault is marked attempted and routed to manual, not re-routed to SP next tick.


**27. [MED] Coverage gate uses `effective_pool_for_collateral(cfx_sentinel)` but the CFX sentinel has no opted-in depositors at first liquidation (spec 6.3)**
- Issue: `effective_pool_for_collateral` (state.rs:524-530) sums the USD value of opted-in depositors' stablecoin balances, filtered by `is_opted_in(collateral_type)`. `is_opted_in` returns true unless the depositor is in `opted_out_collateral` (types.rs:125-126), i.e. opt-OUT, not opt-IN, so by default every depositor counts toward every collateral including a freshly-registered CFX sentinel. That is actually fine for coverage. The real gap: the spec relies on registering CFX as a `CollateralInfo` with a deterministic per-chain sentinel principal, but does NOT specify that this sentinel must be created/registered BEFORE the first liquidation and must never collide with the ICP collateral principals already in `opted_out_collateral` sets of existing depositors. If an existing depositor happens to have opted OUT of a real collateral whose principal collides with the derived sentinel, they would be silently excluded from CFX coverage (or worse, mis-counted). The 'deterministic-from-chain_id, never collide with a real ledger principal' requirement is stated but not given a construction.
- Fix: Specify the sentinel derivation concretely (e.g. a reserved principal namespace clearly outside the ICRC ledger principal space, derived as hash(b"chain-collateral-sentinel" || chain_id_le)) and add a startup/registration assertion that the derived sentinel does not equal any registered stablecoin or collateral ledger principal. Add a test that registers the CFX sentinel and asserts `effective_pool_for_collateral(sentinel)` equals the opted-in stable pool for a fresh pool with no CFX history. Confirm `is_opted_in` (opt-out semantics) gives the intended default-in behavior for the new sentinel, and document it, since a reader expecting opt-IN semantics would mis-reason about coverage.


**28. [MED] SP busy-guard (SP-102) vs claim_cfx vs an in-flight chain burn op (spec 6.4 step 1, 6.5)**
- Issue: The spec gates `claim_cfx` behind the SP-102 busy guard (`liquidation_in_progress()`), matching `claim_collateral` (deposits.rs:238). Good. But there is a longer-lived busy state unique to the chains path that SP-102 does not cover: after `stability_pool_liquidate_chain_vault` returns, the eSpace `Burn` op is still in flight (decoupled, section 6.1 option A) and `pending_chain_burn_e8s` is nonzero until that burn confirms at finality (could be minutes). During that window the SP's `SpLiquidationGuard` is already released (the absorb call returned), so deposits/withdraws/claims are unblocked. A depositor who withdraws their stablecoins in that window has already been correctly debited (the burn happened IC-side synchronously), so that is fine. The actual risk is on the BACKEND side: the eSpace Burn op for the SP path and a `LiquidationSwap` op for a bot path on the SAME vault could both be enqueued if the escalation timing (HIGH finding above) lets the SP absorb a vault with a live bot marker. SP-102 protects the SP pool's internal consistency but does nothing to serialize the backend's two op kinds against one vault.
- Fix: The serialization point for one-vault-one-tier must be the backend's `pending_liquidation` marker plus the `ChainVaultLiquidationGuard`, not SP-102. Enforce in `stability_pool_liquidate_chain_vault`: acquire the chains per-vault guard FIRST, and reject if `pending_liquidation.is_some()` (any tier). This guarantees a vault can have at most one absorb path (bot swap OR SP burn) live at a time, regardless of SP-102. Make the spec state this explicitly in section 6.2 step 3 (it currently says 'acquire the chains per-vault guard' but not 'reject on any existing pending_liquidation marker').


**29. [MED] reconcile_chain_supply semantics (adding reserve_backing to the RHS can mask a real unbacked mint)**
- Issue: §5.4 item 4 says reconcile must "treat reserve_backing as legitimate backing" by adding it to the comparison. But `reconcile_chain_supply` (main.rs:2347-2348) computes `unbacked_excess = onchain_totalSupply > recorded(chain_supplies) + in_flight_mint`. On the BOT path, neither `chain_supplies` nor on-chain `totalSupply` changes (no icUSD burned), so reconcile is ALREADY correct and does NOT false-alarm — the spec's premise that the bot path breaks reconcile is wrong for THIS getter (it breaks the Timer-B `check_invariant`, which is a different consumer). Worse, naively adding `reserve_backing` to the RHS of the `unbacked_excess` check would LOOSEN the unbacked-mint detector: a genuinely unbacked on-chain mint could be hidden by a large reserve_backing term. The bot path needs the term added to the Timer-B invariant (`check_invariant`/`apply_supply_delta`), NOT to reconcile's excess check.
- Fix: Split the §5.4 change precisely: Timer-B `check_invariant` + `apply_supply_delta` get the 3-term RHS (debt+reserve+pending_burn). `reconcile_chain_supply`'s `unbacked_excess` check must stay `onchain > chain_supplies + in_flight` (reserve_backing does NOT belong on the RHS of an unbacked-EXCESS test). reserve_backing can be ADDED as a reported breakdown field on `ChainSupplyReconciliation` for operator visibility, but must not enter the excess inequality.


**30. [MED] SP cross-chain control flow (how the SP discovers a bot-failed chain vault is underspecified and does not reuse the existing discovery channel)**
- Issue: The existing SP liquidation discovers vaults via backend-push `notify_liquidatable_vaults(Vec<LiquidatableVaultInfo>)` or SP-pull `get_liquidatable_vaults() -> Vec<CandidVault>` (stability_pool/liquidation.rs:22,112). Both surface ICP vaults from the `vaults` map. Chain vaults live in `MultiChainStateV5.chain_vaults` (a different type, no `CandidVault`), and the Tier-2 trigger condition is "the BOT failed" — a backend-internal transient state (`sp_attempted_chain_vaults`, swap op Failed) that is NOT visible through `get_liquidatable_vaults`. §6 specifies the burn/absorb mechanics but not the discovery/handoff: who calls `stability_pool_liquidate_chain_vault`, and how does the SP know (a) a chain vault is liquidatable AND (b) the bot already gave up? Without a defined channel this is a gap, not a reuse.
- Fix: Specify the chain-vault discovery channel: a new `get_chain_liquidatable_vaults()` that returns ONLY vaults where the bot has failed/escalated (gated on `sp_attempted_chain_vaults` not-yet-set + a Failed/absent swap op), plus whether the SP pulls it or the backend pushes a chain-vault analog of `notify_liquidatable_vaults`. Define the `LiquidatableVaultInfo` analog carrying the CFX collateral_type sentinel. This is a real new inter-canister contract, not a clone of the ICP path.


**31. [MED] DEX swap encoding correctness (router signature, decimals scale, and getReserves token ordering all carry latent revert/loss risk)**
- Issue: §4.8 is mostly right (swapExactETHForTokens with native CFX in `value`, no pre-wrap, USDC settle) but has three load-bearing assumptions stated as TODOs that, if wrong, silently break the bot tier: (1) selector — canonical `swapExactETHForTokens(uint256,address[],address,uint256)` = 0x7ff36ab5, but Swappi is a fork and the deployed router signature MUST be confirmed against the ROUTER (the liquidity facts only give the FACTORY 0xe2a6f7…; the router address is NOT in the spec and is required). (2) The new dynamic `address[]` encoder is genuinely net-new — `tx.rs` only has `abi_word_address`/`abi_word_u128` static-head helpers (confirmed); the offset/length/tail layout in §4.8 must be tested against a `cast`/Foundry reference because it is the ONLY dynamic-type encode in the codebase. (3) All eSpace tokens are 18-dec (per liquidity facts), but the e8s↔native boundary math (§4.8 '1e10 scale conversion') and `collateral_ratio_e4`'s `10^native_decimals` (vault.rs:192) must agree; a USDC-decimals mismatch (the config carries `settle_stable_decimals` precisely because a future chain is 6-dec) is a silent value error.
- Fix: Make the ROUTER address a required §8 onboarding input (distinct from the factory) with a set-time factory-derived-pair sanity check (the spec already suggests this — elevate to mandatory). Keep the Foundry-reference encoder test in Increment 3 as a gate. Add an explicit unit test asserting USDC realized-out (18-dec) → e8s and the oracle cross-check use `settle_stable_decimals`, not a hardcoded 1e10, so the chain-agnostic claim holds.


**32. [MED] Testing strategy: PocketIC mock-router fidelity vs the real consensus/quorum failure modes**
- Issue: §11's plan to extend `evm_rpc_override` with canned `getReserves`/`getAmountsOut`/receipt-with-Transfer-log is sound for covering the do-not-swap branches and the realized-out decode. BUT a canned single-response mock CANNOT exercise the two failure modes that will actually bite on mainnet: (a) the multi-provider quorum DISAGREEMENT on a volatile getReserves (finding 2) — the mock returns one deterministic value, so the quorum always agrees and the test gives false confidence; (b) the never-mined-swap timeout→escalate path (finding 1) — needs a mock that returns `null` receipt indefinitely. The testnet-no-liquidity constraint is correctly identified, but the mock as described validates the HAPPY encoding, not the integration's real-world fragility.
- Fix: Add PocketIC scenarios that (a) return DIFFERENT getReserves values across the configured providers and assert the read fails-closed → escalate (proving the quorum interaction is understood, then validating whichever fix from finding 2 is chosen), and (b) return a perpetually-null receipt and assert the op reaches Failed via the new timeout and escalates to SP without blocking a subsequently-enqueued user withdrawal (proving finding 3 is mitigated). Keep the mainnet-fork dry-run as the only real-DEX gate before enabled=true.


**33. [MED] Candid / upgrade-arg surface (new types + enum-variant adds will trip the breaking-change check on a stable-memory canister)**
- Issue: The engine adds to the .did: new methods (`liquidate_chain_vault`, `set/get_chain_liquidation_config`, `get_chain_liquidatable_vaults`, `settle_reserve_burn`, `get_chain_reserves`, SP `stability_pool_liquidate_chain_vault`/`claim_cfx`), new types (`ChainLiquidationConfigV1`, `DexKind`, `PendingLiquidationV1`, `ChainLiqClaimV1`), new EVENT variants (`ChainVaultLiquidated`, `ChainReserveCredited`, etc.), and a new `SettlementOpKind::LiquidationSwap` variant. Per MEMORY, adding an enum variant to chains candid types ALREADY produced a breaking-change warning on the staging deploy (the EvmAuth case). §8 flags "never -y past a warning" but does not enumerate WHICH of these additions are wire-incompatible vs purely additive. `SettlementOpKind` is inside the persisted V6 root AND on the candid surface (it appears in event/query types), so adding `LiquidationSwap` is both a CBOR concern (covered by the V6 bump) and a candid concern. The spec does not state which additions need a candid review pass.
- Fix: Add a candid checklist to §8/§12: every new method + type goes in `rumi_protocol_backend.did` and `stability_pool`'s .did; run `didc`/the breaking-change check and treat any warning as a STOP (the project rule). Note explicitly that `SettlementOpKind::LiquidationSwap` and the new event variants are additive-but-warning-prone enum extensions and verify each decodes against the prior .did before deploy.


**34. [LOW] Spec file-path references are wrong for the invariant timer (xrc.rs lives at backend root, not chains/), risking the four-consumer change missing the actual call site**
- Issue: The spec repeatedly cites the Timer-B self-check at chains/xrc.rs (e.g. '5.4 item 3: xrc.rs:367', 'Section 2 tree: xrc.rs EXTEND'). The actual self-check is in src/rumi_protocol_backend/src/xrc.rs (root, line ~367-390, interest_and_treasury_tick / Timer B), and there is NO chains/xrc.rs file. The check_invariant call that must change to pass debt+reserve+pending is at the root xrc.rs:369, and the GA->ReadOnly halt at xrc.rs:377-383. A reader following the spec's path could edit a non-existent file or miss the real halt site.
- Fix: Correct every chains/xrc.rs reference to src/rumi_protocol_backend/src/xrc.rs and pin the exact lines (check_invariant call ~369, invariant_halted/ReadOnly flip ~377-383, chain_debt_e8s read ~367). This is the single most blast-radius-critical edit (the false-halt site) so its path must be exact in the spec.


**35. [LOW] penalty-surplus-as-reserve (5.6) leaves reserve_usdc_native structurally > reserve_backing_e8s, which the bridge-settlement burn model (5.5) cannot fully retire**
- Issue: 5.5 settle_reserve_burn drops chain_supplies and reserve_backing_e8s together when a human bridges and burns foreign icUSD. But reserve_usdc_native always exceeds reserve_backing_e8s by the accumulated 12% penalty surplus (5.6, intentional). settle_reserve_burn retires the BACKING term but the physical USDC custody (reserve_usdc_native) has MORE dollars than backing retired. Over many liquidations the surplus USDC accumulates in custody with no invariant term tracking its disposition — it is real protocol revenue but the books only ever debit reserve_usdc_native by the bridged amount, leaving a growing residue. get_chain_reserves (5.5) surfaces onchain_usdc_balance vs reserve_usdc_native vs reserve_backing_e8s, which is good, but there is no defined sweep/recognition path for the surplus, so 'books==custody before bridging' (5.5) will NEVER hold exactly — custody always carries the surplus.
- Fix: Define how penalty surplus exits the reserve_usdc_native bucket: a developer-gated recognize_reserve_surplus(chain, amount) that moves surplus USDC out of the liquidation-reserve accounting into protocol revenue (or to treasury), so reserve_usdc_native tracks only debt-backing USDC and books==custody can actually hold post-recognition. Otherwise document explicitly that onchain_usdc_balance is expected to exceed reserve_backing_e8s by the cumulative penalty and that the operator reconciliation must account for it, so a non-zero gap is not misread as missing/stolen USDC.


**36. [LOW] sp_attempted_chain_vaults surviving upgrade is asymmetric with how it must be pruned**
- Issue: §3.2 says `sp_attempted_chain_vaults` is "reset/pruned on resolution; surviving an upgrade is harmless — worst case a vault waits one extra tick for manual." But the standing rule is ONE SP attempt then manual, FOREVER for that liquidation episode. If the set is pruned on resolution (vault no longer liquidatable) and the vault later becomes liquidatable AGAIN at a different price, it correctly gets a fresh SP attempt — good. The risk is the opposite: if a vault is in the set, falls to manual, the operator manually resolves it, but the prune-on-resolution doesn't fire (e.g. resolution path forgets to clear it), the vault is permanently barred from SP on a future independent under-collateralization. The spec doesn't specify WHICH transition clears the set entry, and an upgrade freezing a stale entry then becomes not-harmless if the clear is keyed to an event that the upgrade interrupts.
- Fix: Specify the exact clear point: remove from `sp_attempted_chain_vaults` whenever the vault's CR climbs back above `min_cr_e4` (no longer liquidatable) OR when a liquidation episode fully resolves (Closed / debt cleared), checked in the same trigger scan that reads it. Make the clear idempotent and derivable from current vault state alone (CR-based), so an upgrade mid-episode cannot strand a stale entry. Test: vault enters SP-attempted, recovers above threshold, drops again later, and gets a fresh SP attempt.


**37. [LOW] begin_liquidation enqueue-then-reserve ordering vs idempotency key with now_ns**
- Issue: §4.9 Phase 1 uses idempotency key `liquidate-{chain}-{vault}-{now_ns}` and enqueues BEFORE setting the marker (correct, mirrors withdraw). But because the key embeds `now_ns`, two trigger ticks that both observe the vault liquidatable in the SAME scan-but-different-now (e.g. the heap guard released between ticks because begin_liquidation is synchronous and doesn't hold it across ticks) could each enqueue a DISTINCT LiquidationSwap op for the same vault with different now_ns — the idempotency set won't dedup them. The §4.4/§4.5 marker check (`pending_liquidation.is_none()`) is what actually prevents this, and the synchronous begin_liquidation sets the marker in the same message-turn as the enqueue, so within one canister there is no real interleave (no await between the marker read and the enqueue). The residual risk is only if the trigger scan and begin_liquidation are split across an await, which the spec doesn't make explicit.
- Fix: State explicitly that the marker check (`pending_liquidation.is_none()`) and the enqueue+marker-set occur in ONE synchronous mutate_state with NO await between (matching withdraw_collateral_in_state's structure), so the marker is the dedup, not the idempotency key. If the per-tick trigger must await (e.g. fresh price read) before begin_liquidation, re-read the marker inside the final synchronous mutation. Add a test firing two begin_liquidation calls for one vault in immediate succession and asserting exactly one op + one marker.


**38. [LOW] Spec file-path references to non-existent chains files (spec section 2 file map, closing references)**
- Issue: The spec's architecture map and closing 'Spec file references' list `chains/event.rs` and `chains/xrc.rs` as files to EXTEND, but these do not exist under `src/rumi_protocol_backend/src/chains/` (verified: chains/ has no event.rs or xrc.rs; `event.rs` and `xrc.rs` are top-level `src/` files, and chains events appear to live elsewhere). `collateral_config.rs` is correctly located. This is a minor accuracy issue but it undermines confidence in the 'all numbers reconciled against actual code' claim and could send the implementer editing the wrong (ICP-scoped) event.rs/xrc.rs, risking accidental changes to ICP liquidation event types or the ICP Timer-B self-check.
- Fix: Re-resolve the actual locations: chains events and the chains Timer-B self-check operand. The spec references `xrc.rs:367/377` for the Timer-B self-check, which is the TOP-LEVEL xrc.rs (ICP), so the chains supply-invariant check must be carefully scoped to NOT alter the ICP oracle/self-check path. Correct the file map so the implementer extends the right module, and confirm whether a chains-specific event enum exists or whether chain events ride the top-level `event.rs` (which would mean the V6 event additions touch a shared, ICP-load-bearing file and need the same breaking-change discipline as the .did).


**39. [LOW] Gas-limit claim drift (tx.rs comment contradicts the spec's recollection)**
- Issue: §4.8 says "the Mint arm hit OOG at 120k before bumping to 300k" and proposes hardcoding `gas_limit = 250_000` for the swap kind. The actual code (tx.rs:90-96) documents eSpace meters the icUSD `mint` at ~177.5k (via eth_estimateGas) and the cap is 300k for headroom — so a 250k swap cap on eSpace is plausibly TOO LOW for a router swap (which does more work than a bare mint: wrap + transfer + pair update). The spec hedges with "consider 400_000" but then states 250k as the value, and also adds a `MonadTxKind::Swap` arm — note `MonadTxKind` is the Monad-named-but-shared enum (tx.rs:41); adding a Swap arm there is fine but the naming is misleading for a Conflux feature.
- Fix: Measure the Swappi swap gas on eSpace mainnet via eth_estimateGas before fixing the constant; given mint is ~177.5k for a trivial op, budget the swap cap at 400k+ (it is a ceiling, only used gas is charged). Optionally rename `MonadTxKind` → `EvmTxKind` while adding the Swap arm, since the spec is making it genuinely chain-agnostic.


**40. [LOW] ChainCollateralConfig is compile-time, so the 133→150 open-gate change and debt-ceiling set are redeploys, not runtime setters**
- Issue: §4.1.1 and §9 speak of "raise the Conflux open gate to 150%" and "set Conflux debt_ceiling_e8s" as if settable, but `ChainCollateralConfig` (collateral_config.rs:1-54) is a `const ICP_MIRROR` resolved by a compile-time `match chain.0` — the module header explicitly defers runtime admin-settability. So changing the open gate (via `evm_vault_params`, main.rs:1147, which today returns `min_cr_e4`) and setting the debt ceiling both require a code edit + recompile + canister upgrade, plus updating the test at main.rs:8261 that asserts `evm_vault_params(71) == ("CFX", 13_300)`. The spec's framing as operator config could mislead the implementer into looking for a setter that does not exist.
- Fix: State plainly that the open-gate change and Conflux debt-ceiling are compile-time edits (collateral_config.rs + evm_vault_params + its unit test), shipped in Increment 0/1 and tunable only by redeploy — consistent with the spec's own note that promoting these to Tier-B persisted config is an Increment-5 follow-up. This keeps the "DEX params are live-tunable, risk params are compile-time" split honest.
