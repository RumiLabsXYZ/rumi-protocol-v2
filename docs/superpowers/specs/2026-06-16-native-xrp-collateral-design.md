# Native XRP collateral for Rumi — design

Status: **spec / not started.** Goal: let a user deposit native XRP (held on the
XRP Ledger via threshold Ed25519) as Rumi CDP collateral and mint icUSD **on the
IC**. This is distinct from ckXRP (a separate, independent repo): here XRP stays
native and plugs into Rumi's existing CDP, reserves, and redemption.

Date: 2026-06-16.

## Why this is smaller than it first looks

Three pieces already exist, so the new work is narrow:

1. **The XRPL rail** (`chains/xrp/`, merged): address derivation, Payment codec,
   threshold-Ed25519 sign, rippled RPC (account_info/server_state/submit/tx),
   golden-vector-locked. It already does deposit-verify and withdrawal-signing.
2. **Stable-settled liquidation + reserves**: `liquidate_vault_partial_with_stable`
   ([vault.rs:2728](../../../src/rumi_protocol_backend/src/vault.rs)) lets a
   liquidator pay ckUSDC/ckUSDT (1:1 + fee) into the protocol's reserves;
   `redeem_reserves` ([vault.rs:218](../../../src/rumi_protocol_backend/src/vault.rs))
   backs icUSD redemption from those reserves; the invariant
   (`test_stablecoin_repayment_does_not_increase_icusd_supply`) keeps supply
   honest. A juicy XRP liquidation therefore *fills reserves with hard stables* —
   a feature, not a problem to engineer around.
3. **Pricing**: `PriceSource::Xrc { base_asset: "XRP" }` (confirm XRC quotes XRP;
   it almost certainly does) with `PriceSource::CoinGecko` as a proven fallback.

So the remaining work is: the **deposit pipeline (XRP in)**, a **custody
abstraction** in the collateral model, and a **claim-based collateral-out branch**
on the existing liquidation/withdraw/redeem paths.

## Design decisions (recommendations to confirm)

### D1. Custody model — DECIDED: per-vault threshold-derived addresses (2026-06-16)
- **Per-vault address** (`custody_derivation_path(chain, user, nonce)`, already in
  the rail): each vault gets its own XRPL address. Clean 1:1 attribution (whatever
  sits at that address is that vault's collateral), no commingling, no tag
  bookkeeping. Cost: each address locks the ~1 XRP base reserve. **For a CDP with
  meaningfully-sized vaults that overhead is negligible** — recommend this.
- *Alternative* — single account + destination tags (capital-efficient, one
  reserve, but commingled + tag-accounting + uncreditable-on-missing-tag). Better
  for ckXRP's many-small-users case; worse for a CDP. (Note the two products land
  on opposite choices, and that's correct.)

### D2. CollateralConfig representation — RECOMMEND a `custody_type` discriminant
`CollateralConfig` is keyed by `ledger_canister_id: Principal` and assumes ICRC
custody. Add `custody_type: Custody { IcrcLedger(Principal) | XrpLedger }` and key
XRP under a synthetic/reserved principal (it has no IC ledger). Branch the four
custody touchpoints on it: deposit-in, withdraw-out, liquidation-payout,
redemption-payout. Everything else (CR math, debt ceiling, interest, min-CR,
redemption tier) is already per-collateral and reused unchanged. **No new
`MultiChainState` field needed** (avoids UPG-002 risk); the XRP custody addresses
are derived on demand.

### D3. Deposit flow — RECOMMEND open-then-verify (Rob's existing Monad pattern)
1. `open_xrp_vault` → derive the vault's custody address, return it (state:
   `AwaitingDeposit`, no collateral credited, no mint).
2. User sends XRP to that address (must exceed the base reserve to activate).
3. `confirm_xrp_deposit(vault_id, tx_hash)` → rail verifies `validated` +
   `tesSUCCESS`, reads the **`delivered_amount`** (NOT `Amount` — partial-payment
   trap; the rail's `tx` parser must be extended to read it), credits collateral.
4. Borrow mints icUSD on the IC via the existing `mint_icusd`.

### D4. Collateral-out — RECOMMEND the claim model
Branch the tail of `liquidate_vault_partial_with_stable` / `withdraw_collateral` /
`redeem_reserves`: instead of an ICRC transfer of the collateral, record an **XRP
claim** (principal → drops owed) and settle it via a threshold-signed Payment when
the claimant supplies an XRPL address (now or later). The XRP is already at a
protocol-controlled address, so settlement is just the rail's `sign_withdrawal`.
This is a small, well-scoped branch, not a new subsystem.

## Liquidation model (per Rob)

- High MCR for headroom; XRP price from XRC (10-min staleness ceiling, VER-001).
- Liquidatable vaults go to **manual / external** liquidation — anyone holding
  ckUSDC/ckUSDT can liquidate via the existing stable path; the stables land in
  reserves (great for redemption). No stability-pool absorption of XRP.
- Partial liquidation already supported (`liquidate_vault_partial*`), so a whale
  vault can be unwound in a loop (liquidate → sell → repeat).
- Optional: a Rob-operated bot that bids stables and disposes XRP (CEX/OTC). Noted
  as a centralization choice for a pilot, not part of the trustless core.

## Deferred wiring to switch on

- The four `xrp_rpc` transforms must be exposed as `#[query]` shims in `main.rs`
  (`xrp_transform_account/server/submit/tx`) for the outcalls to run.
- Threshold key resolution: derive + persist the per-vault custody addresses;
  define who may trigger it.
- Register XRP as a collateral (dev-gated) with its parameters.

## Parameters (DECIDED 2026-06-16)

All of these are **runtime, per-collateral, developer-gated setters** — none are
compiled into the wasm, so they're tunable live with no redeploy.

| Parameter | Value | Field / setter |
| --- | --- | --- |
| Borrow threshold (MCR) | **150%** | `borrow_threshold_ratio` / `set_collateral_borrow_threshold` |
| Liquidation ratio | **133%** | `liquidation_ratio` / `set_collateral_liquidation_ratio` |
| Recovery CR | **155%** | falls out of borrow 150% × the existing 1.0333 recovery multiplier (or set `recovery_target_cr` = 1.55 explicitly) |
| Liquidation penalty | **12%** | `liquidation_bonus` / `set_collateral_liquidation_bonus` (mirror ICP's encoding — i.e. seize 1.12× proportional collateral) |
| Borrowing fee curve | **same as ICP** | `borrowing_fee` + `borrowing_fee_curve` — copy ICP's config |
| Interest rate curve | **same as ICP** | `interest_rate_apr` + `rate_curve` (`None` ⇒ inherit `global_rate_curve`) — match whatever ICP uses |
| Debt ceiling | **$200** (= 20_000_000_000 e8s) | `debt_ceiling` / `set_collateral_debt_ceiling` — bump anytime |
| Base reserve (~1 XRP) | **user-funded** | netted from the first deposit; credited collateral = `delivered_amount` − `reserve_base` |
| Min vault debt / ledger fee / redemption tier+fees | mirror ICP (TBD) | `set_collateral_min_vault_debt`, etc. |

Two things to confirm against ICP's live config at implementation: (a) the exact
`liquidation_bonus` encoding (1.12 multiplier vs 0.12 fraction); (b) whether ICP
uses `global_rate_curve` or a per-asset `rate_curve`/`borrowing_fee_curve`, so
"same as ICP" copies the right thing.

## Risks

- **Flash-crash tail**: XRP gaps below water faster than liquidators act → bad
  debt. Mitigation: high MCR + capped debt ceiling; accepted for the pilot.
- **Deposit-side traps**: partial payments (credit `delivered_amount`), base
  reserve/activation, sequence serialization (one in-flight tx/account). This is
  where care concentrates.
- **Oracle**: XRC staleness/source-floor; CoinGecko fallback. Single-cluster
  rippled trust in Phase 1.
- **Highest-stakes class**: this mints live icUSD against off-chain collateral.
  Full security audit before mainnet; dev-gated, capped rollout.

## Phased plan

- **P1** — extend `xrp_rpc` `tx` parser for `delivered_amount`; add the `#[query]`
  transform shims; key resolution + custody-address derivation (read-only,
  observable: can price XRP + hand out deposit addresses).
- **P2** — `custody_type` on CollateralConfig; branch deposit-in.
- **P3** — open-then-verify deposit → credit → borrow → mint icUSD on IC.
- **P4** — claim model: branch liquidation/withdraw/redeem out-paths + XRP claims
  ledger + settle-via-Payment endpoint; sequence guard.
- **P5** — register XRP collateral (dev-gated); XRPL-testnet end-to-end via
  PocketIC + mock rippled; parameters; frontend (deposit-address + tag/claim UX).
- **P6** — security audit; capped mainnet pilot.

## Status of decisions

Resolved (2026-06-16): per-vault custody addresses; **user funds** the ~1 XRP base
reserve (netted from first deposit); 150% borrow / 133% liquidation / 155%
recovery / 12% penalty; ICP's borrowing-fee + interest-rate curves; **$200** debt
ceiling via `set_collateral_debt_ceiling` (bump later). Native is a real near-term
build.

Remaining judgment call: how much to harden P1-P5 in-house vs. drive straight to
an external audit, given this mints live icUSD against off-chain collateral. The
$200 ceiling keeps the pilot blast-radius tiny while confidence is built.
