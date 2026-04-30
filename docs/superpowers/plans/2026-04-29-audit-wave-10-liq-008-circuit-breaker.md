# Wave 10: LIQ-008 Mass-Liquidation Circuit Breaker Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Pause auto-publishing of underwater vaults to bot/SP once cumulative liquidated debt within a rolling window crosses a configurable ceiling. Manual liquidation endpoints (`liquidate_vault`, `liquidate_vault_partial`, `liquidate_vault_partial_with_stable`, `partial_liquidate_vault`, `liquidate_vault_debt_already_burned`) stay open.

**Architecture:** Single backend canister upgrade. Adds four `#[serde(default)]` fields to `State` for upgrade safety: a `Vec<(timestamp_ns, debt_e8s)>` rolling log, a window length (default 30 min), a debt ceiling (default 0 = disabled), and a sticky tripped flag. Each of the five existing `record_deficit_accrued` sites in `vault.rs` (Wave-8e LIQ-005 instrumentation) gets a sibling call to `record_recent_liquidation` that appends the path's `debt_cleared_e8s`, prunes entries older than the window, and checks the ceiling. `check_vaults()` consults the tripped flag at the top of `lib.rs::check_vaults` and short-circuits the bot/SP push when set. Three admin endpoints expose tunables (`set_breaker_window_ns`, `set_breaker_window_debt_ceiling_e8s`, `clear_liquidation_breaker`). Four new ProtocolStatus fields + new candid declarations. Two new event variants for the auto-trip / admin-clear audit trail; the two `Set*` variants collapse into the existing `Admin` filter bucket.

**Tech Stack:** Rust, ic-cdk, candid, ic-stable-structures (CBOR via serde_cbor for state). PocketIC 6.0.0 for integration tests.

---

## Wave 0 verification (locked)

**Publishing path mapped:** `check_vaults()` ([src/rumi_protocol_backend/src/lib.rs:477](src/rumi_protocol_backend/src/lib.rs:477)) is invoked from `xrc::fetch_icp_rate` ([src/rumi_protocol_backend/src/xrc.rs:117](src/rumi_protocol_backend/src/xrc.rs:117)) once every 5 minutes after a successful price fetch. It calls `notify_liquidatable_vaults` on the bot ([lib.rs:681](src/rumi_protocol_backend/src/lib.rs:681)) and the stability pool ([lib.rs:705](src/rumi_protocol_backend/src/lib.rs:705)). Manual liquidation endpoints in `main.rs` (`liquidate_vault`, `liquidate_vault_partial`, etc., gated only by `validate_price_for_liquidation` and `validate_freshness_for_vault`) do NOT go through `check_vaults` — the breaker is invisible to them.

**LIQ-006 status: CLOSED.** Wave-5 added `validate_freshness_for_vault` ([src/rumi_protocol_backend/src/main.rs:131](src/rumi_protocol_backend/src/main.rs:131)) which awaits `xrc::ensure_fresh_price_for(&vault.collateral_type)` before each liquidation entry point. Per-collateral price age is now enforced. No need to fold into Wave 10.

**LIQ-007 status: CLOSED.** Wave-5 shipped the sanity band: `pending_outlier_prices` ([state.rs:893-901](src/rumi_protocol_backend/src/state.rs:893)) plus the rejection logic in [xrc.rs:69](src/rumi_protocol_backend/src/xrc.rs:69) and [management.rs:444, 603](src/rumi_protocol_backend/src/management.rs:444). The ReadOnly-vs-liquidations design is documented at [main.rs:110-122](src/rumi_protocol_backend/src/main.rs:110): `liquidation_frozen` is the explicit admin switch, decoupled from `validate_mode` because ReadOnly auto-latches on TCR < 100% and liquidations should remain open in that state (they reduce bad debt). No need to fold into Wave 10.

**LIQ-005 instrumentation sites already present** in `vault.rs` ([2241](src/rumi_protocol_backend/src/vault.rs:2241), [2534](src/rumi_protocol_backend/src/vault.rs:2534), [2924](src/rumi_protocol_backend/src/vault.rs:2924), [3180](src/rumi_protocol_backend/src/vault.rs:3180), [3637](src/rumi_protocol_backend/src/vault.rs:3637)). Each site already has `debt_cleared_e8s` (or its equivalent: `max_liquidatable_debt`, `liquidator_payment`, `actual_liquidation_amount`, `icusd_burned_e8s`, `debt_amount`) computed inside the same `mutate_state` block. Wave 10 co-locates a second call (`record_recent_liquidation`) at each site.

**PocketIC fixture to lift:** `src/rumi_protocol_backend/tests/audit_pocs_liq_005_deficit_account_pic.rs` has `deploy_icrc1_ledger` ([line 216](src/rumi_protocol_backend/tests/audit_pocs_liq_005_deficit_account_pic.rs:216)), `drop_icp_price` ([line 521](src/rumi_protocol_backend/tests/audit_pocs_liq_005_deficit_account_pic.rs:521)), `prepare_mock_xrc`, and the boot/icRC-1/icp-ledger/treasury setup. Wave 10's PIC tests duplicate this fixture and extend `ProtocolStatusSubset` with the four new breaker fields.

---

## Design decisions (locked before implementation)

### Gating signal: G1 (cumulative debt cleared per window)

Track every successful liquidation's `debt_cleared_e8s` (sum across all 5 paths) into a rolling window. Trip when the windowed total exceeds `breaker_window_debt_ceiling_e8s`. Predicate is in icUSD e8s — easy to compare to `total_icusd_borrowed` for an operator-facing TVL ratio.

**Why not G2 (deficit growth rate per window):** healthy liquidation cascades — where the SP is happily absorbing — are still a problem worth pausing. They grind through DEX liquidity and accelerate price drops. G2 would miss those because deficit doesn't accrue on healthy seizures. We expose deficit growth rate as a derived metric in `ProtocolStatus` for operator dashboards, but gate on G1.

### Window storage: W1 (`Vec<(u64, u64)>` with eviction)

Append on every liquidation; prune entries older than `breaker_window_ns` on every write. Reads filter without mutating — pure functional sum. Bounded by liquidation rate × window: 1 liq/sec for 30 min is 1800 entries (≈ 29 KB); tolerable.

**Why not W2 (sliding-bucket sum):** more code, marginal memory savings at our scale. If pathological growth shows up post-deploy we switch to W2 in a follow-up wave — the State field shape stays internal so this is a one-canister-upgrade migration.

**Pruning rule:** writes prune in-place to keep the Vec bounded, reads filter without mutation. This avoids any "query mutates state" pitfall and means a long idle period doesn't leave stale entries inflating reads.

### Trip behavior: T2 (admin-clear latch, no auto-clear)

Once tripped, `liquidation_breaker_tripped` stays true until admin calls `clear_liquidation_breaker`. The breaker is the operator's "something is very wrong" signal — auto-clearing risks toggling on/off across the threshold during a sustained downturn. Admin clear forces a human to look at deficit + supply state before reopening auto-publishing.

**Why not T1 (auto-pause only) or T3 (auto-clear on window roll):** the audit's recommendation says "raise a protocol alert" which implies operator-in-the-loop. T2 matches that intent.

### Partial-size taper during stress: defer

The audit suggests optionally biasing toward smaller partials when the windowed total is in the (50%, 100%) of ceiling band. Defer to a follow-up wave. Wave 10 ships the breaker itself; if the breaker turns out to be too coarse-grained in practice we add the taper after observing one real firing.

### `BreakerTripped` / `BreakerCleared` event variants: yes

Mirror the LIQ-005 pattern. `DeficitAccrued` is auto-emitted with its own `EventTypeFilter::DeficitAccrued` variant; `BreakerTripped` follows that pattern. `BreakerCleared` is admin-initiated and collapses into `EventTypeFilter::Admin`. `SetBreakerWindowNs` and `SetBreakerWindowDebtCeilingE8s` are admin tunables and also collapse into `Admin`.

The combination gives the explorer a clean filter — operators can audit "every breaker firing" without scrolling through unrelated admin events.

---

## File Structure

**Modify:**
- `src/rumi_protocol_backend/src/state.rs` — add 4 new fields to `State` (`#[serde(default)]`), `default_breaker_window_ns` helper, `record_recent_liquidation` helper, `windowed_liquidation_total` getter, update `Default for State` and the test-fixture init block.
- `src/rumi_protocol_backend/src/event.rs` — add `BreakerTripped`, `BreakerCleared`, `SetBreakerWindowNs`, `SetBreakerWindowDebtCeilingE8s` variants + recorder helpers + extend `is_vault_related`, `type_filter`, `involves_principal`, `EventTypeFilter` mapping (add `BreakerTripped` filter variant; the others collapse to `Admin`).
- `src/rumi_protocol_backend/src/vault.rs` — at the 5 LIQ-005 deficit-accrual sites, add a sibling call to `crate::state::record_recent_liquidation` with the same `debt_cleared_e8s` value used for the LIQ-005 predicate.
- `src/rumi_protocol_backend/src/lib.rs` — add the breaker gate at the top of `check_vaults()` (after the bot-claim auto-cancel block). Add 4 new fields to `ProtocolStatus`. Extend `EventTypeFilter` enum.
- `src/rumi_protocol_backend/src/main.rs` — populate new ProtocolStatus fields in `get_protocol_status`. Add 3 admin endpoints: `set_breaker_window_ns`, `set_breaker_window_debt_ceiling_e8s`, `clear_liquidation_breaker`.
- `src/rumi_protocol_backend/rumi_protocol_backend.did` — additive candid changes: 4 new ProtocolStatus fields, 3 new admin endpoints.
- `src/declarations/rumi_protocol_backend/*` — regenerated via `dfx generate`.

**Create:**
- `src/rumi_protocol_backend/tests/audit_pocs_liq_008_circuit_breaker.rs` — Layer 1+2 unit tests (no canister).
- `src/rumi_protocol_backend/tests/audit_pocs_liq_008_circuit_breaker_pic.rs` — Layer 3 PocketIC tests.

---

## Task 1: Add State fields + helper fn + Layer-1 CBOR round-trip tests

**Files:**
- Modify: `src/rumi_protocol_backend/src/state.rs` (insert after the Wave-8e LIQ-005 fields ending at line 1003; add to `Default for State` at line 1107 and the second init block at line 1306).
- Test: `src/rumi_protocol_backend/tests/audit_pocs_liq_008_circuit_breaker.rs` (new file).

- [ ] **Step 1.1: Write the failing CBOR round-trip + defaults tests**

Create `src/rumi_protocol_backend/tests/audit_pocs_liq_008_circuit_breaker.rs`:

```rust
//! Wave-10 LIQ-008: mass-liquidation circuit breaker — Layer 1+2 unit tests.
//!
//! Layer 1: state-model invariants, CBOR round-trip, default values, append-
//! and-prune behavior, ceiling-cross trip behavior, admin-clear semantics.
//! Layer 2: edge cases (window=0 disables, ceiling=0 disables, single big
//! liquidation, prune past window boundary).
//!
//! No canister, no async, no PocketIC. PocketIC fences live in
//! `audit_pocs_liq_008_circuit_breaker_pic.rs`.

use rumi_protocol_backend::state::{record_recent_liquidation, State};

const NS_PER_SEC: u64 = 1_000_000_000;
const DEFAULT_WINDOW_NS: u64 = 30 * 60 * NS_PER_SEC;

#[test]
fn liq_008_state_defaults_disabled_breaker() {
    let s = State::default();
    assert!(s.recent_liquidations.is_empty());
    assert_eq!(s.breaker_window_ns, DEFAULT_WINDOW_NS);
    assert_eq!(s.breaker_window_debt_ceiling_e8s, 0);
    assert!(!s.liquidation_breaker_tripped);
    assert_eq!(s.windowed_liquidation_total(0), 0);
}

#[test]
fn liq_008_state_round_trip_preserves_breaker_fields() {
    let mut s = State::default();
    s.recent_liquidations = vec![(1_000_000, 5_000), (2_000_000, 7_500)];
    s.breaker_window_ns = 60 * NS_PER_SEC;
    s.breaker_window_debt_ceiling_e8s = 1_000_000_000;
    s.liquidation_breaker_tripped = true;

    let bytes = serde_cbor::to_vec(&s).expect("encode");
    let decoded: State = serde_cbor::from_slice(&bytes).expect("decode");

    assert_eq!(decoded.recent_liquidations, s.recent_liquidations);
    assert_eq!(decoded.breaker_window_ns, 60 * NS_PER_SEC);
    assert_eq!(decoded.breaker_window_debt_ceiling_e8s, 1_000_000_000);
    assert!(decoded.liquidation_breaker_tripped);
}

#[test]
fn liq_008_state_decodes_pre_wave_10_blob_with_defaults() {
    let s_full = State::default();
    let full_bytes = serde_cbor::to_vec(&s_full).expect("encode full");
    let mut value: serde_cbor::Value =
        serde_cbor::from_slice(&full_bytes).expect("decode to value");
    if let serde_cbor::Value::Map(m) = &mut value {
        m.retain(|k, _| match k {
            serde_cbor::Value::Text(t) => !matches!(
                t.as_str(),
                "recent_liquidations"
                    | "breaker_window_ns"
                    | "breaker_window_debt_ceiling_e8s"
                    | "liquidation_breaker_tripped"
            ),
            _ => true,
        });
    }
    let stripped = serde_cbor::to_vec(&value).expect("encode stripped");

    let decoded: State = serde_cbor::from_slice(&stripped).expect("decode old-shape");

    assert!(decoded.recent_liquidations.is_empty());
    assert_eq!(decoded.breaker_window_ns, DEFAULT_WINDOW_NS);
    assert_eq!(decoded.breaker_window_debt_ceiling_e8s, 0);
    assert!(!decoded.liquidation_breaker_tripped);
}
```

- [ ] **Step 1.2: Run tests to verify they fail (fields don't exist yet)**

Run: `POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --test audit_pocs_liq_008_circuit_breaker`

Expected: FAIL — compile error: `no field 'recent_liquidations' on type 'State'`.

- [ ] **Step 1.3: Add the four state fields + default fn**

Edit `src/rumi_protocol_backend/src/state.rs`. Insert this block immediately after the `deficit_readonly_threshold_e8s` field (current closing of the State struct at line 1003), inside `pub struct State { ... }`:

```rust
    // ─── Wave-10 LIQ-008: mass-liquidation circuit breaker ───
    //
    // Bounds the auto-publishing path (check_vaults → bot / stability pool)
    // once cumulative liquidated debt within a rolling window crosses a
    // configurable ceiling. Manual liquidation endpoints stay open. Once
    // tripped, the latch is sticky — admin must call clear_liquidation_breaker
    // to resume auto-publishing.
    //
    // serde(default) on every field — pre-Wave-10 snapshots decode to an
    // empty log, the 30-minute default window, a disabled ceiling (0), and
    // a not-tripped flag.

    /// Rolling-window log of liquidations for circuit-breaker gating.
    /// Each entry is `(timestamp_ns, debt_cleared_icusd_e8s)`. Pruned in
    /// place inside `record_recent_liquidation` to keep entries within
    /// `breaker_window_ns`. Reads sum without mutation.
    #[serde(default)]
    pub recent_liquidations: Vec<(u64, u64)>,

    /// Rolling window length in nanoseconds. Default 30 minutes. 0 disables
    /// the breaker entirely (no recording, no tripping). Admin-tunable via
    /// `set_breaker_window_ns`.
    #[serde(default = "default_breaker_window_ns")]
    pub breaker_window_ns: u64,

    /// Cumulative-debt-cleared ceiling within the window, in icUSD e8s.
    /// 0 disables tripping (operator should leave at 0 for the first 24-48h
    /// post-deploy and set after observing baseline `windowed_liquidation_total`).
    /// Admin-tunable via `set_breaker_window_debt_ceiling_e8s`.
    #[serde(default)]
    pub breaker_window_debt_ceiling_e8s: u64,

    /// Sticky latch. Set to true the first time the windowed debt total
    /// crosses the ceiling. Cleared by admin via `clear_liquidation_breaker`.
    /// While true, `check_vaults` skips both bot and SP `notify_liquidatable_vaults`
    /// pushes; manual liquidation endpoints are unaffected.
    #[serde(default)]
    pub liquidation_breaker_tripped: bool,
```

Then add the helper fn near the existing `default_deficit_repayment_fraction` (around line 110):

```rust
fn default_breaker_window_ns() -> u64 {
    30 * 60 * 1_000_000_000 // 30 minutes
}
```

Then add the four field initializers in `Default for State` at line 1107 (immediately after `deficit_readonly_threshold_e8s: 0,`):

```rust
            recent_liquidations: Vec::new(),
            breaker_window_ns: default_breaker_window_ns(),
            breaker_window_debt_ceiling_e8s: 0,
            liquidation_breaker_tripped: false,
```

Repeat the same four-line block in the second init at line 1306 (test-fixture path, immediately after `deficit_readonly_threshold_e8s: 0,`).

- [ ] **Step 1.4: Run tests to verify they pass**

Run: `POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --test audit_pocs_liq_008_circuit_breaker`

Expected: PASS, 3 tests.

NOTE: `windowed_liquidation_total` is not yet defined — `liq_008_state_defaults_disabled_breaker` will fail compilation until Task 2. That's OK; we'll see "method not found" and add it in Task 2.

Adjust step expectation: 2 of 3 PASS, 1 FAIL with method-not-found. Continue to Task 2 to satisfy the third.

- [ ] **Step 1.5: Commit**

```bash
git checkout -b fix/audit-wave-10-liq-008-circuit-breaker
git add src/rumi_protocol_backend/src/state.rs src/rumi_protocol_backend/tests/audit_pocs_liq_008_circuit_breaker.rs
git commit -m "feat(state): wave-10 add LIQ-008 circuit-breaker fields

Four #[serde(default)] fields: rolling-window log, window length (default
30 min), debt ceiling (default 0 = disabled), sticky tripped latch.
Layer-1 fence covers defaults + CBOR round-trip + pre-Wave-10 blob decode."
```

---

## Task 2: `record_recent_liquidation` + `windowed_liquidation_total` + Layer-1 record/prune/trip tests

**Files:**
- Modify: `src/rumi_protocol_backend/src/state.rs` — add the helper functions.
- Test: `src/rumi_protocol_backend/tests/audit_pocs_liq_008_circuit_breaker.rs`.

- [ ] **Step 2.1: Append the failing record/prune/trip tests to the Layer-1 file**

Append to `src/rumi_protocol_backend/tests/audit_pocs_liq_008_circuit_breaker.rs`:

```rust
#[test]
fn liq_008_record_appends_and_prunes_window() {
    let mut s = State::default();
    s.breaker_window_ns = 60 * NS_PER_SEC;

    record_recent_liquidation(&mut s, 1_000, 100 * NS_PER_SEC);
    record_recent_liquidation(&mut s, 2_000, 110 * NS_PER_SEC);
    record_recent_liquidation(&mut s, 3_000, 120 * NS_PER_SEC);
    assert_eq!(s.recent_liquidations.len(), 3);
    assert_eq!(s.windowed_liquidation_total(120 * NS_PER_SEC), 6_000);

    // Advance past the window and add another entry — earlier ones evicted
    // in-place by the new write's prune step.
    record_recent_liquidation(&mut s, 4_000, 200 * NS_PER_SEC);
    assert_eq!(s.recent_liquidations.len(), 1);
    assert_eq!(s.recent_liquidations[0].1, 4_000);
    assert_eq!(s.windowed_liquidation_total(200 * NS_PER_SEC), 4_000);
}

#[test]
fn liq_008_record_skips_when_window_zero() {
    let mut s = State::default();
    s.breaker_window_ns = 0;

    record_recent_liquidation(&mut s, 5_000, 100 * NS_PER_SEC);
    assert!(s.recent_liquidations.is_empty());
    assert!(!s.liquidation_breaker_tripped);
}

#[test]
fn liq_008_record_skips_when_debt_zero() {
    let mut s = State::default();
    s.breaker_window_ns = 60 * NS_PER_SEC;

    record_recent_liquidation(&mut s, 0, 100 * NS_PER_SEC);
    assert!(s.recent_liquidations.is_empty());
}

#[test]
fn liq_008_breaker_does_not_trip_when_ceiling_zero() {
    let mut s = State::default();
    s.breaker_window_ns = 60 * NS_PER_SEC;
    s.breaker_window_debt_ceiling_e8s = 0;

    record_recent_liquidation(&mut s, u64::MAX / 2, 100 * NS_PER_SEC);
    assert!(!s.liquidation_breaker_tripped);
}

#[test]
fn liq_008_breaker_trips_at_ceiling() {
    let mut s = State::default();
    s.breaker_window_ns = 60 * NS_PER_SEC;
    s.breaker_window_debt_ceiling_e8s = 1_000;

    record_recent_liquidation(&mut s, 600, 100 * NS_PER_SEC);
    assert!(!s.liquidation_breaker_tripped);

    record_recent_liquidation(&mut s, 500, 110 * NS_PER_SEC);
    assert!(s.liquidation_breaker_tripped);
    assert_eq!(s.windowed_liquidation_total(110 * NS_PER_SEC), 1_100);
}

#[test]
fn liq_008_single_huge_liquidation_trips_immediately() {
    let mut s = State::default();
    s.breaker_window_ns = 60 * NS_PER_SEC;
    s.breaker_window_debt_ceiling_e8s = 1_000;

    record_recent_liquidation(&mut s, 5_000, 100 * NS_PER_SEC);
    assert!(s.liquidation_breaker_tripped);
}

#[test]
fn liq_008_breaker_stays_tripped_after_window_rolls() {
    let mut s = State::default();
    s.breaker_window_ns = 60 * NS_PER_SEC;
    s.breaker_window_debt_ceiling_e8s = 1_000;

    record_recent_liquidation(&mut s, 1_500, 100 * NS_PER_SEC);
    assert!(s.liquidation_breaker_tripped);

    // Advance time past the window so windowed_liquidation_total drops to 0,
    // but the tripped flag stays — admin clear is required (T2 semantics).
    assert_eq!(s.windowed_liquidation_total(200 * NS_PER_SEC), 0);
    assert!(s.liquidation_breaker_tripped);
}

#[test]
fn liq_008_clear_breaker_resets_latch_but_preserves_log() {
    let mut s = State::default();
    s.breaker_window_ns = 60 * NS_PER_SEC;
    s.breaker_window_debt_ceiling_e8s = 1_000;

    record_recent_liquidation(&mut s, 1_500, 100 * NS_PER_SEC);
    assert!(s.liquidation_breaker_tripped);
    assert_eq!(s.recent_liquidations.len(), 1);

    s.liquidation_breaker_tripped = false;
    assert_eq!(s.recent_liquidations.len(), 1);
    assert_eq!(s.windowed_liquidation_total(100 * NS_PER_SEC), 1_500);
}

#[test]
fn liq_008_window_one_ns_evicts_immediately() {
    let mut s = State::default();
    s.breaker_window_ns = 1;
    s.breaker_window_debt_ceiling_e8s = 1_000;

    record_recent_liquidation(&mut s, 500, 100);
    record_recent_liquidation(&mut s, 500, 200);
    record_recent_liquidation(&mut s, 500, 300);
    assert_eq!(s.recent_liquidations.len(), 1);
    assert_eq!(s.recent_liquidations[0], (300, 500));
    assert!(!s.liquidation_breaker_tripped);
}

#[test]
fn liq_008_windowed_total_filters_without_mutation() {
    let mut s = State::default();
    s.breaker_window_ns = 60 * NS_PER_SEC;

    record_recent_liquidation(&mut s, 100, 100 * NS_PER_SEC);
    record_recent_liquidation(&mut s, 200, 110 * NS_PER_SEC);
    let len_before = s.recent_liquidations.len();

    let total_now = s.windowed_liquidation_total(120 * NS_PER_SEC);
    assert_eq!(total_now, 300);
    assert_eq!(s.recent_liquidations.len(), len_before);

    // Reading from a future timestamp filters older entries out of the sum
    // without removing them.
    let total_future = s.windowed_liquidation_total(200 * NS_PER_SEC);
    assert_eq!(total_future, 0);
    assert_eq!(s.recent_liquidations.len(), len_before);
}
```

- [ ] **Step 2.2: Run tests to verify they fail (helpers not defined yet)**

Run: `POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --test audit_pocs_liq_008_circuit_breaker`

Expected: FAIL — compile error: cannot find function `record_recent_liquidation` or method `windowed_liquidation_total`.

- [ ] **Step 2.3: Add `record_recent_liquidation` free function and `windowed_liquidation_total` method**

Edit `src/rumi_protocol_backend/src/state.rs`. Add both helpers near the end of the `impl State` block (search for `pub fn check_deficit_readonly_latch` — the LIQ-005 helper — and add immediately after it):

```rust
    /// Wave-10 LIQ-008: pure-read sum of liquidation debt cleared in the
    /// rolling window ending at `now_ns`. Filters without mutation. Returns
    /// 0 when the window is disabled (`breaker_window_ns == 0`) or the log
    /// is empty.
    pub fn windowed_liquidation_total(&self, now_ns: u64) -> u64 {
        if self.breaker_window_ns == 0 {
            return 0;
        }
        let cutoff = now_ns.saturating_sub(self.breaker_window_ns);
        self.recent_liquidations
            .iter()
            .filter(|(ts, _)| *ts >= cutoff)
            .map(|(_, debt)| *debt)
            .sum()
    }
```

Then add a free function at the bottom of `state.rs` (or beside `accrue_deficit_shortfall` in the existing `impl State`):

```rust
/// Wave-10 LIQ-008: append a successful liquidation to the rolling-window
/// log, prune entries past `breaker_window_ns`, and trip the latch if the
/// windowed total crosses the ceiling. Free function (not method) so the
/// 5 vault.rs liquidation sites can call it directly via
/// `crate::state::record_recent_liquidation(s, debt_e8s, ic_cdk::api::time())`.
///
/// No-ops if the window is disabled (window_ns == 0) or the recorded debt
/// is zero. Idempotent at the latch level: once tripped, additional records
/// continue to append + prune but do not re-emit a `BreakerTripped` event
/// (the trip is a one-shot signal — admin clear arms the next firing).
pub fn record_recent_liquidation(state: &mut State, debt_e8s: u64, now_ns: u64) {
    if debt_e8s == 0 || state.breaker_window_ns == 0 {
        return;
    }
    state.recent_liquidations.push((now_ns, debt_e8s));
    let cutoff = now_ns.saturating_sub(state.breaker_window_ns);
    state.recent_liquidations.retain(|(ts, _)| *ts >= cutoff);

    if state.breaker_window_debt_ceiling_e8s == 0 || state.liquidation_breaker_tripped {
        return;
    }
    let total = state.windowed_liquidation_total(now_ns);
    if total >= state.breaker_window_debt_ceiling_e8s {
        state.liquidation_breaker_tripped = true;
        ic_canister_log::log!(
            crate::logs::INFO,
            "[LIQ-008] circuit breaker tripped: windowed total {} e8s >= ceiling {} e8s (window {} ns, log size {})",
            total,
            state.breaker_window_debt_ceiling_e8s,
            state.breaker_window_ns,
            state.recent_liquidations.len()
        );
        crate::event::record_breaker_tripped(state, total, now_ns);
    }
}
```

- [ ] **Step 2.4: Run tests to verify they pass**

Run: `POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --test audit_pocs_liq_008_circuit_breaker`

Expected: FAIL — `crate::event::record_breaker_tripped` not yet defined. Continue to Task 3 (event variants) to satisfy. The other tests (which don't traverse the trip path) should pass.

- [ ] **Step 2.5: Commit**

```bash
git add src/rumi_protocol_backend/src/state.rs src/rumi_protocol_backend/tests/audit_pocs_liq_008_circuit_breaker.rs
git commit -m "feat(state): wave-10 record_recent_liquidation + windowed_liquidation_total

Append-and-prune helper for the rolling window, plus a pure-read sum that
filters without mutation. record_recent_liquidation is the helper the 5
vault.rs liquidation sites will call as a sibling to record_deficit_accrued."
```

---

## Task 3: Event variants (`BreakerTripped`, `BreakerCleared`, `SetBreakerWindowNs`, `SetBreakerWindowDebtCeilingE8s`) + recorders + filter wiring

**Files:**
- Modify: `src/rumi_protocol_backend/src/event.rs`.
- Modify: `src/rumi_protocol_backend/src/lib.rs` (add `EventTypeFilter::BreakerTripped` enum variant).

- [ ] **Step 3.1: Add the four event variants to `enum Event`**

Edit `src/rumi_protocol_backend/src/event.rs`. Insert immediately after the `SetDeficitReadonlyThresholdE8s` variant (line 631-634), still inside the `enum Event { ... }` block:

```rust
    /// Wave-10 LIQ-008: circuit breaker auto-tripped because the rolling-
    /// window cumulative liquidation debt crossed the configured ceiling.
    /// `total_e8s` is the windowed sum at the moment of tripping;
    /// `ceiling_e8s` is the configured trip threshold for audit purposes.
    #[serde(rename = "breaker_tripped")]
    BreakerTripped {
        total_e8s: u64,
        ceiling_e8s: u64,
        timestamp: u64,
    },

    /// Wave-10 LIQ-008: admin manually cleared the breaker latch and
    /// resumed `check_vaults` auto-publishing. `remaining_total_e8s` is the
    /// windowed sum at the moment of clearing (informational; admins inspect
    /// it before deciding to clear).
    #[serde(rename = "breaker_cleared")]
    BreakerCleared {
        remaining_total_e8s: u64,
        timestamp: u64,
    },

    /// Wave-10 LIQ-008: admin tuned the rolling-window length.
    #[serde(rename = "set_breaker_window_ns")]
    SetBreakerWindowNs {
        window_ns: u64,
        timestamp: u64,
    },

    /// Wave-10 LIQ-008: admin tuned the cumulative-debt ceiling. 0 disables
    /// the breaker.
    #[serde(rename = "set_breaker_window_debt_ceiling_e8s")]
    SetBreakerWindowDebtCeilingE8s {
        ceiling_e8s: u64,
        timestamp: u64,
    },
```

- [ ] **Step 3.2: Update `is_vault_related` to return `false` for the new variants**

Find `pub fn is_vault_related(&self, filter_vault_id: &u64) -> bool` (currently around line 639). The four new variants must each return `false` (none of them are vault-keyed). Add to the match block alongside the existing per-variant arms:

```rust
            Event::BreakerTripped { .. } => false,
            Event::BreakerCleared { .. } => false,
            Event::SetBreakerWindowNs { .. } => false,
            Event::SetBreakerWindowDebtCeilingE8s { .. } => false,
```

- [ ] **Step 3.3: Update `involves_principal` to return `false` for the new variants**

Find the LIQ-005 entries `Event::SetDeficitRepaymentFraction { .. } => false,` (currently around lines 724-725). Add four sibling arms below:

```rust
            Event::BreakerTripped { .. } => false,
            Event::BreakerCleared { .. } => false,
            Event::SetBreakerWindowNs { .. } => false,
            Event::SetBreakerWindowDebtCeilingE8s { .. } => false,
```

- [ ] **Step 3.4: Update `type_filter` mapping**

Find `pub fn type_filter(&self) -> EventTypeFilter` (currently around line 736). Add immediately after the LIQ-005 mapping (`Event::DeficitAccrued { .. } => EventTypeFilter::DeficitAccrued,` etc):

```rust
            Event::BreakerTripped { .. } => EventTypeFilter::BreakerTripped,
            Event::BreakerCleared { .. } => EventTypeFilter::Admin,
            Event::SetBreakerWindowNs { .. } => EventTypeFilter::Admin,
            Event::SetBreakerWindowDebtCeilingE8s { .. } => EventTypeFilter::Admin,
```

- [ ] **Step 3.5: Update the textual `Some(...)` mapping near line 832**

Find `Event::SetDeficitRepaymentFraction { .. } => Some("SetDeficitRepaymentFraction"),`. Add four sibling arms:

```rust
            Event::BreakerTripped { .. } => Some("BreakerTripped"),
            Event::BreakerCleared { .. } => Some("BreakerCleared"),
            Event::SetBreakerWindowNs { .. } => Some("SetBreakerWindowNs"),
            Event::SetBreakerWindowDebtCeilingE8s { .. } => Some("SetBreakerWindowDebtCeilingE8s"),
```

- [ ] **Step 3.6: Update the human-readable rendering near line 1595**

Find `Event::SetDeficitRepaymentFraction { fraction, .. } => { ... }` and the matching `SetDeficitReadonlyThresholdE8s` arm. Add four sibling arms inside the same match block:

```rust
            Event::BreakerTripped { total_e8s, ceiling_e8s, .. } => {
                format!("BreakerTripped(total={} e8s, ceiling={} e8s)", total_e8s, ceiling_e8s)
            }
            Event::BreakerCleared { remaining_total_e8s, .. } => {
                format!("BreakerCleared(remaining={} e8s)", remaining_total_e8s)
            }
            Event::SetBreakerWindowNs { window_ns, .. } => {
                format!("SetBreakerWindowNs({} ns)", window_ns)
            }
            Event::SetBreakerWindowDebtCeilingE8s { ceiling_e8s, .. } => {
                format!("SetBreakerWindowDebtCeilingE8s({} e8s)", ceiling_e8s)
            }
```

- [ ] **Step 3.7: Add the four recorder helpers at the end of `event.rs`**

Append to `src/rumi_protocol_backend/src/event.rs` after `record_set_deficit_readonly_threshold_e8s`:

```rust
/// Wave-10 LIQ-008: recorder for an automatic breaker trip. Called from
/// `state::record_recent_liquidation` when the windowed total crosses the
/// configured ceiling. State mutation (`liquidation_breaker_tripped = true`)
/// happens at the call site so `record_recent_liquidation` can short-circuit
/// the `total >= ceiling` check on subsequent records.
pub fn record_breaker_tripped(_state: &mut State, total_e8s: u64, timestamp: u64) {
    let ceiling_e8s = _state.breaker_window_debt_ceiling_e8s;
    record_event(&Event::BreakerTripped {
        total_e8s,
        ceiling_e8s,
        timestamp,
    });
}

/// Wave-10 LIQ-008: recorder for the admin-clear path. Clears the latch
/// and emits `BreakerCleared` with the windowed total at the moment of
/// clearing.
pub fn record_breaker_cleared(state: &mut State, remaining_total_e8s: u64, timestamp: u64) {
    state.liquidation_breaker_tripped = false;
    record_event(&Event::BreakerCleared {
        remaining_total_e8s,
        timestamp,
    });
}

/// Wave-10 LIQ-008: recorder for an admin-tuned rolling window length.
pub fn record_set_breaker_window_ns(state: &mut State, window_ns: u64) {
    state.breaker_window_ns = window_ns;
    record_event(&Event::SetBreakerWindowNs {
        window_ns,
        timestamp: now(),
    });
}

/// Wave-10 LIQ-008: recorder for an admin-tuned debt ceiling.
pub fn record_set_breaker_window_debt_ceiling_e8s(state: &mut State, ceiling_e8s: u64) {
    state.breaker_window_debt_ceiling_e8s = ceiling_e8s;
    record_event(&Event::SetBreakerWindowDebtCeilingE8s {
        ceiling_e8s,
        timestamp: now(),
    });
}
```

- [ ] **Step 3.8: Add `BreakerTripped` to the `EventTypeFilter` enum in `lib.rs`**

Edit `src/rumi_protocol_backend/src/lib.rs`. Find `pub enum EventTypeFilter` (search for `DeficitAccrued` to locate). Add a new variant immediately after `DeficitRepaid`:

```rust
    /// Wave-10 LIQ-008: an automatic breaker trip due to mass-liquidation
    /// volume. Distinct from the admin tunables which collapse to `Admin`.
    BreakerTripped,
```

- [ ] **Step 3.9: Run the Layer-1 suite to verify the trip test now passes**

Run: `POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --test audit_pocs_liq_008_circuit_breaker`

Expected: PASS, all 11 tests.

- [ ] **Step 3.10: Run the existing backend lib suite to confirm no regression**

Run: `POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --lib`

Expected: 83 passed, 1 ignored (matches Wave-8e baseline).

- [ ] **Step 3.11: Commit**

```bash
git add src/rumi_protocol_backend/src/event.rs src/rumi_protocol_backend/src/lib.rs
git commit -m "feat(events): wave-10 add LIQ-008 breaker event variants + recorders

Four new variants: BreakerTripped (auto-trip), BreakerCleared (admin),
SetBreakerWindowNs (admin tunable), SetBreakerWindowDebtCeilingE8s (admin
tunable). BreakerTripped gets its own EventTypeFilter for explorer audit;
the other three collapse into the existing Admin filter bucket."
```

---

## Task 4: Per-path liquidation site recording in `vault.rs`

Five sites; each gets a single sibling call to `crate::state::record_recent_liquidation`. The variable holding `debt_cleared` differs per site — the table below maps each site to the expected variable.

**Files:**
- Modify: `src/rumi_protocol_backend/src/vault.rs` at lines 2241, 2534, 2924, 3180, 3637 (the existing `record_deficit_accrued` call sites).

| Site | Function | Local variable holding `debt_cleared_e8s` |
|------|----------|-------------------------------------------|
| ~2241 | `liquidate_vault_partial` | `max_liquidatable_debt.to_u64()` |
| ~2534 | `liquidate_vault_partial_with_stable` | `max_liquidatable_debt.to_u64()` |
| ~2924 | `liquidate_vault_debt_already_burned` (SP writedown) | `icusd_burned_e8s` |
| ~3180 | `liquidate_vault` (full liq path) | `debt_amount.to_u64()` |
| ~3637 | `partial_liquidate_vault` | `liquidator_payment.to_u64()` |

The exact variable name at each site must be verified by reading the surrounding 30 lines — the LIQ-005 fence at the same site already passes `shortfall` (computed from these variables), so the variable will be in scope.

- [ ] **Step 4.1: Wire site #1 — `liquidate_vault_partial` (~line 2241)**

Read the existing block at vault.rs:2226-2260 to confirm variable names. Add a single line **before** the `if shortfall > 0 { ... record_deficit_accrued(...) }` block, still inside the same `mutate_state(|s| { ... })`:

```rust
        // Wave-10 LIQ-008: append to the rolling-window log for the
        // mass-liquidation circuit breaker. Records the gross debt cleared,
        // independent of whether this liquidation netted a deficit.
        crate::state::record_recent_liquidation(
            s,
            max_liquidatable_debt.to_u64(),
            ic_cdk::api::time(),
        );
```

- [ ] **Step 4.2: Wire site #2 — `liquidate_vault_partial_with_stable` (~line 2534)**

Mirror Step 4.1 at the second LIQ-005 site. Variable should be `max_liquidatable_debt.to_u64()`. If the site uses a different name (e.g., `actual_liquidation_amount`), substitute the actual variable.

```rust
        crate::state::record_recent_liquidation(
            s,
            max_liquidatable_debt.to_u64(),
            ic_cdk::api::time(),
        );
```

- [ ] **Step 4.3: Wire site #3 — `liquidate_vault_debt_already_burned` (~line 2924)**

The SP writedown path uses `icusd_burned_e8s` (a `u64` already, no `.to_u64()` call needed):

```rust
        crate::state::record_recent_liquidation(
            s,
            icusd_burned_e8s,
            ic_cdk::api::time(),
        );
```

- [ ] **Step 4.4: Wire site #4 — `liquidate_vault` (full liq path, ~line 3180)**

```rust
        crate::state::record_recent_liquidation(
            s,
            debt_amount.to_u64(),
            ic_cdk::api::time(),
        );
```

If the variable is `vault.borrowed_icusd_amount.to_u64()` (snapshot-before-mutate), use that — verify by reading the surrounding context. Whatever value `record_deficit_accrued` is using for its `shortfall` calculation as the "debt cleared" reference is the right value here.

- [ ] **Step 4.5: Wire site #5 — `partial_liquidate_vault` (~line 3637)**

```rust
        crate::state::record_recent_liquidation(
            s,
            liquidator_payment.to_u64(),
            ic_cdk::api::time(),
        );
```

- [ ] **Step 4.6: Run the backend lib suite + Layer-1 audit POCs to verify no compile regression**

Run: `POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --lib`

Expected: 83 passed, 1 ignored.

Run: `POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --test audit_pocs_liq_008_circuit_breaker`

Expected: PASS, 11 tests.

Run: `POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --test audit_pocs_liq_005_deficit_account`

Expected: PASS (Wave-8e Layer-1 fence still green — no regression).

- [ ] **Step 4.7: Commit**

```bash
git add src/rumi_protocol_backend/src/vault.rs
git commit -m "feat(vault): wave-10 record liquidations into LIQ-008 rolling window

Co-locate record_recent_liquidation alongside the LIQ-005 record_deficit_accrued
call at all 5 liquidation paths. Each site contributes its gross debt cleared
to the breaker's rolling-window log."
```

---

## Task 5: `check_vaults` breaker gate + ProtocolStatus + admin endpoints + candid

**Files:**
- Modify: `src/rumi_protocol_backend/src/lib.rs` — gate inside `check_vaults`, 4 new ProtocolStatus fields.
- Modify: `src/rumi_protocol_backend/src/main.rs` — populate ProtocolStatus, 3 admin endpoints.
- Modify: `src/rumi_protocol_backend/rumi_protocol_backend.did` — 4 new ProtocolStatus fields, 3 new endpoints.

- [ ] **Step 5.1: Gate the auto-publish path in `check_vaults`**

Edit `src/rumi_protocol_backend/src/lib.rs`. The bot-claim auto-cancel block runs unconditionally (it's hygiene). Insert the breaker check immediately AFTER the auto-cancel block ends and BEFORE the `let dummy_rate = read_state(...)` block (current line 503):

```rust
    // Wave-10 LIQ-008: short-circuit the auto-publishing path if the
    // breaker is tripped. Manual liquidation endpoints are unaffected.
    // The bot-claim auto-cancel above runs regardless because it is
    // hygiene, not auto-publishing.
    if read_state(|s| s.liquidation_breaker_tripped) {
        log!(
            INFO,
            "[LIQ-008] check_vaults skipping notify (breaker tripped). Manual liquidation remains available."
        );
        return;
    }
```

- [ ] **Step 5.2: Add the 4 new `ProtocolStatus` fields**

Edit `src/rumi_protocol_backend/src/lib.rs`. After the `deficit_readonly_threshold_e8s: u64,` field at line 135:

```rust
    /// Wave-10 LIQ-008: rolling window length for the mass-liquidation
    /// circuit breaker, in nanoseconds. 0 disables the breaker.
    pub breaker_window_ns: u64,
    /// Wave-10 LIQ-008: cumulative-debt ceiling within the window, in icUSD
    /// e8s. 0 disables the breaker.
    pub breaker_window_debt_ceiling_e8s: u64,
    /// Wave-10 LIQ-008: live windowed sum of debt cleared (icUSD e8s).
    /// Compares to `breaker_window_debt_ceiling_e8s` to project breaker headroom.
    pub windowed_liquidation_total_e8s: u64,
    /// Wave-10 LIQ-008: true once the breaker has tripped on the current
    /// window total. Cleared by admin via `clear_liquidation_breaker`.
    pub liquidation_breaker_tripped: bool,
```

- [ ] **Step 5.3: Populate the 4 new fields in `get_protocol_status`**

Edit `src/rumi_protocol_backend/src/main.rs`. Find the LIQ-005 lines:

```rust
        protocol_deficit_icusd: s.protocol_deficit_icusd.to_u64(),
        total_deficit_repaid_icusd: s.total_deficit_repaid_icusd.to_u64(),
        deficit_repayment_fraction: s.deficit_repayment_fraction.to_f64(),
        deficit_readonly_threshold_e8s: s.deficit_readonly_threshold_e8s,
```

Add four siblings immediately after:

```rust
        breaker_window_ns: s.breaker_window_ns,
        breaker_window_debt_ceiling_e8s: s.breaker_window_debt_ceiling_e8s,
        windowed_liquidation_total_e8s: s.windowed_liquidation_total(ic_cdk::api::time()),
        liquidation_breaker_tripped: s.liquidation_breaker_tripped,
```

- [ ] **Step 5.4: Add 3 admin endpoints**

Edit `src/rumi_protocol_backend/src/main.rs`. Append after `set_deficit_readonly_threshold_e8s` (currently ends ~line 3275):

```rust
/// Wave-10 LIQ-008: tune the rolling-window length for the mass-liquidation
/// circuit breaker. 0 disables the breaker (no recording, no tripping).
/// Admin-only.
#[candid_method(update)]
#[update]
async fn set_breaker_window_ns(new_window_ns: u64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set breaker window".to_string(),
        ));
    }
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_breaker_window_ns(s, new_window_ns);
    });
    log!(
        INFO,
        "[set_breaker_window_ns] Window set to: {} ns ({})",
        new_window_ns,
        if new_window_ns == 0 { "breaker disabled" } else { "breaker armed" }
    );
    Ok(())
}

/// Wave-10 LIQ-008: tune the cumulative-debt ceiling for the mass-liquidation
/// circuit breaker, in icUSD e8s. 0 disables tripping. Admin-only.
#[candid_method(update)]
#[update]
async fn set_breaker_window_debt_ceiling_e8s(new_ceiling: u64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set breaker debt ceiling".to_string(),
        ));
    }
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_breaker_window_debt_ceiling_e8s(s, new_ceiling);
    });
    log!(
        INFO,
        "[set_breaker_window_debt_ceiling_e8s] Ceiling set to: {} e8s ({})",
        new_ceiling,
        if new_ceiling == 0 { "breaker disabled" } else { "breaker armed" }
    );
    Ok(())
}

/// Wave-10 LIQ-008: clear the breaker latch so `check_vaults` resumes
/// auto-publishing on the next tick. Admin-only. Emits `BreakerCleared`
/// with the windowed total at clear time so the audit trail captures
/// what state the operator was looking at when they decided to resume.
#[candid_method(update)]
#[update]
async fn clear_liquidation_breaker() -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can clear the liquidation breaker".to_string(),
        ));
    }
    let now = ic_cdk::api::time();
    let remaining = read_state(|s| s.windowed_liquidation_total(now));
    mutate_state(|s| {
        rumi_protocol_backend::event::record_breaker_cleared(s, remaining, now);
    });
    log!(
        INFO,
        "[clear_liquidation_breaker] Breaker cleared (windowed total at clear: {} e8s)",
        remaining
    );
    Ok(())
}
```

- [ ] **Step 5.5: Update `rumi_protocol_backend.did`**

Edit `src/rumi_protocol_backend/rumi_protocol_backend.did`. Find `type ProtocolStatus = record { ... }` at line 552. Inside the record, add four new fields right after `deficit_readonly_threshold_e8s : nat64;`:

```
  breaker_window_ns : nat64;
  breaker_window_debt_ceiling_e8s : nat64;
  windowed_liquidation_total_e8s : nat64;
  liquidation_breaker_tripped : bool;
```

Then in the service block (search for `set_deficit_readonly_threshold_e8s : (nat64) -> (Result);`), insert three new methods alphabetically (the file appears to use alphabetical ordering inside the service block — verify by inspection):

```
  clear_liquidation_breaker : () -> (Result);
  set_breaker_window_debt_ceiling_e8s : (nat64) -> (Result);
  set_breaker_window_ns : (nat64) -> (Result);
```

- [ ] **Step 5.6: Build the canister to verify candid + rust types align**

Run: `cargo build -p rumi_protocol_backend --target wasm32-unknown-unknown --release`

Expected: clean build.

If candid_method drift is detected (a mismatch between the Rust signatures and the .did file), check that the new endpoints have `#[candid_method(update)]` and the fields match exactly.

- [ ] **Step 5.7: Run unit tests + audit POCs**

Run: `POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --lib`

Expected: 83 passed, 1 ignored.

Run: `POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --test audit_pocs_liq_008_circuit_breaker`

Expected: PASS, 11 tests.

- [ ] **Step 5.8: Commit**

```bash
git add src/rumi_protocol_backend/src/lib.rs src/rumi_protocol_backend/src/main.rs src/rumi_protocol_backend/rumi_protocol_backend.did
git commit -m "feat(api): wave-10 LIQ-008 breaker gate + ProtocolStatus + admin endpoints

check_vaults short-circuits both bot and SP notify pushes when the breaker
is tripped. ProtocolStatus exposes window, ceiling, live windowed total,
and tripped flag. Three admin endpoints (set window, set ceiling, clear)
plus their candid declarations. Manual liquidation endpoints unaffected."
```

---

## Task 6: Layer-3 PocketIC fence

**Files:**
- Create: `src/rumi_protocol_backend/tests/audit_pocs_liq_008_circuit_breaker_pic.rs`.

The fixture is structurally identical to `audit_pocs_liq_005_deficit_account_pic.rs` — same icRC-1 ledger deploy helper, same mock-XRC, same boot sequence. Differences:
- Extends `ProtocolStatusSubset` with the four LIQ-008 fields.
- Helpers: `set_breaker_window_ns_admin`, `set_breaker_window_debt_ceiling_e8s_admin`, `clear_liquidation_breaker_admin`, `liquidate_vault_caller`, `notify_liquidatable_via_check_vaults` (or just trigger via XRC tick + advance time).

Five tests, mirroring the Wave-8e PocketIC structure:

- [ ] **Step 6.1: Lift the fixture template from Wave-8e**

Copy `src/rumi_protocol_backend/tests/audit_pocs_liq_005_deficit_account_pic.rs` to `src/rumi_protocol_backend/tests/audit_pocs_liq_008_circuit_breaker_pic.rs` and replace:
- File-level docstring → describe Wave-10 LIQ-008 fence.
- `ProtocolStatusSubset` → add the four breaker fields after the existing LIQ-005 fields.
- All `liq_005_pic_*` test fn names → `liq_008_pic_*`.

Then strip the four LIQ-005-specific tests and leave only the helpers + boot.

- [ ] **Step 6.2: Add `liq_008_pic_breaker_trips_after_cumulative_threshold`**

```rust
#[test]
fn liq_008_pic_breaker_trips_after_cumulative_threshold() {
    let f = boot_protocol_with_one_vault();
    set_breaker_window_ns_admin(&f, 30 * 60 * NS_PER_SEC);
    // Configure the ceiling such that two ~50% partial liquidations of the
    // single underwater vault trip the breaker on the second.
    set_breaker_window_debt_ceiling_e8s_admin(&f, /* set after observing */ 80_000_000);

    drop_icp_price(&f, /* drop hard */);

    // First liquidation: assert breaker NOT yet tripped.
    liquidate_vault_partial_caller(&f, /* tx 1 */);
    let st1 = get_protocol_status_subset(&f);
    assert!(!st1.liquidation_breaker_tripped);
    assert!(st1.windowed_liquidation_total_e8s > 0);

    // Second liquidation crosses the ceiling.
    liquidate_vault_partial_caller(&f, /* tx 2 */);
    let st2 = get_protocol_status_subset(&f);
    assert!(st2.liquidation_breaker_tripped);
    assert!(st2.windowed_liquidation_total_e8s >= 80_000_000);
}
```

(Test author note: the exact `set_breaker_window_debt_ceiling_e8s` value depends on the boot fixture's vault size. Set it after capturing baseline `windowed_liquidation_total_e8s` from the first liquidation in a scratch run, then hard-code.)

- [ ] **Step 6.3: Add `liq_008_pic_manual_liquidation_still_works_after_trip`**

After tripping the breaker (call `set_breaker_window_debt_ceiling_e8s_admin` to a low value, then drop price + liquidate to trip), call `liquidate_vault` directly via the manual endpoint. Assert it succeeds and the vault's debt is cleared.

```rust
#[test]
fn liq_008_pic_manual_liquidation_still_works_after_trip() {
    let f = boot_protocol_with_two_vaults();
    set_breaker_window_ns_admin(&f, 30 * 60 * NS_PER_SEC);
    set_breaker_window_debt_ceiling_e8s_admin(&f, 1);

    drop_icp_price(&f);
    liquidate_vault_partial_caller(&f, vault_1, /* any partial */);
    assert!(get_protocol_status_subset(&f).liquidation_breaker_tripped);

    // Direct manual liquidation on vault_2 still succeeds.
    let result = call_liquidate_vault_full(&f, vault_2);
    assert!(result.is_ok(), "manual liquidation rejected after breaker trip: {:?}", result);
}
```

- [ ] **Step 6.4: Add `liq_008_pic_admin_clear_resumes_publishing`**

```rust
#[test]
fn liq_008_pic_admin_clear_resumes_publishing() {
    let f = boot_protocol_with_one_vault();
    set_breaker_window_ns_admin(&f, 30 * 60 * NS_PER_SEC);
    set_breaker_window_debt_ceiling_e8s_admin(&f, 1);

    drop_icp_price(&f);
    liquidate_vault_partial_caller(&f, /* tx 1 */);
    assert!(get_protocol_status_subset(&f).liquidation_breaker_tripped);

    clear_liquidation_breaker_admin(&f);
    assert!(!get_protocol_status_subset(&f).liquidation_breaker_tripped);
}
```

- [ ] **Step 6.5: Add `liq_008_pic_window_eviction_drops_old_entries`**

```rust
#[test]
fn liq_008_pic_window_eviction_drops_old_entries() {
    let f = boot_protocol_with_one_vault();
    set_breaker_window_ns_admin(&f, 60 * NS_PER_SEC);
    set_breaker_window_debt_ceiling_e8s_admin(&f, 0); // disable trip; we just want to test eviction

    drop_icp_price(&f);
    liquidate_vault_partial_caller(&f, /* records into log */);
    let total_before = get_protocol_status_subset(&f).windowed_liquidation_total_e8s;
    assert!(total_before > 0);

    // Advance time past the window. windowed_liquidation_total should drop
    // back to 0 because the read-side filter excludes the entry, even though
    // the prune-on-write hasn't fired (no new liquidation).
    f.pic.advance_time(Duration::from_secs(120));
    let total_after = get_protocol_status_subset(&f).windowed_liquidation_total_e8s;
    assert_eq!(total_after, 0);

    // The latch is still false (no trip happened — ceiling was 0).
    assert!(!get_protocol_status_subset(&f).liquidation_breaker_tripped);
}
```

- [ ] **Step 6.6: Add `liq_008_pic_upgrade_preserves_breaker_state`**

Mirrors `liq_005_pic_upgrade_preserves_deficit_state`. Set non-default values on all four fields (window, ceiling, log non-empty, tripped flag). Upgrade via `dfx-style upgrade arg`. Re-read `ProtocolStatusSubset` and assert all four values match.

```rust
#[test]
fn liq_008_pic_upgrade_preserves_breaker_state() {
    let f = boot_protocol_with_one_vault();
    set_breaker_window_ns_admin(&f, 12 * 60 * NS_PER_SEC); // 12 min, not default
    set_breaker_window_debt_ceiling_e8s_admin(&f, 1);

    drop_icp_price(&f);
    liquidate_vault_partial_caller(&f, /* trips */);
    let pre = get_protocol_status_subset(&f);
    assert!(pre.liquidation_breaker_tripped);
    assert_eq!(pre.breaker_window_ns, 12 * 60 * NS_PER_SEC);
    assert_eq!(pre.breaker_window_debt_ceiling_e8s, 1);

    upgrade_protocol(&f);

    let post = get_protocol_status_subset(&f);
    assert!(post.liquidation_breaker_tripped, "tripped flag must survive upgrade");
    assert_eq!(post.breaker_window_ns, 12 * 60 * NS_PER_SEC);
    assert_eq!(post.breaker_window_debt_ceiling_e8s, 1);
    // windowed_liquidation_total_e8s is timestamp-relative; the upgrade
    // doesn't advance time so it should equal pre.
    assert_eq!(post.windowed_liquidation_total_e8s, pre.windowed_liquidation_total_e8s);
}
```

- [ ] **Step 6.7: Run the LIQ-008 PocketIC suite**

Run: `POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --test audit_pocs_liq_008_circuit_breaker_pic`

Expected: 5 tests pass.

- [ ] **Step 6.8: Run the LIQ-005 PocketIC suite to confirm no regression**

Run: `POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --test audit_pocs_liq_005_deficit_account_pic`

Expected: 4 tests pass (matches Wave-8e baseline).

- [ ] **Step 6.9: Run the broad PocketIC suite to confirm no regression**

Run: `POCKET_IC_BIN=/Users/robertripley/coding/rumi-protocol-v2/pocket-ic cargo test -p rumi_protocol_backend --test pocket_ic_tests`

Expected: 27 passed, 2 ignored (matches the pre-Wave-10 baseline).

- [ ] **Step 6.10: Commit**

```bash
git add src/rumi_protocol_backend/tests/audit_pocs_liq_008_circuit_breaker_pic.rs
git commit -m "test(audit): wave-10 LIQ-008 PocketIC fence

Five tests covering trip on cumulative threshold, manual liquidation open
after trip, admin clear resumes publishing, window eviction drops old
entries, upgrade preserves all four breaker fields. Fixture lifted from
Wave-8e LIQ-005 PocketIC fence."
```

---

## Task 7: Candid declarations regen + push + PR + deploy

**Files:**
- Modify: `src/declarations/rumi_protocol_backend/*` (regenerated by `dfx generate`).

- [ ] **Step 7.1: Regenerate declarations**

Run: `dfx generate rumi_protocol_backend`

Inspect: `src/declarations/rumi_protocol_backend/rumi_protocol_backend.did.js` and `*.d.ts` should reflect the four new ProtocolStatus fields and the three new endpoints.

- [ ] **Step 7.2: Commit declarations**

```bash
git add src/declarations/rumi_protocol_backend/
git commit -m "chore(declarations): regen for wave-10 LIQ-008 candid"
```

- [ ] **Step 7.3: Push and open PR**

```bash
git push -u origin fix/audit-wave-10-liq-008-circuit-breaker
gh pr create --title "Wave 10: LIQ-008 mass-liquidation circuit breaker" --body "$(cat <<'EOF'
## Summary
- Adds rolling-window cumulative-debt circuit breaker to gate `check_vaults` auto-publishing of underwater vaults to bot/SP.
- Manual liquidation endpoints stay open under all conditions.
- Sticky tripped latch (T2): admin must call `clear_liquidation_breaker` to resume.
- Defaults: 30-minute window, ceiling = 0 (disabled). Operator sets ceiling after observing baseline `windowed_liquidation_total_e8s` post-deploy.

## Audit reference
LIQ-008 (LOW) from `audit-reports/2026-04-22-28e9896/raw-pass-results/liquidation-mechanics.json`. Slotted after Wave 8e (LIQ-005) so the deficit account is in place when the breaker trips.

## Test plan
- [x] `cargo test -p rumi_protocol_backend --test audit_pocs_liq_008_circuit_breaker` (Layer 1+2: 11 tests)
- [x] `cargo test -p rumi_protocol_backend --test audit_pocs_liq_008_circuit_breaker_pic` (Layer 3: 5 tests)
- [x] `cargo test -p rumi_protocol_backend --lib` (83 passed, 1 ignored — no regression)
- [x] `cargo test -p rumi_protocol_backend --test audit_pocs_liq_005_deficit_account_pic` (4 passed — no regression)
- [x] `cargo test -p rumi_protocol_backend --test pocket_ic_tests` (27 passed, 2 ignored — no regression)

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 7.4: Merge with `--merge` (no squash) per project standing rule**

```bash
gh pr merge --merge
```

- [ ] **Step 7.5: Deploy to mainnet**

```bash
dfx identity use rumi_identity
dfx deploy rumi_protocol_backend --network ic --argument '(variant { Upgrade = record { mode = null; description = opt "Wave-10 LIQ-008: mass-liquidation circuit breaker (rolling-window debt ceiling + admin-clear latch)" } })'
```

- [ ] **Step 7.6: Smoke test post-deploy**

```bash
dfx canister --network ic call rumi_protocol_backend get_protocol_status '()' --query
```

Confirm:
- `breaker_window_ns = 1_800_000_000_000` (30 min default)
- `breaker_window_debt_ceiling_e8s = 0` (disabled — operator must set after baseline)
- `windowed_liquidation_total_e8s = 0`
- `liquidation_breaker_tripped = false`

```bash
dfx canister --network ic info rumi_protocol_backend
```

Confirm module hash changed off `0x3e58a322ff602912ee3ad5546b33f185ca9ba496f1b5c37b056e72342ebe46d9`. Record the new hash for the bake-watch memory entry.

- [ ] **Step 7.7: 24-hour bake watch**

For 24h after deploy, periodically check `dfx canister --network ic logs rumi_protocol_backend | grep LIQ-008` for unexpected `[LIQ-008] circuit breaker tripped` lines. With ceiling = 0 the breaker cannot trip, so any LIQ-008 log line in this period is either:
- Informational from `record_recent_liquidation` (no log emitted at this level) — should be empty, OR
- The `[LIQ-008] check_vaults skipping notify (breaker tripped)` line — which would indicate the latch was somehow set by something other than admin call (a bug). Investigate immediately if seen.

Once 24h baseline observed, calibrate the ceiling and call `set_breaker_window_debt_ceiling_e8s`.

---

## After Wave 10

- LIQ-006 + LIQ-007 are CLOSED per Wave 0 verification — no follow-up needed.
- BOT-001 (`bot_auto_cancel_safety_gap`) — separate wave (rare stuck-vault scenario).
- Wave 11+ picks up remaining LOW/INFO findings.

---

## Self-review checklist

- [x] Spec coverage: every audit recommendation in LIQ-008 maps to a task.
  - "Track liquidations per rolling window (e.g. 30 min)" → Tasks 1, 2, 4
  - "If cumulative liquidated debt exceeds X% of TVL within the window, pause `notify_liquidatable_vaults`" → Task 5 (gate in check_vaults)
  - "manual liquidation still available" → Task 5 (the gate is in check_vaults only; manual endpoints in main.rs are untouched). Verified by Task 6.3 PocketIC fence.
  - "raise a protocol alert" → BreakerTripped event emission in Task 3 + 5
  - "Optionally bias toward smaller partials during stress by shrinking `recommended_liquidation_amount`" → DEFERRED (locked design decision; documented as Wave 11+ candidate)
- [x] Placeholder scan: zero TBD/TODO/"add appropriate validation" placeholders. The PIC test author-notes ("set after observing") are by design — the boot fixture's vault size determines the threshold.
- [x] Type consistency: `record_recent_liquidation(state, debt_e8s, now_ns)` signature is consistent across Tasks 2, 4. `windowed_liquidation_total(now_ns)` consistent across Tasks 2, 5, 6. Event variant field names (`total_e8s`, `ceiling_e8s`, `remaining_total_e8s`, `window_ns`) consistent across event variants and recorder helpers. `breaker_window_ns` / `breaker_window_debt_ceiling_e8s` / `liquidation_breaker_tripped` field names consistent across state.rs, lib.rs ProtocolStatus, main.rs population, .did file, and PocketIC test fixtures.
