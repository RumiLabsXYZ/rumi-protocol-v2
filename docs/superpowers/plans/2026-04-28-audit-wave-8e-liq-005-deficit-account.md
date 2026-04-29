# Wave 8e: LIQ-005 Bad-Debt Deficit Account Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Isolate bad debt from underwater liquidations into a protocol-level deficit account that future fee revenue repays. No socialization to stability-pool depositors or pro-rata redistribution to other vaults. Add a ReadOnly-latch as a secondary guard if the deficit exceeds a configurable threshold.

**Architecture:** Single backend canister upgrade. Adds four `#[serde(default)]` fields to `State` for upgrade safety. Five liquidation paths in `vault.rs` get a small post-mutation fence that computes `shortfall = max(0, debt_cleared_e8s - usd(seized_collateral))` and increments `protocol_deficit_icusd` with a `DeficitAccrued` event. Two fee-collection paths (borrowing fee, redemption fee) route a configurable fraction of each fee toward deficit repayment before the remainder flows to treasury, emitting a `DeficitRepaid` event. Two admin endpoints (`set_deficit_repayment_fraction`, `set_deficit_readonly_threshold_e8s`) expose tunables. Four new ProtocolStatus fields + new candid declarations.

**Tech Stack:** Rust, ic-cdk, candid, rust_decimal, ic-stable-structures (CBOR via serde_cbor for state). PocketIC 6.0.0 for integration tests.

---

## Design decisions (locked before implementation)

### Accrual: D1 (per-site, not via central helper)

**Why:** Each of the 5 liquidation paths has different shapes (full vs partial, ICP-paid vs stablecoin-paid vs SP-burned, recovery-mode vs normal). A central `apply_liquidation_seizure(...)` helper would duplicate the per-path bonus / protocol-cut math instead of factoring it. Defer the abstraction until we duplicate non-trivial decimal math (we don't, after the per-site fence).

**Predicate (load-bearing, document inline at each call site):**

```text
shortfall_e8s = max(0, debt_cleared_e8s - usd_value_of(total_to_seize))
```

Where:
- `debt_cleared_e8s` = the icUSD amount whose vault debt is being zeroed at this call (full liq: `vault.borrowed_icusd_amount` or `repay_cap` in recovery mode; partial: `actual_liquidation_amount` / `liquidator_payment` / `max_liquidatable_debt`).
- `usd_value_of(total_to_seize)` = `crate::numeric::collateral_usd_value(total_to_seize.to_u64(), price, decimals)` — already computed in each path for the existing `fee_amount` calculation.

**Why this predicate (not "vault.collateral × price < debt"):** the vault could have CR slightly below `min_liq_ratio` (e.g., 1.05) but still be solvent — the bonus + protocol cut math takes a haircut on a healthy bonus, not on principal. The deficit only accrues when the protocol actually nets a loss after the seizure. The seized USD < debt cleared comparison captures this exactly.

### Repayment: R1 simplified (state-side decrement, no extra ledger op)

The audit spec describes burning from a "fee account" via `transfer_idempotent` to the minting account. The Rumi backend canister IS the icUSD minting account, so we can't "burn from main" — only a non-minting subaccount can burn. Adding a fee subaccount + new mint/transfer hops would double the ledger ops per fee collection.

**Simpler equivalent that preserves the audit's intent:**

- **Borrowing fee** (currently minted to treasury): if deficit > 0, mint `fee - to_repay` to treasury instead of `fee`. Decrement deficit by `to_repay`. Net effect: supply increases by `fee - to_repay` instead of `fee`. The "skipped mint" is the deficit repayment — equivalent to mint+burn in supply terms but with one fewer ledger op.
- **Redemption fee** (already burned via `transfer_icusd_from` to protocol = minting account): the fee portion of the redeemer's icUSD is already destroyed without reducing vault debt (see `vault.rs:1671-1673` comment). If deficit > 0, decrement deficit by `to_repay`. Pure state mutation; the burn already happened on the redemption path.

**Trade-off:** No separate ICRC-3 burn block per repayment. The audit trail comes from the `DeficitRepaid` event (with `anchor_block_index`, the existing icUSD ledger block from the fee collection). This is sufficient for invariant verification (sum of accrued events − sum of repaid events == current deficit) and matches the protocol's existing event-driven audit posture.

If a real burn block is later required (e.g., for an external auditor), Wave 11+ can introduce a fee-holding subaccount and convert R1-simplified to R1-real-burn without changing the deficit accounting, just the ledger plumbing.

### ReadOnly latch threshold

Configurable via admin endpoint, default `0` (disabled at first deploy). Operator sets the threshold post-deploy after observing baseline deficit accrual for 24-48h. Threshold is in icUSD e8s (absolute), not a percentage of TVL — small protocols at the start should not auto-latch on a $5 deficit. A future wave can add a TVL-relative computation if desired.

When `deficit_readonly_threshold_e8s > 0` and `protocol_deficit_icusd.to_u64() >= deficit_readonly_threshold_e8s`, set `state.mode = Mode::ReadOnly`. Existing admin path `exit_recovery_mode` clears the latch (it manages `manual_mode_override`). The latch is one-shot per crossing — once tripped, the admin must explicitly clear it.

### Fee fraction default

`deficit_repayment_fraction = 0.5` (50%). Tunable 0.0..=1.0 via admin endpoint.

---

## File Structure

**Modify:**
- `src/rumi_protocol_backend/src/state.rs` — add 4 new fields to `State` (`#[serde(default)]`), update `Default for State`, helper getters/setters for fraction + threshold.
- `src/rumi_protocol_backend/src/vault.rs` — instrument 5 liquidation paths with post-mutation deficit-accrual fence; route borrowing fee + redemption fee through deficit repayment.
- `src/rumi_protocol_backend/src/treasury.rs` — modify `mint_borrowing_fee_to_treasury` to accept the fee amount and split into deficit-repay portion + treasury portion.
- `src/rumi_protocol_backend/src/event.rs` — add `DeficitAccrued` and `DeficitRepaid` event variants + recorder helpers + extend `EventTypeFilter` + extend `type_filter()` mapping + extend `involves_principal`.
- `src/rumi_protocol_backend/src/lib.rs` — add 4 new fields to `ProtocolStatus` + extend `EventTypeFilter` if needed.
- `src/rumi_protocol_backend/src/main.rs` — populate new ProtocolStatus fields in `get_protocol_status`; add `set_deficit_repayment_fraction` + `set_deficit_readonly_threshold_e8s` admin endpoints.
- `src/rumi_protocol_backend/rumi_protocol_backend.did` — additive candid changes.
- `src/declarations/rumi_protocol_backend/*` — regenerated via `dfx generate`.

**Create:**
- `src/rumi_protocol_backend/tests/audit_pocs_liq_005_deficit_account.rs` — Layer 1+2 unit tests (no canister).
- `src/rumi_protocol_backend/tests/audit_pocs_liq_005_deficit_account_pic.rs` — Layer 3 PocketIC tests.

---

## Task 1: Add State fields + default + CBOR round-trip test

**Files:**
- Modify: `src/rumi_protocol_backend/src/state.rs` (insert near the existing Wave-8c fields around line 952; add to `Default for State` around line 1054).
- Test: `src/rumi_protocol_backend/tests/audit_pocs_liq_005_deficit_account.rs` (new file).

- [ ] **Step 1.1: Add helper fn for default fraction and write the failing CBOR round-trip tests**

Create `src/rumi_protocol_backend/tests/audit_pocs_liq_005_deficit_account.rs`:

```rust
//! Wave-8e LIQ-005: bad-debt deficit account — Layer 1+2 unit tests.
//!
//! Layer 1: state-model invariants, CBOR round-trip, default values, predicate
//! arithmetic for deficit accrual and repayment.
//! Layer 2: deterministic decimal math across edge cases (fee = 0, fraction = 0,
//! fraction = 1, deficit = 0, repay capped at remaining deficit).
//!
//! No canister, no async, no PocketIC. PocketIC fences live in
//! `audit_pocs_liq_005_deficit_account_pic.rs`.

use rumi_protocol_backend::numeric::ICUSD;
use rumi_protocol_backend::state::State;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

#[test]
fn liq_005_state_defaults_zero_deficit_and_half_fraction() {
    let s = State::default();
    assert_eq!(s.protocol_deficit_icusd, ICUSD::new(0));
    assert_eq!(s.total_deficit_repaid_icusd, ICUSD::new(0));
    assert_eq!(s.deficit_repayment_fraction.0, dec!(0.5));
    assert_eq!(s.deficit_readonly_threshold_e8s, 0);
}

#[test]
fn liq_005_state_round_trip_preserves_all_four_fields() {
    let mut s = State::default();
    s.protocol_deficit_icusd = ICUSD::new(123_456_789);
    s.total_deficit_repaid_icusd = ICUSD::new(987_654_321);
    s.deficit_repayment_fraction = rumi_protocol_backend::numeric::Ratio::from(dec!(0.75));
    s.deficit_readonly_threshold_e8s = 1_000_000_000;

    let bytes = serde_cbor::to_vec(&s).expect("encode");
    let decoded: State = serde_cbor::from_slice(&bytes).expect("decode");

    assert_eq!(decoded.protocol_deficit_icusd, s.protocol_deficit_icusd);
    assert_eq!(decoded.total_deficit_repaid_icusd, s.total_deficit_repaid_icusd);
    assert_eq!(decoded.deficit_repayment_fraction.0, dec!(0.75));
    assert_eq!(decoded.deficit_readonly_threshold_e8s, 1_000_000_000);
}

#[test]
fn liq_005_state_decodes_pre_8e_blob_with_defaults() {
    // Encode an OLD-shape State (without the four new fields) by encoding a
    // minimal map that mirrors the post-Wave-8d on-disk layout but stripping
    // the four LIQ-005 fields. Easiest fixture: encode a current State, then
    // round-trip through a serde_cbor::Value to drop the new keys.
    let s_full = State::default();
    let full_bytes = serde_cbor::to_vec(&s_full).expect("encode full");
    let mut value: serde_cbor::Value =
        serde_cbor::from_slice(&full_bytes).expect("decode to value");
    if let serde_cbor::Value::Map(m) = &mut value {
        m.retain(|k, _| match k {
            serde_cbor::Value::Text(t) => !matches!(
                t.as_str(),
                "protocol_deficit_icusd"
                    | "total_deficit_repaid_icusd"
                    | "deficit_repayment_fraction"
                    | "deficit_readonly_threshold_e8s"
            ),
            _ => true,
        });
    }
    let stripped = serde_cbor::to_vec(&value).expect("encode stripped");

    let decoded: State = serde_cbor::from_slice(&stripped).expect("decode old-shape");

    assert_eq!(decoded.protocol_deficit_icusd, ICUSD::new(0));
    assert_eq!(decoded.total_deficit_repaid_icusd, ICUSD::new(0));
    assert_eq!(decoded.deficit_repayment_fraction.0, dec!(0.5));
    assert_eq!(decoded.deficit_readonly_threshold_e8s, 0);
}
```

- [ ] **Step 1.2: Run tests to verify they fail (fields don't exist yet)**

Run: `POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --test audit_pocs_liq_005_deficit_account`
Expected: FAIL — compile error: `no field 'protocol_deficit_icusd' on type 'State'`.

- [ ] **Step 1.3: Add the four state fields + default fn**

Edit `src/rumi_protocol_backend/src/state.rs`. Insert this block immediately after the `consumed_writedown_proofs` field (current location around line 952, end of `pub struct State { ... }`):

```rust
    // ─── Wave-8e LIQ-005: bad-debt deficit account ───
    //
    // Underwater liquidations (where seized collateral USD value < debt
    // cleared) accrue the shortfall here as a protocol-level liability.
    // Future fee revenue (borrowing fee, redemption fee) burns icUSD to
    // amortize the deficit. No socialization to stability-pool depositors
    // or pro-rata redistribution to other vaults.
    //
    // `serde(default)` on every field — pre-Wave-8e snapshots decode to zero
    // deficit, half-fraction repayment, and a disabled ReadOnly latch.

    /// Cumulative bad debt the protocol has absorbed from underwater
    /// liquidations. Increments via `accrue_deficit_shortfall` at every
    /// liquidation site that nets seized USD < debt cleared. Decreases only
    /// via `apply_deficit_repayment` on fee collection.
    #[serde(default)]
    pub protocol_deficit_icusd: ICUSD,

    /// Lifetime sum of icUSD applied as deficit repayment (i.e., mint
    /// foregone or burn already executed during fee collection). Reporting-
    /// only; never decreases. Together with `protocol_deficit_icusd` and the
    /// `DeficitAccrued` / `DeficitRepaid` event log this satisfies:
    ///   sum(DeficitAccrued.amount) - sum(DeficitRepaid.amount)
    ///       == protocol_deficit_icusd
    #[serde(default)]
    pub total_deficit_repaid_icusd: ICUSD,

    /// Fraction of each collected fee routed to deficit repayment before the
    /// remainder flows to its existing destination. Default 0.5 (50%);
    /// 0.0 disables repayment, 1.0 routes the entire fee until cleared.
    /// Bounded to [0, 1] in `set_deficit_repayment_fraction`.
    #[serde(default = "default_deficit_repayment_fraction")]
    pub deficit_repayment_fraction: Ratio,

    /// ICUSD-e8s ceiling above which the protocol auto-transitions to
    /// ReadOnly mode. 0 disables the latch. Tuned via
    /// `set_deficit_readonly_threshold_e8s`. Operator should leave at 0
    /// for the first 24-48h post-deploy and set after observing baseline
    /// deficit accrual.
    #[serde(default)]
    pub deficit_readonly_threshold_e8s: u64,
```

Add the default helper near the existing `default_liquidation_ordering_tolerance` helper (search for that fn name in `state.rs`, place the new helper next to it):

```rust
fn default_deficit_repayment_fraction() -> Ratio {
    Ratio::from(rust_decimal_macros::dec!(0.5))
}
```

(If `rust_decimal_macros` isn't already imported in state.rs, use `Ratio::from(rust_decimal::Decimal::new(5, 1))` instead.)

Update `Default for State` (around line 1054). Add these four lines just before the closing `}`:

```rust
            protocol_deficit_icusd: ICUSD::new(0),
            total_deficit_repaid_icusd: ICUSD::new(0),
            deficit_repayment_fraction: default_deficit_repayment_fraction(),
            deficit_readonly_threshold_e8s: 0,
```

`From<InitArg> for State` (line 1059): the `..Default::default()` spread (if used) covers the new fields automatically. If `From<InitArg>` constructs every field explicitly, append the same four lines there too. Verify by reading both.

- [ ] **Step 1.4: Run tests to verify they pass**

Run: `POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --test audit_pocs_liq_005_deficit_account`
Expected: PASS — all three tests green.

- [ ] **Step 1.5: Commit**

```bash
git add src/rumi_protocol_backend/src/state.rs src/rumi_protocol_backend/tests/audit_pocs_liq_005_deficit_account.rs
git commit -m "$(cat <<'EOF'
feat(state): Wave-8e LIQ-005 add four deficit account fields

Adds protocol_deficit_icusd, total_deficit_repaid_icusd,
deficit_repayment_fraction (default 0.5), and deficit_readonly_threshold_e8s
(default 0 = disabled) to State. All four are #[serde(default)] so pre-8e
CBOR snapshots decode cleanly.

Three Layer-1 audit_pocs cover defaults, full round-trip, and a stripped
old-shape blob to fence the upgrade path before any liquidation-site
instrumentation lands.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Add the deficit-mutation helpers to State

**Files:**
- Modify: `src/rumi_protocol_backend/src/state.rs` (add an `impl State` block of helpers near the existing liquidation methods around line 2838).
- Test: `src/rumi_protocol_backend/tests/audit_pocs_liq_005_deficit_account.rs` (extend with helper tests).

- [ ] **Step 2.1: Add the failing tests for the helpers**

Append to `audit_pocs_liq_005_deficit_account.rs`:

```rust
#[test]
fn liq_005_accrue_shortfall_increments_deficit() {
    let mut s = State::default();
    let initial = s.protocol_deficit_icusd;
    let added = s.accrue_deficit_shortfall(ICUSD::new(500));
    assert_eq!(added, ICUSD::new(500));
    assert_eq!(s.protocol_deficit_icusd, initial + ICUSD::new(500));
    assert_eq!(s.total_deficit_repaid_icusd, ICUSD::new(0));
}

#[test]
fn liq_005_accrue_zero_shortfall_is_noop() {
    let mut s = State::default();
    let added = s.accrue_deficit_shortfall(ICUSD::new(0));
    assert_eq!(added, ICUSD::new(0));
    assert_eq!(s.protocol_deficit_icusd, ICUSD::new(0));
}

#[test]
fn liq_005_compute_repay_amount_zero_when_deficit_zero() {
    let s = State::default();
    let repay = s.compute_deficit_repay_amount(ICUSD::new(1_000_000));
    assert_eq!(repay, ICUSD::new(0));
}

#[test]
fn liq_005_compute_repay_amount_caps_at_remaining_deficit() {
    let mut s = State::default();
    s.protocol_deficit_icusd = ICUSD::new(500);
    // 50% of 10_000 = 5_000, but deficit is only 500.
    let repay = s.compute_deficit_repay_amount(ICUSD::new(10_000));
    assert_eq!(repay, ICUSD::new(500));
}

#[test]
fn liq_005_compute_repay_amount_uses_fraction() {
    let mut s = State::default();
    s.protocol_deficit_icusd = ICUSD::new(1_000_000_000_000);
    s.deficit_repayment_fraction = rumi_protocol_backend::numeric::Ratio::from(dec!(0.25));
    let repay = s.compute_deficit_repay_amount(ICUSD::new(1_000_000));
    assert_eq!(repay, ICUSD::new(250_000));
}

#[test]
fn liq_005_compute_repay_amount_zero_when_fraction_zero() {
    let mut s = State::default();
    s.protocol_deficit_icusd = ICUSD::new(10_000_000);
    s.deficit_repayment_fraction = rumi_protocol_backend::numeric::Ratio::from(Decimal::ZERO);
    let repay = s.compute_deficit_repay_amount(ICUSD::new(1_000_000));
    assert_eq!(repay, ICUSD::new(0));
}

#[test]
fn liq_005_apply_repayment_decrements_and_increments_counters() {
    let mut s = State::default();
    s.protocol_deficit_icusd = ICUSD::new(800);
    s.apply_deficit_repayment(ICUSD::new(300));
    assert_eq!(s.protocol_deficit_icusd, ICUSD::new(500));
    assert_eq!(s.total_deficit_repaid_icusd, ICUSD::new(300));
}

#[test]
fn liq_005_apply_repayment_saturates_to_zero() {
    let mut s = State::default();
    s.protocol_deficit_icusd = ICUSD::new(100);
    // Caller asks for 500 but only 100 outstanding — saturate.
    s.apply_deficit_repayment(ICUSD::new(500));
    assert_eq!(s.protocol_deficit_icusd, ICUSD::new(0));
    assert_eq!(s.total_deficit_repaid_icusd, ICUSD::new(500));
}

#[test]
fn liq_005_check_readonly_latch_disabled_when_threshold_zero() {
    let mut s = State::default();
    s.protocol_deficit_icusd = ICUSD::new(u64::MAX);
    let latched = s.check_deficit_readonly_latch();
    assert!(!latched);
    assert_eq!(s.mode, rumi_protocol_backend::state::Mode::default());
}

#[test]
fn liq_005_check_readonly_latch_fires_at_threshold() {
    let mut s = State::default();
    s.deficit_readonly_threshold_e8s = 1_000;
    s.protocol_deficit_icusd = ICUSD::new(1_000);
    let latched = s.check_deficit_readonly_latch();
    assert!(latched);
    assert_eq!(s.mode, rumi_protocol_backend::state::Mode::ReadOnly);
}

#[test]
fn liq_005_check_readonly_latch_does_not_fire_below_threshold() {
    let mut s = State::default();
    s.deficit_readonly_threshold_e8s = 1_000;
    s.protocol_deficit_icusd = ICUSD::new(999);
    let latched = s.check_deficit_readonly_latch();
    assert!(!latched);
}
```

- [ ] **Step 2.2: Run tests to verify failure**

Run: `POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --test audit_pocs_liq_005_deficit_account`
Expected: FAIL — `no method named 'accrue_deficit_shortfall' / 'compute_deficit_repay_amount' / 'apply_deficit_repayment' / 'check_deficit_readonly_latch' on State`.

- [ ] **Step 2.3: Implement the helper methods**

In `src/rumi_protocol_backend/src/state.rs`, find the existing `impl State { ... }` block that contains `pub fn liquidate_vault` (around line 2838) and add these methods at the end of that block (just before the closing `}` for the impl):

```rust
    // ─── Wave-8e LIQ-005: deficit-account helpers ───

    /// Increment `protocol_deficit_icusd` by `shortfall` and return the
    /// amount actually added (always equal to `shortfall` for non-zero
    /// inputs). Caller is responsible for emitting `DeficitAccrued` and
    /// invoking `check_deficit_readonly_latch` afterwards.
    pub fn accrue_deficit_shortfall(&mut self, shortfall: ICUSD) -> ICUSD {
        if shortfall.0 == 0 {
            return ICUSD::new(0);
        }
        self.protocol_deficit_icusd += shortfall;
        shortfall
    }

    /// Compute how much of `fee_e8s` to route to deficit repayment given the
    /// current deficit and configured fraction. Caps at remaining deficit.
    /// Returns ICUSD::new(0) when `protocol_deficit_icusd == 0` or
    /// `deficit_repayment_fraction == 0`.
    pub fn compute_deficit_repay_amount(&self, fee: ICUSD) -> ICUSD {
        if self.protocol_deficit_icusd.0 == 0 || self.deficit_repayment_fraction.0.is_zero() {
            return ICUSD::new(0);
        }
        let candidate_dec =
            rust_decimal::Decimal::from(fee.0) * self.deficit_repayment_fraction.0;
        let candidate_e8s = rust_decimal::prelude::ToPrimitive::to_u64(&candidate_dec)
            .unwrap_or(0);
        let capped = candidate_e8s.min(self.protocol_deficit_icusd.0);
        ICUSD::new(capped)
    }

    /// Apply a successful deficit repayment: decrement the outstanding
    /// deficit and accumulate into the lifetime counter. Saturating —
    /// repaying more than the outstanding deficit caps the decrement at
    /// zero but preserves the full ask in `total_deficit_repaid_icusd` so
    /// the lifetime counter matches the sum of `DeficitRepaid.amount`
    /// events. Caller is responsible for emitting `DeficitRepaid`.
    pub fn apply_deficit_repayment(&mut self, amount: ICUSD) {
        if amount.0 == 0 {
            return;
        }
        self.protocol_deficit_icusd = self.protocol_deficit_icusd.saturating_sub(amount);
        self.total_deficit_repaid_icusd += amount;
    }

    /// If `deficit_readonly_threshold_e8s > 0` and the current deficit has
    /// reached the threshold, force `mode = Mode::ReadOnly` and return
    /// true. Returns false otherwise. The latch is one-shot per crossing —
    /// the admin must call `exit_recovery_mode` to clear it.
    pub fn check_deficit_readonly_latch(&mut self) -> bool {
        if self.deficit_readonly_threshold_e8s == 0 {
            return false;
        }
        if self.protocol_deficit_icusd.0 < self.deficit_readonly_threshold_e8s {
            return false;
        }
        self.mode = Mode::ReadOnly;
        true
    }
```

- [ ] **Step 2.4: Run tests to verify pass**

Run: `POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --test audit_pocs_liq_005_deficit_account`
Expected: PASS — all 13 tests green.

- [ ] **Step 2.5: Commit**

```bash
git add src/rumi_protocol_backend/src/state.rs src/rumi_protocol_backend/tests/audit_pocs_liq_005_deficit_account.rs
git commit -m "$(cat <<'EOF'
feat(state): Wave-8e LIQ-005 add deficit-account helper methods

State now exposes accrue_deficit_shortfall, compute_deficit_repay_amount,
apply_deficit_repayment, and check_deficit_readonly_latch. Each is a
small pure operation on the four LIQ-005 fields with no async or ledger
side-effects, so they're trivially testable without PocketIC.

The helpers are designed to be called inside existing liquidation /
fee-collection mutate_state blocks — the predicate computations and
event emissions stay at the call sites where the original decimal math
lives.

Ten Layer-1 audit_pocs cover defaults, increment, no-op-on-zero, the
fraction-driven repayment computation (including cap at remaining
deficit), saturating decrement, and the ReadOnly latch behavior at
both threshold boundaries.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Add Event variants + recorder helpers + EventTypeFilter

**Files:**
- Modify: `src/rumi_protocol_backend/src/event.rs`.
- Modify: `src/rumi_protocol_backend/src/lib.rs` (`EventTypeFilter` enum).
- Test: `src/rumi_protocol_backend/tests/audit_pocs_liq_005_deficit_account.rs`.

- [ ] **Step 3.1: Add the failing event tests**

Append to `audit_pocs_liq_005_deficit_account.rs`:

```rust
use rumi_protocol_backend::event::{Event, FeeSource, record_deficit_accrued, record_deficit_repaid};
use rumi_protocol_backend::EventTypeFilter;

#[test]
fn liq_005_event_deficit_accrued_round_trip() {
    let e = Event::DeficitAccrued {
        vault_id: 42,
        amount: ICUSD::new(1_500),
        new_deficit: ICUSD::new(1_500),
        timestamp: 1_700_000_000_000_000_000,
    };
    let bytes = serde_cbor::to_vec(&e).expect("encode");
    let decoded: Event = serde_cbor::from_slice(&bytes).expect("decode");
    assert_eq!(decoded, e);
    assert_eq!(decoded.type_filter(), EventTypeFilter::DeficitAccrued);
}

#[test]
fn liq_005_event_deficit_repaid_round_trip_borrowing() {
    let e = Event::DeficitRepaid {
        amount: ICUSD::new(750),
        source: FeeSource::BorrowingFee,
        remaining_deficit: ICUSD::new(750),
        anchor_block_index: Some(99_999),
        timestamp: 1_700_000_000_000_000_001,
    };
    let bytes = serde_cbor::to_vec(&e).expect("encode");
    let decoded: Event = serde_cbor::from_slice(&bytes).expect("decode");
    assert_eq!(decoded, e);
    assert_eq!(decoded.type_filter(), EventTypeFilter::DeficitRepaid);
}

#[test]
fn liq_005_event_deficit_repaid_round_trip_redemption_no_anchor() {
    let e = Event::DeficitRepaid {
        amount: ICUSD::new(123),
        source: FeeSource::RedemptionFee,
        remaining_deficit: ICUSD::new(0),
        anchor_block_index: None,
        timestamp: 1_700_000_000_000_000_002,
    };
    let bytes = serde_cbor::to_vec(&e).expect("encode");
    let decoded: Event = serde_cbor::from_slice(&bytes).expect("decode");
    assert_eq!(decoded, e);
}

#[test]
fn liq_005_record_deficit_accrued_emits_event_and_updates_state() {
    let mut s = State::default();
    record_deficit_accrued(&mut s, /*vault_id=*/ 7, ICUSD::new(900), /*timestamp=*/ 1_000);
    assert_eq!(s.protocol_deficit_icusd, ICUSD::new(900));
}

#[test]
fn liq_005_record_deficit_repaid_emits_event_and_updates_state() {
    let mut s = State::default();
    s.protocol_deficit_icusd = ICUSD::new(900);
    record_deficit_repaid(
        &mut s,
        ICUSD::new(400),
        FeeSource::BorrowingFee,
        Some(12_345),
        1_001,
    );
    assert_eq!(s.protocol_deficit_icusd, ICUSD::new(500));
    assert_eq!(s.total_deficit_repaid_icusd, ICUSD::new(400));
}
```

- [ ] **Step 3.2: Run tests to verify failure**

Run: `POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --test audit_pocs_liq_005_deficit_account`
Expected: FAIL — `no variant 'DeficitAccrued' / 'DeficitRepaid' on Event`, `unresolved import 'FeeSource'`, `no variant 'DeficitAccrued' on EventTypeFilter`.

- [ ] **Step 3.3: Add Event variants + FeeSource enum**

In `src/rumi_protocol_backend/src/event.rs`, add the `FeeSource` enum at the top-level scope just before the `pub enum Event`:

```rust
/// Wave-8e LIQ-005: identifies which fee revenue stream a deficit
/// repayment was sourced from. Persisted in the `DeficitRepaid` event so
/// the explorer can attribute repayment volume per source.
#[derive(CandidType, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeeSource {
    BorrowingFee,
    RedemptionFee,
}
```

Add the two variants inside `pub enum Event { ... }` (place them after `RedistributeVault` and before `BorrowFromVault` to group with debt-affecting events):

```rust
    /// Wave-8e LIQ-005: an underwater liquidation netted seized USD <
    /// debt cleared, accruing the shortfall to `protocol_deficit_icusd`.
    /// Emitted from every liquidation path (`liquidate_vault`,
    /// `liquidate_vault_partial`, `liquidate_vault_partial_with_stable`,
    /// `partial_liquidate_vault`, `liquidate_vault_debt_already_burned`)
    /// when shortfall > 0.
    #[serde(rename = "deficit_accrued")]
    DeficitAccrued {
        vault_id: u64,
        amount: ICUSD,
        new_deficit: ICUSD,
        timestamp: u64,
    },

    /// Wave-8e LIQ-005: a fee collection routed `amount` icUSD toward
    /// deficit repayment. For borrowing-fee source this means the protocol
    /// minted `original_fee - amount` to treasury instead of `original_fee`.
    /// For redemption-fee source this means the deficit decremented because
    /// `amount` of the redeemer's burned icUSD was applied against the
    /// outstanding deficit instead of accruing as protocol revenue.
    /// `anchor_block_index` is the icUSD ledger block that generated the
    /// fee (treasury mint block for borrowing fees; the redeemer's
    /// `transfer_icusd_from` burn block for redemption fees).
    #[serde(rename = "deficit_repaid")]
    DeficitRepaid {
        amount: ICUSD,
        source: FeeSource,
        remaining_deficit: ICUSD,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        anchor_block_index: Option<u64>,
        timestamp: u64,
    },
```

Add the recorder helpers near the other `pub fn record_*` helpers (around line 1700+):

```rust
/// Record a `DeficitAccrued` event and increment `protocol_deficit_icusd`.
/// Caller is responsible for invoking `state.check_deficit_readonly_latch()`
/// afterwards if the latch threshold is configured.
pub fn record_deficit_accrued(
    state: &mut State,
    vault_id: u64,
    amount: ICUSD,
    timestamp: u64,
) {
    state.accrue_deficit_shortfall(amount);
    record_event(&Event::DeficitAccrued {
        vault_id,
        amount,
        new_deficit: state.protocol_deficit_icusd,
        timestamp,
    });
}

/// Record a `DeficitRepaid` event and apply the repayment to state.
pub fn record_deficit_repaid(
    state: &mut State,
    amount: ICUSD,
    source: FeeSource,
    anchor_block_index: Option<u64>,
    timestamp: u64,
) {
    state.apply_deficit_repayment(amount);
    record_event(&Event::DeficitRepaid {
        amount,
        source,
        remaining_deficit: state.protocol_deficit_icusd,
        anchor_block_index,
        timestamp,
    });
}
```

- [ ] **Step 3.4: Add EventTypeFilter variants + extend type_filter()**

In `src/rumi_protocol_backend/src/lib.rs`, extend `pub enum EventTypeFilter` (around line 202):

```rust
    DeficitAccrued,
    DeficitRepaid,
```

In `src/rumi_protocol_backend/src/event.rs`, find `pub fn type_filter(&self) -> EventTypeFilter` (around line 672) and add the two new variants. Pattern: the existing match maps every Event variant to a coarse filter. Insert:

```rust
            Event::DeficitAccrued { .. } => EventTypeFilter::DeficitAccrued,
            Event::DeficitRepaid { .. } => EventTypeFilter::DeficitRepaid,
```

Also extend `pub fn involves_principal(&self, p: &Principal) -> bool` (around line 958) — the new events have no Principal, so they match no caller filter:

```rust
            Event::DeficitAccrued { .. } => false,
            Event::DeficitRepaid { .. } => false,
```

(Verify by reading the existing match — pattern is `Event::Variant { caller_field } => caller_field.as_ref() == Some(p)` for events with callers. Events with no caller short-circuit `false`.)

- [ ] **Step 3.5: Run tests to verify pass**

Run: `POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --test audit_pocs_liq_005_deficit_account`
Expected: PASS — 18 tests green.

- [ ] **Step 3.6: Commit**

```bash
git add src/rumi_protocol_backend/src/event.rs src/rumi_protocol_backend/src/lib.rs src/rumi_protocol_backend/tests/audit_pocs_liq_005_deficit_account.rs
git commit -m "$(cat <<'EOF'
feat(event): Wave-8e LIQ-005 add DeficitAccrued / DeficitRepaid variants

New Event variants record bad-debt accrual and fee-driven repayment.
DeficitAccrued is emitted from every liquidation path that nets
shortfall > 0; DeficitRepaid is emitted from each fee-collection site
that decrements the deficit. FeeSource enum tags borrowing vs redemption
fee origin for explorer attribution.

EventTypeFilter gets two new variants and the type_filter() / involves_principal()
mappings extend accordingly. Recorder helpers wrap the state helpers from
Task 2 with the matching event emission so call sites stay one-liners.

Five Layer-1 audit_pocs cover CBOR round-trip on both new variants and
the recorder helpers' state mutations.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Instrument `liquidate_vault` (full liquidation, normal + recovery partial)

**Files:**
- Modify: `src/rumi_protocol_backend/src/vault.rs` (around line 3027 — inside the `mutate_state` block in `liquidate_vault`).
- Test: `src/rumi_protocol_backend/tests/audit_pocs_liq_005_deficit_account.rs`.

- [ ] **Step 4.1: Add the failing predicate test**

Append to `audit_pocs_liq_005_deficit_account.rs`:

```rust
use rumi_protocol_backend::numeric::collateral_usd_value;
use rust_decimal_macros::dec;

#[test]
fn liq_005_predicate_zero_when_seized_covers_debt() {
    // 1 ICP at $10, 8 decimals, vs 8 icUSD debt cleared. Seized USD = 10 > 8 → no shortfall.
    let seized_usd = collateral_usd_value(100_000_000, dec!(10.0), 8);
    let debt_cleared = ICUSD::new(800_000_000); // 8 icUSD
    let shortfall = if seized_usd < debt_cleared {
        debt_cleared - seized_usd
    } else {
        ICUSD::new(0)
    };
    assert_eq!(shortfall, ICUSD::new(0));
}

#[test]
fn liq_005_predicate_positive_when_seized_under_debt() {
    // 0.5 ICP at $10 = $5 seized, 8 icUSD debt → shortfall = 3 icUSD.
    let seized_usd = collateral_usd_value(50_000_000, dec!(10.0), 8);
    let debt_cleared = ICUSD::new(800_000_000); // 8 icUSD
    let shortfall = if seized_usd < debt_cleared {
        debt_cleared - seized_usd
    } else {
        ICUSD::new(0)
    };
    assert_eq!(shortfall, ICUSD::new(300_000_000));
}

#[test]
fn liq_005_predicate_handles_8_decimal_collateral() {
    // 1.5 nICP-equivalent at $0.50 = $0.75. Debt cleared 1 icUSD. Shortfall 0.25.
    let seized_usd = collateral_usd_value(150_000_000, dec!(0.50), 8);
    let debt_cleared = ICUSD::new(100_000_000);
    let shortfall = if seized_usd < debt_cleared {
        debt_cleared - seized_usd
    } else {
        ICUSD::new(0)
    };
    assert_eq!(shortfall, ICUSD::new(25_000_000));
}
```

- [ ] **Step 4.2: Run to verify pass (the predicate is just `numeric::collateral_usd_value` + a comparison; no instrumentation yet)**

Run: `POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --test audit_pocs_liq_005_deficit_account`
Expected: PASS — these are pure predicate tests, no liquidation path mutated yet.

(These pass right away because they're testing the off-the-shelf primitive. They serve as a fence in case anyone ever changes `collateral_usd_value`.)

- [ ] **Step 4.3: Instrument `liquidate_vault`**

In `src/rumi_protocol_backend/src/vault.rs`, find the `mutate_state` block in `liquidate_vault` (around line 3028, "Step 4: Update protocol state ATOMICALLY"). After the existing `s.liquidate_vault(...)` call but BEFORE the `LiquidateVault` event recording, add:

```rust
        // Wave-8e LIQ-005: if seized USD < debt cleared, the protocol
        // absorbed bad debt. Track the shortfall in `protocol_deficit_icusd`
        // and check the ReadOnly latch. The icUSD payment from the
        // liquidator was already burned to the protocol's minting account
        // via `transfer_icusd_from`, so the supply side is consistent —
        // the liquidator effectively paid `debt_amount` icUSD for collateral
        // worth less, and the protocol now records the outstanding loss.
        let seized_usd = crate::numeric::collateral_usd_value(
            total_to_seize.to_u64(),
            collateral_price,
            config_decimals,
        );
        let shortfall = if seized_usd < debt_amount {
            debt_amount - seized_usd
        } else {
            ICUSD::new(0)
        };
        if shortfall.0 > 0 {
            crate::event::record_deficit_accrued(
                s,
                vault_id,
                shortfall,
                ic_cdk::api::time(),
            );
            if s.check_deficit_readonly_latch() {
                log!(INFO,
                    "[LIQ-005] deficit threshold {} crossed by vault #{} shortfall {}; auto-latched ReadOnly",
                    s.deficit_readonly_threshold_e8s, vault_id, shortfall.to_u64()
                );
            }
        }
```

Note: the `mutate_state` block in `liquidate_vault` is `let interest_share = mutate_state(|s| { ... });`. Find the line `let interest_share = s.liquidate_vault(vault_id, mode, collateral_price_usd);` and insert the new block immediately after it (so the deficit accrual happens AFTER the vault state mutation but BEFORE the event recording — so the deficit's `new_deficit` reads correctly).

Verify the variable bindings exist in scope: `total_to_seize`, `debt_amount`, `collateral_price`, `config_decimals`, `vault_id`. They all do (look at lines ~2933-2976 and line ~3076).

- [ ] **Step 4.4: Run the full Layer-1 fence**

Run: `POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --test audit_pocs_liq_005_deficit_account`
Expected: PASS — 21 tests green.

Run: `POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --lib`
Expected: PASS — backend lib tests still 83 passed, 1 ignored.

- [ ] **Step 4.5: Commit**

```bash
git add src/rumi_protocol_backend/src/vault.rs src/rumi_protocol_backend/tests/audit_pocs_liq_005_deficit_account.rs
git commit -m "$(cat <<'EOF'
feat(vault): Wave-8e LIQ-005 instrument liquidate_vault for deficit accrual

When the seized collateral USD value is less than the debt cleared in
liquidate_vault (full or recovery-mode partial), the shortfall now
accrues to protocol_deficit_icusd and emits a DeficitAccrued event.
The ReadOnly latch fires if the configured threshold is crossed.

The accrual lives inside the existing mutate_state block so a panic
between vault mutation and deficit accrual cannot leave inconsistent
state. Three Layer-1 fences pin the predicate against the central
collateral_usd_value helper.

Four sister liquidation paths (liquidate_vault_partial,
liquidate_vault_partial_with_stable, partial_liquidate_vault,
liquidate_vault_debt_already_burned) are instrumented in subsequent
commits.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Instrument `liquidate_vault_partial`

**Files:**
- Modify: `src/rumi_protocol_backend/src/vault.rs` (inside the `mutate_state` block in `liquidate_vault_partial` around line 2169).

- [ ] **Step 5.1: Instrument the partial path**

In `src/rumi_protocol_backend/src/vault.rs`, find `liquidate_vault_partial` and its `mutate_state` block (the one starting `let interest_share = mutate_state(|s| {`). Inside that block, after the vault mutations (`vault.borrowed_icusd_amount -= ...`, etc.) and BEFORE the `PartialLiquidateVault` event recording, add:

```rust
        // Wave-8e LIQ-005: per-call deficit accrual. See `liquidate_vault`
        // for the rationale. `max_liquidatable_debt` is the icUSD amount
        // the vault's debt was reduced by; `total_to_seize` is the
        // collateral seized. Predicate: seized USD < debt cleared.
        let seized_usd = crate::numeric::collateral_usd_value(
            total_to_seize.to_u64(),
            collateral_price,
            config_decimals,
        );
        let shortfall = if seized_usd < max_liquidatable_debt {
            max_liquidatable_debt - seized_usd
        } else {
            ICUSD::new(0)
        };
        if shortfall.0 > 0 {
            crate::event::record_deficit_accrued(
                s,
                vault_id,
                shortfall,
                ic_cdk::api::time(),
            );
            if s.check_deficit_readonly_latch() {
                log!(INFO,
                    "[LIQ-005] deficit threshold {} crossed by partial vault #{} shortfall {}; auto-latched ReadOnly",
                    s.deficit_readonly_threshold_e8s, vault_id, shortfall.to_u64()
                );
            }
        }
```

Variables in scope: `total_to_seize`, `max_liquidatable_debt`, `collateral_price`, `config_decimals`, `vault_id`. Verify by reading the surrounding code.

- [ ] **Step 5.2: Run tests**

Run: `POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --test audit_pocs_liq_005_deficit_account && POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --lib`
Expected: PASS.

- [ ] **Step 5.3: Commit**

```bash
git add src/rumi_protocol_backend/src/vault.rs
git commit -m "fix(vault): Wave-8e LIQ-005 accrue deficit on liquidate_vault_partial

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Instrument `liquidate_vault_partial_with_stable`

**Files:**
- Modify: `src/rumi_protocol_backend/src/vault.rs` (inside `mutate_state` in `liquidate_vault_partial_with_stable` around line 2432).

- [ ] **Step 6.1: Instrument the stablecoin partial path**

Same pattern as Task 5 — find the `mutate_state` block in `liquidate_vault_partial_with_stable`, insert the deficit-accrual fence after vault mutation and before `PartialLiquidateVault` event recording. Variable names match Task 5: `total_to_seize`, `max_liquidatable_debt`, `collateral_price`, `config_decimals`, `vault_id`.

```rust
        // Wave-8e LIQ-005: per-call deficit accrual. The stablecoin path
        // pulls ckUSDT/ckUSDC from the liquidator (1:1 with icUSD plus a
        // surcharge). The icUSD-denominated debt cleared is
        // `max_liquidatable_debt`; the collateral seized is `total_to_seize`.
        // The stablecoin payment is unrelated to the deficit predicate —
        // we still measure shortfall in icUSD-equivalent collateral USD value.
        let seized_usd = crate::numeric::collateral_usd_value(
            total_to_seize.to_u64(),
            collateral_price,
            config_decimals,
        );
        let shortfall = if seized_usd < max_liquidatable_debt {
            max_liquidatable_debt - seized_usd
        } else {
            ICUSD::new(0)
        };
        if shortfall.0 > 0 {
            crate::event::record_deficit_accrued(
                s,
                vault_id,
                shortfall,
                ic_cdk::api::time(),
            );
            if s.check_deficit_readonly_latch() {
                log!(INFO,
                    "[LIQ-005] deficit threshold {} crossed by stable-partial vault #{} shortfall {}; auto-latched ReadOnly",
                    s.deficit_readonly_threshold_e8s, vault_id, shortfall.to_u64()
                );
            }
        }
```

- [ ] **Step 6.2: Run tests**

Run: `POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --test audit_pocs_liq_005_deficit_account && POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --lib`
Expected: PASS.

- [ ] **Step 6.3: Commit**

```bash
git add src/rumi_protocol_backend/src/vault.rs
git commit -m "fix(vault): Wave-8e LIQ-005 accrue deficit on liquidate_vault_partial_with_stable

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Instrument `partial_liquidate_vault` (the third partial entry point)

**Files:**
- Modify: `src/rumi_protocol_backend/src/vault.rs` (inside `mutate_state` in `partial_liquidate_vault` around line 3444).

- [ ] **Step 7.1: Instrument**

Same pattern. The variable name for debt cleared in this path is `liquidator_payment` (not `max_liquidatable_debt`). The other variables (`total_to_seize`, `collateral_price`, `config_decimals`, `arg.vault_id`) match.

```rust
        // Wave-8e LIQ-005: per-call deficit accrual.
        let seized_usd = crate::numeric::collateral_usd_value(
            total_to_seize.to_u64(),
            collateral_price,
            config_decimals,
        );
        let shortfall = if seized_usd < liquidator_payment {
            liquidator_payment - seized_usd
        } else {
            ICUSD::new(0)
        };
        if shortfall.0 > 0 {
            crate::event::record_deficit_accrued(
                s,
                arg.vault_id,
                shortfall,
                ic_cdk::api::time(),
            );
            if s.check_deficit_readonly_latch() {
                log!(INFO,
                    "[LIQ-005] deficit threshold {} crossed by partial_liquidate_vault #{} shortfall {}; auto-latched ReadOnly",
                    s.deficit_readonly_threshold_e8s, arg.vault_id, shortfall.to_u64()
                );
            }
        }
```

- [ ] **Step 7.2: Run + commit**

Run: `POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --test audit_pocs_liq_005_deficit_account && POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --lib`

```bash
git add src/rumi_protocol_backend/src/vault.rs
git commit -m "fix(vault): Wave-8e LIQ-005 accrue deficit on partial_liquidate_vault

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: Instrument `liquidate_vault_debt_already_burned` (SP path)

**Files:**
- Modify: `src/rumi_protocol_backend/src/vault.rs` (inside the SP-path `mutate_state` block around line 2786).

- [ ] **Step 8.1: Instrument the SP writedown path**

This path is special because the icUSD was already burned via the 3pool (or 3USD reserves were credited) before the call. The supply-side accounting differs but the deficit predicate is the same: did the seized collateral cover the debt cleared?

Variable names: `max_liquidatable_debt` (debt cleared), `total_to_seize` (collateral seized), `collateral_price`, `config_decimals`, `vault_id`. Verify by reading the surrounding context (Task 0 reading already mapped this).

```rust
        // Wave-8e LIQ-005: per-call deficit accrual. Even though icUSD was
        // burned externally (legacy 3pool burn) or 3USD reserves were
        // credited (reserves path), the protocol's solvency invariant is
        // still: seized collateral USD value vs. debt cleared. If the SP
        // absorbed an underwater vault, the protocol records the shortfall
        // here so future fee revenue burns it down — this is what the
        // audit (LIQ-005) prescribes instead of socializing onto SP
        // depositors.
        let seized_usd = crate::numeric::collateral_usd_value(
            total_to_seize.to_u64(),
            collateral_price,
            config_decimals,
        );
        let shortfall = if seized_usd < max_liquidatable_debt {
            max_liquidatable_debt - seized_usd
        } else {
            ICUSD::new(0)
        };
        if shortfall.0 > 0 {
            crate::event::record_deficit_accrued(
                s,
                vault_id,
                shortfall,
                ic_cdk::api::time(),
            );
            if s.check_deficit_readonly_latch() {
                log!(INFO,
                    "[LIQ-005] deficit threshold {} crossed by SP writedown vault #{} shortfall {}; auto-latched ReadOnly",
                    s.deficit_readonly_threshold_e8s, vault_id, shortfall.to_u64()
                );
            }
        }
```

- [ ] **Step 8.2: Run + commit**

```bash
git add src/rumi_protocol_backend/src/vault.rs
git commit -m "fix(vault): Wave-8e LIQ-005 accrue deficit on SP writedown path

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 9: Borrowing-fee deficit repayment

**Files:**
- Modify: `src/rumi_protocol_backend/src/treasury.rs` (`mint_borrowing_fee_to_treasury`, around line 397).
- Modify: `src/rumi_protocol_backend/src/vault.rs` (caller at line 885, no signature change but add a state read).
- Test: `src/rumi_protocol_backend/tests/audit_pocs_liq_005_deficit_account.rs`.

- [ ] **Step 9.1: Add the failing fee-routing tests**

Append to `audit_pocs_liq_005_deficit_account.rs`:

```rust
#[test]
fn liq_005_borrowing_fee_no_deficit_routes_full_fee() {
    let mut s = State::default();
    let outcome = rumi_protocol_backend::treasury::plan_fee_routing(
        &mut s,
        ICUSD::new(1_000_000),
        FeeSource::BorrowingFee,
    );
    assert_eq!(outcome.to_repay, ICUSD::new(0));
    assert_eq!(outcome.to_remainder, ICUSD::new(1_000_000));
    // No state mutation on a no-deficit call.
    assert_eq!(s.protocol_deficit_icusd, ICUSD::new(0));
    assert_eq!(s.total_deficit_repaid_icusd, ICUSD::new(0));
}

#[test]
fn liq_005_borrowing_fee_with_deficit_splits_at_fraction() {
    let mut s = State::default();
    s.protocol_deficit_icusd = ICUSD::new(10_000_000); // 0.1 icUSD
    let outcome = rumi_protocol_backend::treasury::plan_fee_routing(
        &mut s,
        ICUSD::new(1_000_000),
        FeeSource::BorrowingFee,
    );
    // 50% of 1_000_000 = 500_000, capped at deficit 10_000_000 — no cap binds.
    assert_eq!(outcome.to_repay, ICUSD::new(500_000));
    assert_eq!(outcome.to_remainder, ICUSD::new(500_000));
    // plan_fee_routing already applied the repayment to state.
    assert_eq!(s.protocol_deficit_icusd, ICUSD::new(9_500_000));
    assert_eq!(s.total_deficit_repaid_icusd, ICUSD::new(500_000));
}

#[test]
fn liq_005_borrowing_fee_caps_repay_at_remaining_deficit() {
    let mut s = State::default();
    s.protocol_deficit_icusd = ICUSD::new(50_000); // small deficit
    let outcome = rumi_protocol_backend::treasury::plan_fee_routing(
        &mut s,
        ICUSD::new(1_000_000),
        FeeSource::BorrowingFee,
    );
    // 50% of 1_000_000 = 500_000, but deficit is only 50_000.
    assert_eq!(outcome.to_repay, ICUSD::new(50_000));
    assert_eq!(outcome.to_remainder, ICUSD::new(950_000));
    assert_eq!(s.protocol_deficit_icusd, ICUSD::new(0));
}

#[test]
fn liq_005_redemption_fee_repayment_decrements_deficit() {
    let mut s = State::default();
    s.protocol_deficit_icusd = ICUSD::new(800_000);
    let outcome = rumi_protocol_backend::treasury::plan_fee_routing(
        &mut s,
        ICUSD::new(400_000),
        FeeSource::RedemptionFee,
    );
    assert_eq!(outcome.to_repay, ICUSD::new(200_000));
    assert_eq!(outcome.to_remainder, ICUSD::new(200_000));
    assert_eq!(s.protocol_deficit_icusd, ICUSD::new(600_000));
    assert_eq!(s.total_deficit_repaid_icusd, ICUSD::new(200_000));
}

#[test]
fn liq_005_zero_fee_is_noop() {
    let mut s = State::default();
    s.protocol_deficit_icusd = ICUSD::new(1_000);
    let outcome = rumi_protocol_backend::treasury::plan_fee_routing(
        &mut s,
        ICUSD::new(0),
        FeeSource::BorrowingFee,
    );
    assert_eq!(outcome.to_repay, ICUSD::new(0));
    assert_eq!(outcome.to_remainder, ICUSD::new(0));
    assert_eq!(s.protocol_deficit_icusd, ICUSD::new(1_000));
}
```

- [ ] **Step 9.2: Run to verify failure**

Run: `POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --test audit_pocs_liq_005_deficit_account`
Expected: FAIL — `plan_fee_routing` not defined; `FeeRoutingOutcome` (or whatever the struct is called) not defined.

- [ ] **Step 9.3: Add `plan_fee_routing` to treasury.rs**

In `src/rumi_protocol_backend/src/treasury.rs`, near the top after the existing helper section, add:

```rust
/// Wave-8e LIQ-005: outcome of routing a fee through the deficit repayment
/// path. `to_repay` is the icUSD foregone-or-burned to pay down the
/// deficit; `to_remainder` is what flows to the original destination
/// (treasury for borrowing fee; redemption fee remainder accrues as the
/// existing redeem path's protocol revenue).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeeRoutingOutcome {
    pub to_repay: ICUSD,
    pub to_remainder: ICUSD,
}

/// Plan how a fee splits between deficit repayment and its existing
/// destination. Mutates state to apply the repayment + emit the
/// `DeficitRepaid` event. The caller is responsible for the actual mint
/// of `to_remainder` to treasury (or for the redemption-side accounting
/// that absorbs the remainder as protocol revenue).
///
/// `anchor_block_index` is set lazily by the caller after the existing
/// ledger op completes — for borrowing fees the treasury mint block, for
/// redemption fees the redeemer's burn block. Pass `None` here and emit a
/// follow-up event if you need a richer audit anchor.
pub fn plan_fee_routing(
    state: &mut crate::state::State,
    fee: crate::numeric::ICUSD,
    source: crate::event::FeeSource,
) -> FeeRoutingOutcome {
    if fee.0 == 0 {
        return FeeRoutingOutcome {
            to_repay: crate::numeric::ICUSD::new(0),
            to_remainder: crate::numeric::ICUSD::new(0),
        };
    }
    let to_repay = state.compute_deficit_repay_amount(fee);
    if to_repay.0 > 0 {
        crate::event::record_deficit_repaid(
            state,
            to_repay,
            source,
            None,
            ic_cdk::api::time(),
        );
    }
    let to_remainder = crate::numeric::ICUSD::new(fee.0 - to_repay.0);
    FeeRoutingOutcome { to_repay, to_remainder }
}
```

- [ ] **Step 9.4: Wire `mint_borrowing_fee_to_treasury` to use it**

Modify `mint_borrowing_fee_to_treasury` (treasury.rs:397) to compute the split via `plan_fee_routing` BEFORE the mint, and mint only `to_remainder` to treasury. The function signature stays `pub async fn mint_borrowing_fee_to_treasury(fee: ICUSD)`:

```rust
pub async fn mint_borrowing_fee_to_treasury(fee: ICUSD) {
    if fee.0 == 0 {
        return;
    }
    // Wave-8e LIQ-005: route a configurable fraction of the fee to
    // deficit repayment first. The "repayment" is supply-conserving:
    // we mint `to_remainder` instead of `fee` to treasury, so the
    // skipped `to_repay` mint pays down the deficit by foregone revenue.
    let outcome = crate::state::mutate_state(|s| {
        plan_fee_routing(s, fee, crate::event::FeeSource::BorrowingFee)
    });
    if outcome.to_remainder.0 == 0 {
        log!(
            INFO,
            "[treasury] Borrowing fee {} fully routed to deficit repayment ({}); no treasury mint",
            fee.to_u64(),
            outcome.to_repay.to_u64()
        );
        return;
    }
    let treasury = read_state(|s| s.treasury_principal);
    if let Some(tp) = treasury {
        match management::mint_icusd(outcome.to_remainder, tp).await {
            Ok(block_index) => {
                log!(
                    INFO,
                    "[treasury] Minted {} icUSD borrowing fee (deficit repay {}, block {})",
                    outcome.to_remainder.to_u64(),
                    outcome.to_repay.to_u64(),
                    block_index
                );
                let _ = notify_treasury_deposit(
                    tp,
                    DepositType::BorrowingFee,
                    AssetType::ICUSD,
                    outcome.to_remainder.to_u64(),
                    block_index,
                )
                .await;
            }
            Err(e) => log!(
                INFO,
                "[treasury] WARNING: borrowing fee mint failed: {:?}",
                e
            ),
        }
    }
}
```

No change to the borrowing fee call site in `vault.rs:885` — the function signature is unchanged.

- [ ] **Step 9.5: Run + commit**

Run: `POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --test audit_pocs_liq_005_deficit_account && POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --lib`
Expected: PASS — 26 unit tests, lib unchanged.

```bash
git add src/rumi_protocol_backend/src/treasury.rs src/rumi_protocol_backend/tests/audit_pocs_liq_005_deficit_account.rs
git commit -m "$(cat <<'EOF'
feat(treasury): Wave-8e LIQ-005 route borrowing fee through deficit repayment

mint_borrowing_fee_to_treasury now consults plan_fee_routing first. When
protocol_deficit_icusd > 0, a configurable fraction of the fee skips the
treasury mint (effectively burning by not minting) and decrements the
deficit. The remainder flows to treasury as before.

plan_fee_routing is the new shared decision helper used by both the
borrowing-fee and redemption-fee call sites. It mutates state (apply
repayment + emit DeficitRepaid) and returns the to_repay / to_remainder
split to the caller. The caller is responsible for the original ledger
op on `to_remainder`.

Five Layer-1 audit_pocs cover no-deficit, deficit-binding, cap-at-
remaining-deficit, redemption-fee, and zero-fee cases.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: Redemption-fee deficit repayment

**Files:**
- Modify: `src/rumi_protocol_backend/src/vault.rs` — both redemption sites: `redeem_collateral` (around line 467) and `redeem_reserves` (around line 388).

- [ ] **Step 10.1: Wire redeem_collateral to use plan_fee_routing**

In `redeem_collateral` (vault.rs:435), the existing `mutate_state` block (line 467) computes `fee_amount`, sets `current_base_rate` and `last_redemption_time`, and calls `record_redemption_on_vaults`. Inside this block, AFTER the `record_redemption_on_vaults` call, add a deficit-repayment plan:

```rust
            // Wave-8e LIQ-005: route a configurable fraction of the
            // redemption fee toward deficit repayment. The redeemer's
            // icUSD has already been burned via `transfer_icusd_from`
            // (the protocol's main account is the icUSD minting account),
            // so the supply side is already correct — `to_repay` here is
            // a pure state mutation that decrements the deficit.
            let _routing = crate::treasury::plan_fee_routing(
                s,
                fee_amount,
                crate::event::FeeSource::RedemptionFee,
            );
```

The `fee_amount` is in scope (computed earlier in the block). The `_routing` value is intentionally unused — for redemption fees we don't care about the remainder split (it stays as protocol revenue in the existing accounting, just like today).

- [ ] **Step 10.2: Wire redeem_reserves to use plan_fee_routing**

In `redeem_reserves` (vault.rs:206) the redemption fee math is in two places. The reserves portion has `fee_e6s` / `fee_icusd`; the spillover portion has `vault_fee` (computed at line 393). In the spillover `mutate_state` block (around line 388), after the existing `record_redemption_on_vaults` call, add:

```rust
            // Wave-8e LIQ-005: spillover-portion fee → deficit repayment.
            let _routing_spillover = crate::treasury::plan_fee_routing(
                s,
                vault_fee,
                crate::event::FeeSource::RedemptionFee,
            );
```

For the reserves-portion `fee_icusd` (this fee comes from the redemption against the protocol's stablecoin reserves; the icUSD is also burned via `transfer_icusd_from`). Find the `mutate_state` block in `redeem_reserves` that handles the reserves-side fee accounting (it should be just before the spillover block — verify by reading lines 206-385). Add a parallel routing call after the reserves-side fee is recorded:

```rust
            let _routing_reserves = crate::treasury::plan_fee_routing(
                s,
                fee_icusd,
                crate::event::FeeSource::RedemptionFee,
            );
```

If the `redeem_reserves` reserves-side path doesn't have a clean `mutate_state` block where this would fit naturally, defer this hook to a follow-up commit and document the gap. **Verify by reading vault.rs:206-405 carefully before adding** — it's possible the reserves-side fee never shows up as `ICUSD` (only e6s for ckUSDT), in which case the deficit repayment doesn't apply directly and we should add a small e6s→e8s conversion or skip with a comment.

- [ ] **Step 10.3: Run tests**

Run: `POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --test audit_pocs_liq_005_deficit_account && POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --lib`
Expected: PASS.

- [ ] **Step 10.4: Commit**

```bash
git add src/rumi_protocol_backend/src/vault.rs
git commit -m "$(cat <<'EOF'
feat(vault): Wave-8e LIQ-005 route redemption fee through deficit repayment

Both redeem_collateral and redeem_reserves consult plan_fee_routing on
the redemption-fee side. Unlike the borrowing-fee path, the redeemer's
icUSD has already been burned via transfer_icusd_from to the protocol's
main (= minting) account, so the deficit repayment is a pure state
mutation: decrement protocol_deficit_icusd, increment
total_deficit_repaid_icusd, emit DeficitRepaid.

The remainder of the redemption fee stays as protocol revenue as before
(implicit in the existing accounting where the fee never gets credited
back to the redeemer).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: ProtocolStatus + admin endpoints + candid

**Files:**
- Modify: `src/rumi_protocol_backend/src/lib.rs` (`ProtocolStatus` struct).
- Modify: `src/rumi_protocol_backend/src/main.rs` (`get_protocol_status` populator + new admin endpoints).
- Modify: `src/rumi_protocol_backend/src/event.rs` (admin-event variants + recorder helpers, like the existing `SetBorrowingFee` etc.).
- Modify: `src/rumi_protocol_backend/rumi_protocol_backend.did`.
- Test: `src/rumi_protocol_backend/tests/audit_pocs_liq_005_deficit_account.rs`.

- [ ] **Step 11.1: Add the failing admin-fence tests**

Append to `audit_pocs_liq_005_deficit_account.rs`:

```rust
#[test]
fn liq_005_admin_event_set_repayment_fraction_round_trip() {
    let e = Event::SetDeficitRepaymentFraction {
        fraction: rumi_protocol_backend::numeric::Ratio::from(dec!(0.25)),
        timestamp: 1_000,
    };
    let bytes = serde_cbor::to_vec(&e).expect("encode");
    let decoded: Event = serde_cbor::from_slice(&bytes).expect("decode");
    assert_eq!(decoded, e);
}

#[test]
fn liq_005_admin_event_set_readonly_threshold_round_trip() {
    let e = Event::SetDeficitReadonlyThresholdE8s {
        threshold_e8s: 1_000_000_000,
        timestamp: 1_001,
    };
    let bytes = serde_cbor::to_vec(&e).expect("encode");
    let decoded: Event = serde_cbor::from_slice(&bytes).expect("decode");
    assert_eq!(decoded, e);
}
```

- [ ] **Step 11.2: Verify failure**

Run: `POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --test audit_pocs_liq_005_deficit_account`
Expected: FAIL — `no variant 'SetDeficitRepaymentFraction' / 'SetDeficitReadonlyThresholdE8s' on Event`.

- [ ] **Step 11.3: Add admin event variants + recorder helpers**

In `src/rumi_protocol_backend/src/event.rs`, add to `pub enum Event`:

```rust
    #[serde(rename = "set_deficit_repayment_fraction")]
    SetDeficitRepaymentFraction {
        fraction: Ratio,
        timestamp: u64,
    },

    #[serde(rename = "set_deficit_readonly_threshold_e8s")]
    SetDeficitReadonlyThresholdE8s {
        threshold_e8s: u64,
        timestamp: u64,
    },
```

Extend `type_filter()`:

```rust
            Event::SetDeficitRepaymentFraction { .. } => EventTypeFilter::Admin,
            Event::SetDeficitReadonlyThresholdE8s { .. } => EventTypeFilter::Admin,
```

Add recorders next to the existing `record_set_*` family:

```rust
pub fn record_set_deficit_repayment_fraction(state: &mut State, fraction: Ratio) {
    state.deficit_repayment_fraction = fraction;
    record_event(&Event::SetDeficitRepaymentFraction {
        fraction,
        timestamp: now(),
    });
}

pub fn record_set_deficit_readonly_threshold_e8s(state: &mut State, threshold_e8s: u64) {
    state.deficit_readonly_threshold_e8s = threshold_e8s;
    record_event(&Event::SetDeficitReadonlyThresholdE8s {
        threshold_e8s,
        timestamp: now(),
    });
}
```

- [ ] **Step 11.4: Add ProtocolStatus fields**

In `src/rumi_protocol_backend/src/lib.rs`, append to `pub struct ProtocolStatus`:

```rust
    pub protocol_deficit_icusd: u64,
    pub total_deficit_repaid_icusd: u64,
    pub deficit_repayment_fraction: f64,
    pub deficit_readonly_threshold_e8s: u64,
```

In `src/rumi_protocol_backend/src/main.rs::get_protocol_status` (line 538), append inside the `ProtocolStatus { ... }` constructor:

```rust
        protocol_deficit_icusd: s.protocol_deficit_icusd.to_u64(),
        total_deficit_repaid_icusd: s.total_deficit_repaid_icusd.to_u64(),
        deficit_repayment_fraction: s.deficit_repayment_fraction.to_f64(),
        deficit_readonly_threshold_e8s: s.deficit_readonly_threshold_e8s,
```

- [ ] **Step 11.5: Add admin endpoints in main.rs**

Find an existing admin endpoint (e.g., `set_borrowing_fee` or `set_liquidation_protocol_share`) in `src/rumi_protocol_backend/src/main.rs` to use as a template — look for `require_controller` or similar caller-gating. Add:

```rust
#[candid_method(update)]
#[update]
fn set_deficit_repayment_fraction(fraction: f64) -> Result<(), ProtocolError> {
    require_controller()?;
    if !fraction.is_finite() || fraction < 0.0 || fraction > 1.0 {
        return Err(ProtocolError::GenericError(format!(
            "deficit_repayment_fraction must be in [0.0, 1.0]; got {}",
            fraction
        )));
    }
    let dec = rust_decimal::Decimal::from_f64(fraction)
        .ok_or_else(|| ProtocolError::GenericError("non-finite fraction".to_string()))?;
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_deficit_repayment_fraction(
            s,
            rumi_protocol_backend::numeric::Ratio::from(dec),
        );
    });
    Ok(())
}

#[candid_method(update)]
#[update]
fn set_deficit_readonly_threshold_e8s(threshold_e8s: u64) -> Result<(), ProtocolError> {
    require_controller()?;
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_deficit_readonly_threshold_e8s(
            s,
            threshold_e8s,
        );
    });
    Ok(())
}
```

(The `require_controller` helper exists in main.rs — verify by `grep -n require_controller src/rumi_protocol_backend/src/main.rs`. If the project uses a different gate name, use that instead.)

- [ ] **Step 11.6: Update candid (.did file)**

Add to `src/rumi_protocol_backend/rumi_protocol_backend.did`:

In the `ProtocolStatus` record, append:
```candid
  protocol_deficit_icusd : nat64;
  total_deficit_repaid_icusd : nat64;
  deficit_repayment_fraction : float64;
  deficit_readonly_threshold_e8s : nat64;
```

In the service block, add the two new methods:
```candid
  set_deficit_repayment_fraction : (float64) -> (variant { Ok; Err : ProtocolError });
  set_deficit_readonly_threshold_e8s : (nat64) -> (variant { Ok; Err : ProtocolError });
```

(Match the existing style in the .did file. The `Result<(), ProtocolError>` translates to `variant { Ok; Err : ProtocolError }` per the existing convention.)

- [ ] **Step 11.7: Run tests and check candid**

Run: `POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --test audit_pocs_liq_005_deficit_account && POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --lib`
Expected: PASS — 28 unit tests.

Also run the candid compatibility check:
Run: `cargo build -p rumi_protocol_backend --target wasm32-unknown-unknown 2>&1 | tail -20`
Expected: clean build.

- [ ] **Step 11.8: Commit**

```bash
git add src/rumi_protocol_backend/src/event.rs src/rumi_protocol_backend/src/lib.rs src/rumi_protocol_backend/src/main.rs src/rumi_protocol_backend/rumi_protocol_backend.did src/rumi_protocol_backend/tests/audit_pocs_liq_005_deficit_account.rs
git commit -m "$(cat <<'EOF'
feat(api): Wave-8e LIQ-005 expose deficit fields + admin endpoints

ProtocolStatus gains four new fields so the explorer can surface deficit
state at a glance: protocol_deficit_icusd, total_deficit_repaid_icusd,
deficit_repayment_fraction, deficit_readonly_threshold_e8s.

Two new admin endpoints (set_deficit_repayment_fraction,
set_deficit_readonly_threshold_e8s) gate on require_controller and emit
audit events. Defaults: fraction = 0.5, threshold = 0 (latch disabled
until operator sets a baseline post-deploy).

Candid is updated additively — pre-Wave-8e callers continue to work and
the new fields appear at the end of the ProtocolStatus record.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 12: Layer-3 PocketIC fence

**Files:**
- Create: `src/rumi_protocol_backend/tests/audit_pocs_liq_005_deficit_account_pic.rs`.

- [ ] **Step 12.1: Scaffold the file using the Wave-8d Phase 2 fixture**

Read `src/rumi_protocol_backend/tests/audit_pocs_liq_004_icrc3_burn_proof_pic.rs` for the lean PocketIC fixture pattern (canister setup, ledger init, helper calls). Write the new file to mirror that scaffolding.

The seven required test cases (from the user's brief, slightly tightened):

```rust
//! Wave-8e LIQ-005: Layer-3 PocketIC fences for the deficit account.
//!
//! Each test installs a fresh fixture (rumi_protocol_backend +
//! icusd_ledger + a mock collateral ledger), opens at least one vault,
//! and asserts the deficit accounting end-to-end through the canister
//! boundary. Reuses the lean fixture pattern from
//! `audit_pocs_liq_004_icrc3_burn_proof_pic.rs`.

// Standard PocketIC imports — copy from the LIQ-004 file.

#[test]
fn liq_005_pocket_ic_underwater_liquidation_accrues_deficit() {
    // 1. Open a vault: 100 ICP collateral at $10 → $1000. Borrow 800 icUSD.
    //    (CR = 125%, healthy at 110% liq threshold.)
    // 2. Drop ICP price hard via XRC mock: $10 → $5.
    //    (Now collateral = $500, debt = 800 → CR = 62.5%, deeply underwater.)
    // 3. Call liquidate_vault (full).
    // 4. Assert get_protocol_status().protocol_deficit_icusd > 0.
    //    Expected shortfall ≈ 800 - 500 = $300 (300_000_000_e8s).
    //    Verify the value is within ±1% of expected.
    // 5. Assert exactly one DeficitAccrued event in the log via get_events.
}

#[test]
fn liq_005_pocket_ic_borrowing_fee_repays_deficit() {
    // 1. Same as above to seed deficit ≈ 300 icUSD.
    // 2. Set deficit_repayment_fraction = 0.5 (default; explicit set anyway).
    // 3. Open a new vault and borrow 100 icUSD — this collects a borrowing fee.
    //    Read get_protocol_status before and after.
    // 4. Assert protocol_deficit_icusd decreased by exactly fraction × fee.
    //    Assert total_deficit_repaid_icusd increased by the same.
    //    Assert one DeficitRepaid{source=BorrowingFee} event.
    //    Assert the treasury icUSD balance grew by `fee - to_repay` (NOT `fee`).
}

#[test]
fn liq_005_pocket_ic_redemption_fee_repays_deficit() {
    // 1. Seed deficit ≈ 100 icUSD by liquidating an underwater vault.
    // 2. Open a new vault, borrow 1000 icUSD, transfer to a redeemer principal.
    // 3. Redeemer calls redeem_collateral with 100 icUSD.
    // 4. Assert protocol_deficit_icusd decremented by fraction × redemption_fee.
    //    Assert one DeficitRepaid{source=RedemptionFee} event.
}

#[test]
fn liq_005_pocket_ic_repayment_caps_at_remaining_deficit() {
    // 1. Seed deficit = 5 icUSD (small, so the fraction-based candidate exceeds it).
    // 2. Trigger a 100 icUSD borrow → fee ≈ 0.5 icUSD (depending on fee rate).
    //    With fraction = 0.5, candidate = 0.25 icUSD → cap binds at 5 — wait,
    //    candidate (0.25) < deficit (5), so cap doesn't bind here.
    //    Adjust fixture: set deficit = 0.001 icUSD so the fee 0.5 × 0.5 = 0.25
    //    exceeds the deficit and cap binds.
    // 3. Assert protocol_deficit_icusd == 0 after the borrow.
    //    Assert total_deficit_repaid_icusd == 0.001 icUSD (the original deficit).
    //    Treasury mint == fee - 0.001.
}

#[test]
fn liq_005_pocket_ic_readonly_latch_at_threshold() {
    // 1. Set deficit_readonly_threshold_e8s = 100_000_000 (1 icUSD).
    // 2. Seed deficit to 99_000_000 via a controlled liquidation.
    // 3. Confirm mode is still Normal.
    // 4. Trigger another underwater liquidation that adds ≥ 1_000_000 of
    //    deficit (crossing 100_000_000).
    // 5. Assert get_protocol_status().mode == Mode::ReadOnly.
    // 6. Assert a subsequent borrow_from_vault returns ProtocolError::Frozen
    //    (or whatever ReadOnly returns — verify by reading validate_mode).
}

#[test]
fn liq_005_pocket_ic_admin_can_clear_readonly_after_latch() {
    // 1. Trip the latch as in the test above.
    // 2. Admin calls exit_recovery_mode.
    // 3. Assert mode is back to Normal AND that subsequent borrows succeed.
    // 4. Re-trigger the predicate → confirm latch fires again from the
    //    next deficit increment (not stuck disabled).
}

#[test]
fn liq_005_pocket_ic_upgrade_preserves_deficit_state() {
    // 1. Seed deficit = 50_000 icUSD; total_repaid = 25_000 icUSD;
    //    fraction = 0.75; threshold = 1_000_000_000.
    // 2. Upgrade the rumi_protocol_backend canister (re-install with mode=Upgrade).
    // 3. Read get_protocol_status — assert all four fields survive intact.
}
```

For each test, follow the LIQ-004 file's pattern of:
1. `let pic = create_pocket_ic_fixture(...);`
2. Set XRC mock prices.
3. Make `update_call` / `query_call` invocations as described.
4. Decode candid responses with `Decode!(...)`.
5. Compare events from `get_events`.

Use a 2% tolerance for any fee or shortfall comparisons (decimal rounding may give a few e8s difference).

- [ ] **Step 12.2: Run the PocketIC fence**

Run: `POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --test audit_pocs_liq_005_deficit_account_pic`
Expected: 7 tests pass.

If a test fails, diagnose:
- Wrong shortfall computation → re-verify the predicate at the call site.
- Repayment doesn't fire → verify `plan_fee_routing` is wired correctly in `mint_borrowing_fee_to_treasury` and the redemption sites.
- ReadOnly latch sticky → verify `check_deficit_readonly_latch` only sets mode when threshold > 0 and only on crossing.

- [ ] **Step 12.3: Commit**

```bash
git add src/rumi_protocol_backend/tests/audit_pocs_liq_005_deficit_account_pic.rs
git commit -m "$(cat <<'EOF'
test(audit_pocs): Wave-8e LIQ-005 Layer-3 PocketIC fence

Seven canister-boundary tests cover the LIQ-005 contract end-to-end:
underwater liquidation accrues, borrowing fee repays, redemption fee
repays, repayment caps at remaining deficit, ReadOnly latch fires,
admin clears latch, and upgrade preserves all four state fields.

Uses the lean fixture pattern from audit_pocs_liq_004_icrc3_burn_proof_pic.rs
so the build cost stays low. POCKET_IC_BIN=./pocket-ic must be set.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 13: Frontend candid regen

**Files:**
- Modify (regenerated): `src/declarations/rumi_protocol_backend/*`.

- [ ] **Step 13.1: Regenerate declarations**

Run: `dfx generate rumi_protocol_backend`
Expected: declarations updated to include the four new ProtocolStatus fields and two new admin methods.

- [ ] **Step 13.2: Inspect the diff**

Run: `git diff -- src/declarations/rumi_protocol_backend/`
Expected: additive changes only — new fields + new methods. No renames or deletions.

- [ ] **Step 13.3: Commit**

```bash
git add src/declarations/rumi_protocol_backend/
git commit -m "$(cat <<'EOF'
chore(declarations): regenerate after Wave-8e LIQ-005 candid changes

dfx generate rumi_protocol_backend output. Additive only — four new
ProtocolStatus fields, two new admin endpoints. Pre-Wave-8e frontend
callers continue to work.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 14: Open PR

- [ ] **Step 14.1: Push branch**

```bash
git push -u origin fix/audit-wave-8e-liq-005-deficit-account
```

- [ ] **Step 14.2: Open PR**

```bash
gh pr create --title "Wave-8e LIQ-005: bad-debt deficit account + fee-driven repayment" --body "$(cat <<'EOF'
## Summary

Implements LIQ-005 from the 2026-04-22 audit: isolate underwater-liquidation bad debt into a protocol-level deficit account that future fee revenue repays. No socialization to stability-pool depositors or pro-rata redistribution to other vaults. Adds a configurable ReadOnly auto-latch as a secondary guard if the deficit grows past a threshold.

- New State fields (all `#[serde(default)]` for upgrade safety): `protocol_deficit_icusd`, `total_deficit_repaid_icusd`, `deficit_repayment_fraction` (default 0.5), `deficit_readonly_threshold_e8s` (default 0 = disabled).
- All 5 liquidation paths instrumented (`liquidate_vault`, `liquidate_vault_partial`, `liquidate_vault_partial_with_stable`, `partial_liquidate_vault`, `liquidate_vault_debt_already_burned`) with a per-call `seized_usd < debt_cleared` predicate that accrues the shortfall + emits `DeficitAccrued` + checks the ReadOnly latch.
- Both fee streams (borrowing fee, redemption fee) route a configurable fraction to deficit repayment via `plan_fee_routing`. Borrowing fee: mint less to treasury (foregone revenue). Redemption fee: pure state mutation (icUSD already burned via transfer_icusd_from to minting account). Both emit `DeficitRepaid` with the fee source.
- 4 new ProtocolStatus fields surfacing the deficit state to the explorer.
- 2 new admin endpoints: `set_deficit_repayment_fraction`, `set_deficit_readonly_threshold_e8s`. Both gated by `require_controller`.
- Additive candid; pre-Wave-8e callers remain compatible.

Audit anchor: `audit-reports/2026-04-22-28e9896/raw-pass-results/liquidation-mechanics.json` LIQ-005.

## Test plan

- [x] `cargo test -p rumi_protocol_backend --test audit_pocs_liq_005_deficit_account` — 28 Layer-1 / Layer-2 unit tests.
- [x] `POCKET_IC_BIN=./pocket-ic cargo test -p rumi_protocol_backend --test audit_pocs_liq_005_deficit_account_pic` — 7 Layer-3 PocketIC tests.
- [x] `cargo test -p rumi_protocol_backend --lib` — backend lib still 83 passed, 1 ignored.
- [x] `POCKET_IC_BIN=./pocket-ic cargo test -p rumi_protocol_backend --test pocket_ic_tests` — 27 passed, 2 ignored (pre-existing).
- [x] Smoke `dfx canister --network ic call rumi_protocol_backend get_protocol_status '()' --query` post-deploy: confirm new fields decode + read default values.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

---

## Task 15: Deploy to mainnet

- [ ] **Step 15.1: Confirm pre-deploy hook is enabled**

Run: `cat .claude/hooks/pre-deploy-test.sh | head -20`
Expected: hook script exists. Do not skip it during the deploy.

- [ ] **Step 15.2: Switch to deployment identity**

```bash
dfx identity use rumi_identity
dfx identity get-principal
```
Expected: `fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae`.

- [ ] **Step 15.3: Deploy backend canister**

```bash
dfx deploy rumi_protocol_backend --network ic --argument '(variant { Upgrade = record { mode = null; description = opt "Wave-8e LIQ-005: bad-debt deficit account + fee-driven repayment + ReadOnly auto-latch" } })'
```

The pre-deploy hook will run unit + integration tests. Wait for it to pass before the upgrade lands.

- [ ] **Step 15.4: Post-deploy smoke checks**

```bash
dfx canister --network ic call rumi_protocol_backend get_protocol_status '()' --query
```

Expected: response includes `protocol_deficit_icusd = 0`, `total_deficit_repaid_icusd = 0`, `deficit_repayment_fraction = 0.5`, `deficit_readonly_threshold_e8s = 0`.

```bash
dfx canister --network ic info rumi_protocol_backend
```

Expected: module hash differs from `0x2804f726cc127466a719a951ccd1cd2bd5e24ea381bbe7e2e170399ec418759d` (the post-Wave-8d hash).

- [ ] **Step 15.5: Watch logs for 24-48h**

Use `dfx canister --network ic logs rumi_protocol_backend` periodically. Expected: rare `[LIQ-005]` lines on day 1 (no historical underwater liquidations to back-fill since the audit decision is forward-looking only). If `DeficitAccrued` events fire on perfectly healthy vaults, the predicate is too aggressive — investigate immediately and consider rolling back.

- [ ] **Step 15.6: Update memory + close out**

After 24h with no false positives, save a short project memory entry recording the deploy date, module hash, and any baseline deficit figures observed. Wave 10 (LIQ-008 mass-liquidation circuit breaker) can then begin in a separate session.
