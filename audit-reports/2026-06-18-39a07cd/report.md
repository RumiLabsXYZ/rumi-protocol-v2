# Security Audit — Conflux/Monad native-collateral chains rail

**Anchor:** `origin/main` @ `39a07cd1c3fcfbb0acedc975d4ba4ced2d25f71d` (PR #251 Conflux rail + PR #252 Option-B interest accrual).
**Date:** 2026-06-18. **Auditor:** automated harness (audit-icp-cdp), 10 specialist passes + adversarial verification + manual reachability resolution. **Dirty files:** none.
**Purpose:** pre-mainnet gate for a **gated eSpace-mainnet soft-launch** (chain 1030, small debt ceiling, dev/allowlist-gated opens, manual liquidation + monitoring).

## Bottom line

**GO for the gated soft-launch, with conditions (below). NO confirmed real bug in the interest-accrual work (PR #252); no regression.** Every finding the harness raised against the new interest path was verified to be a **false positive** (IC message atomicity + the existing in-code guards). The substantive findings are **pre-existing chains-rail properties that are already on the documented mainnet-gate list** — chiefly the manual price oracle. They are mitigated, not eliminated, by the gated-launch controls.

11 candidate findings → after adversarial verification + manual resolution of a verifier conflict:
- **1 HIGH** (pre-existing): manual price oracle has no staleness/bounds.
- **1 MEDIUM** (pre-existing, theoretical): burn-watch idempotency map can grow unbounded during a deep reorg + stalled cursor.
- **2 LOW** (pre-existing): price-set endpoint lacks bounds/delta/timelock; RPC provider concentration.
- **1 INFORMATIONAL** (new code, defense-in-depth): `confirm_interest_mint_in_state` has no local status guard (currently unreachable to exploit).
- **5 verified FALSE POSITIVES** (documented below so a re-audit doesn't re-raise them).

## Findings (severity order)

### F-01 — HIGH — Manual price oracle: no staleness or bounds → undercollateralization / freeze
- **Category:** CDP-domain / oracle integrity. **Status vs prior audit:** pre-existing (the chains rail has always used `manual_prices` with no freshness). **In PR #252?** No — unrelated to interest accrual.
- **Locations:** `chains/vault.rs:collateral_ratio_e4` + the open/withdraw CR checks; `chains/multi_chain_state.rs` `manual_prices: BTreeMap<(ChainId,String),u64>` (no timestamp); `main.rs:set_manual_collateral_price` (only rejects `price_e8==0`).
- **Mechanism:** CR = `collateral_native * price_e8 / 10^dec / debt_e8s * 10_000`. `price_e8` is an admin-set scalar with **no timestamp, staleness check, or sanity bound**. If the CFX market moves and the admin doesn't refresh, a vault that opened over-MCR at the stale price is silently undercollateralized at the true price; a `price_e8` driven to a low value freezes all open/withdraw (BelowMinCr) for vaults with debt.
- **Why HIGH not CRITICAL:** requires admin negligence or a market event, not a single attacker-controlled input. For the **gated soft-launch** the blast radius is bounded by dev-gated opens + a small debt ceiling, so it is **mitigated** (see conditions). For a **public launch** it is a hard gate.
- **Recommendation (mainnet):** store `(price_e8, updated_at_ns)` per `(chain,symbol)`; reject CR-relevant ops when `now - updated_at > MAX_PRICE_AGE`; emit a `PriceSet` event for off-chain staleness alerting; move to an automated oracle (XRC already carries CFX via 6/9 sources; or Pyth/Chainlink on EVM) before removing the caps. **Owner:** chains rail (coordinate with the `feat/conflux-evm-self-serve-auth` session; not edited here).

### F-02 — MEDIUM — `processed_burn_keys` can grow unbounded during a deep reorg + stalled cursor
- **Category:** IC-platform / oracle-finality. **Status:** pre-existing (the code's own docstring flags "revisit for deeper finality / higher vault counts"). **In PR #252?** No.
- **Location:** `chains/evm/deposit_watch.rs:advance_cursor_and_prune` (~865-891).
- **Mechanism:** the burn-watch idempotency set is pruned only when the cursor advances (`finalized > last_observed`). With Conflux `finality_depth=100`, a deep reorg that regresses/stalls the finalized block below the cursor — or a reorg-halt awaiting operator `clear_reorg_halt` — stops pruning while new burns keep being recorded, so the `BTreeMap<u64,BTreeSet<String>>` grows. Theoretical (needs reorg/halt + prolonged stall + many burns); no fund loss or invariant breach.
- **Recommendation:** add block-count/time-based eviction independent of cursor advance (e.g. retain only the last N blocks' keys; the on-chain `minted[vault_id]` guard backstops a re-applied burn). Monitor map size. **Owner:** chains rail.

### F-03 — LOW — `set_manual_collateral_price` lacks bounds / delta / timelock
- **Category:** CDP-domain / oracle. **Status:** pre-existing. Developer-gated, reversible, fully observable. A fat-finger or compromised dev key can set a nonsensical price (the only check is `> 0`). Per the audit brief, admin-only = LOW. **Recommendation:** %-band-vs-previous check, price history, optional timelock for mainnet. Folds into F-01's oracle work.

### F-04 — LOW — RPC provider concentration (Confura-only); mainnet needs independent providers
- **Category:** IC-platform / quorum. **Status:** known mainnet gate. NOTE the harness's "min_quorum=1 means 1-of-N" claim was a **false positive**: required agreement is `max(floor=1, strict_majority=2) = 2`, so reads still need 2-of-3 agreement. The real risk is that all three eSpace endpoints are one operator (Confura), so it is a 1-of-1 *trust* model. Fine for testnet; **mainnet requires ≥3 independent providers** (already on the gate list).

### F-05 — INFORMATIONAL — `confirm_interest_mint_in_state` relies on upstream invariants (no local status guard)
- **Category:** CDP-domain / debt-interest. **In PR #252?** Yes (new code). **Verdict: currently UNREACHABLE to exploit — defense-in-depth only.**
- **The concern (two verifiers conflicted; resolved manually):** `confirm_interest_mint_in_state` (settlement.rs:147) has no `status == Open` check, and `close_chain_vault_in_state`'s zero-collateral shortcut (vault.rs:575) stamps `Closed` without the `InterestRealizationPending` guard. **But the exploit precondition `(Open, collateral == 0)` is unreachable:** open requires collateral > 0 (CR), partial withdraws stay CR-checked > 0, and a full withdraw sets status to **`Closing`** (not Open) — and any withdraw is blocked while `pending_interest_mint_e8s > 0` (vault.rs:444). So a pending-interest vault can never reach `Closed` before its interest mint confirms. Verified by reading vault.rs:424-452 + 555-594.
- **Why still worth a cheap fix:** the safety currently rests on an *upstream* invariant (the close shortcut never coinciding with a pending-interest, collateral-0, Open vault). A future change to the close shortcut, the withdraw guard, or a new collateral-0-Open path would silently let interest credit a Closed vault. **Recommendation (cheap, defense-in-depth):** add `if vault.status != Open { return Err(...); }` to `confirm_interest_mint_in_state` (leave the op Inflight / mark Failed), and add `pending_interest_mint_e8s == 0` to the close shortcut guard. **Owner:** whoever next edits chains/ (the M2 session) — not edited here.

### F-06 — LOW (theoretical) — Withdrawal CR validated at enqueue, not at settlement confirm
- **Category:** CDP-domain / CR math. **Status:** pre-existing. Between a withdraw enqueue and its on-chain confirm, an admin could move the manual price, so the withdrawal was validated against a now-stale price. Requires admin action on their own op (admin-gated → LOW/theoretical). Re-checking CR at confirm (or snapshotting the price on the op) would harden it. Subsumed by F-01's oracle work.

## Verified FALSE POSITIVES (recorded so a re-audit collapses them)
1. **"Interest harvest vs settlement confirm race on pre_total"** (settlement.rs:1127) — the `read_state(pre_total)` is immediately followed by `mutate_state(confirm)` with **no `.await` between**; IC messages run atomically between awaits, so no observer-burn or cross-chain settlement can interleave. (Raised twice — also in the prior code review. Pattern is confusing; see F-05's note — moving the read inside the mutate would stop the recurring false alarm.)
2. **Double add-back on reverted NativeWithdrawal** (settlement.rs:867) — the per-op CAS (`still Inflight?`) inside the single `mutate_state` makes the second tick a no-op.
3. **Reused vault IDs carry stale `last_interest_accrual_ns`** — `chain_vault_id_counter` is monotonic (never reused; ~1.8e19 opens to overflow) and the on-chain `minted[vault_id]` guard would block a reused-id mint regardless.
4. **Interest accrual rounding / resubmit double-count** — the `pending_interest_mint_e8s != 0` harvest filter (interest.rs:72) blocks re-harvest while a mint is in flight; resubmit re-uses the stored `op.amount_e8s`, not a fresh accrual.
5. **min_quorum=1 ⇒ 1-of-N reads** — required agreement is `max(1, strict_majority)=2`; reclassified as F-04 (operator concentration).

## Methodology + coverage
See `methodology.md` and `coverage-crosscheck.md`. 10 read-only specialist passes (Explore agents) over `chains/` + `IcUSD.sol`; every medium+ candidate independently re-verified; the supply-invariant / mint / withdraw / interest passes cross-checked against the grep-authoritative enumeration of all 18 chain entry points, the settlement/observer guard sites, and the 70 supply-mutation sites (no entry-point coverage gap).

## Limitations
- Read-only review of the **chains rail only**; the ICP-native engine, the M2 self-serve auth (`feat/conflux-evm-self-serve-auth`), and chains liquidation (not built) are out of scope.
- Manual prices, single-operator RPC, and deferred automated liquidation are accepted properties of a *gated, capped, dev-gated* soft-launch; they are **hard gates for a public launch**.
- No live-chain (real reorg / real oracle manipulation) testing; F-01/F-02 reproductions are documented, not executed as PoC tests.

## Gated-soft-launch conditions (the audit's go-with-conditions)
1. **Keep opens dev/allowlist-gated + a small `debt_ceiling_e8s`** on chain 1030 — caps the blast radius of F-01.
2. **Active CFX price discipline:** a refresh cadence + off-chain staleness/large-move alerting (mitigates F-01/F-03/F-06 until the staleness check / automated oracle lands).
3. **Manual-liquidation playbook ready** (automated liquidation is deferred).
4. **Coordinate the F-05 defense-in-depth one-liner** with the chains/M2 session (cheap, not blocking).
5. **Before lifting the caps to a public launch:** F-01 (oracle staleness/automation), F-04 (independent RPC providers), F-02 (burn-key eviction).
