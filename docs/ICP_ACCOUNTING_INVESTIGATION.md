# ICP Accounting Investigation & Fix Report

**Date:** March 7, 2026
**Commits:** `d7ec53c` (sweep function + hardening), `1bdf5c0` (phantom fix)
**Status:** Resolved. Books balanced to the e8s.

---

## Summary

An ICP accounting discrepancy was discovered when the newly deployed `admin_sweep_to_treasury` function reported no surplus to sweep. The canister's actual ICP balance (289.37 ICP) was significantly less than its tracked obligations (297.64 ICP) — an 8.27 ICP gap.

A full forensic investigation cross-referencing on-chain event logs with ICP ledger transactions (via Rosetta API) identified three root causes, all tracing back to the early canister reinstall:

1. **Phantom `pending_margin_transfers`** (5.62 ICP) — a bug in `state.close_vault()`
2. **Phantom vault collateral** (1.13 ICP) — vaults opened during reinstall with no real ICP deposit
3. **Untracked historical outflows** (1.66 ICP) — early transfers without event tracking

All issues have been fixed and the books now balance exactly: **actual = tracked = 289.37 ICP**.

---

## Investigation Methodology

### Data Sources

1. **Event log** — ~5,000 Candid-encoded events from stable storage (`get_event_log`), covering all protocol state changes since inception
2. **ICP Ledger (Rosetta API)** — Complete transaction history for the backend canister account (`460bb50324a09ff9501f9efb63f262c8c89d010162e90a3068e478713d524697`), totaling 164 transactions
3. **Source code** — `state.rs`, `vault.rs`, `event.rs`, `management.rs`

### Approach

- Parsed all event types to compute expected ICP inflows and outflows
- Fetched all 164 ledger transactions and computed actual inflows/outflows
- Cross-referenced every outbound ledger transaction against events by `block_index`
- Identified mismatches between event-recorded amounts and ledger reality

---

## Root Cause 1: Phantom `pending_margin_transfers` (5.62 ICP)

### The Bug

`state.rs:close_vault()` inserted a `pending_margin_transfer` entry every time a vault was closed:

```rust
pub fn close_vault(&mut self, vault_id: u64) {
    if let Some(vault) = self.vault_id_to_vaults.remove(&vault_id) {
        self.pending_margin_transfers.insert(
            vault_id,
            PendingMarginTransfer {
                owner,
                margin: ICP::from(vault.collateral_amount), // BUG
                collateral_type: vault.collateral_type,
            },
        );
        // ...
    }
}
```

This was **never correct**:

- **`CloseVault`** (simple close) requires `collateral_amount = 0` as a precondition, so the entry always had 0 margin — useless but harmless.
- **`WithdrawAndCloseVault`** already transferred collateral directly to the owner before calling `close_vault()`. The pending entry was phantom — no `MarginTransfer` event was ever recorded to clear it.

### Compounding Factor: Event Replay

The `CollateralWithdrawn` event replay handler was a no-op:

```rust
Event::CollateralWithdrawn { vault_id, .. } => {
    // The vault's margin has already been set to 0 in the vault.rs function
}
```

During live operation, `vault.rs` zeros the vault's collateral before the transfer. But during **event replay** (on canister upgrade), this zeroing never happens. So when `WithdrawAndCloseVault` replay subsequently calls `close_vault()`, the vault still has its full collateral amount, creating a phantom pending entry with real ICP value.

### Impact

11 vaults accumulated phantom pending entries totaling 5.62 ICP:

| Vault ID | Phantom Amount (ICP) |
|----------|---------------------|
| 15       | 0.0500              |
| 25       | 0.1000              |
| 27       | 0.7215              |
| 29       | 0.0500              |
| 31       | 0.0500              |
| 32       | 0.8170              |
| 35       | 0.0300              |
| 37       | 0.1000              |
| 38       | 0.3400              |
| 39       | 0.3643              |
| 42       | 3.0000              |

None of these had corresponding `MarginTransfer` events. All had `WithdrawAndCloseVault` events confirming collateral was already returned directly.

### Fix

**Commit `1bdf5c0`:**

1. Removed the `pending_margin_transfers.insert()` from `close_vault()`. Legitimate pending transfers (for liquidator rewards) are created directly in `vault.rs` liquidation code, not here.
2. Fixed `CollateralWithdrawn` replay to properly zero the vault's collateral, mirroring what `vault.rs` does during live operation.

After upgrade, event replay no longer creates phantom entries. The 5.62 ICP of inflated obligations disappeared.

---

## Root Cause 2: Phantom Vault Collateral (1.13 ICP)

### The Problem

10 vaults had `block_index = 0` in their `OpenVault` events — impossible for a real ICP ledger transaction (current blocks are in the tens of millions).

| Vault IDs | Collateral (ICP) | Explanation |
|-----------|-----------------|-------------|
| #19–#25   | 1.13 total      | Reinstall-era: vaults recreated with no real ICP deposit |
| #30–#32   | 2.97 total      | Push-deposit flow: real ICP arrived via sweep, event recorded block 0 instead of actual sweep block |

**Vaults #30–#32** had real ICP backing — their sweep transactions appeared as "untracked inbound" on the ledger, matching exactly. The block index was just recorded wrong.

**Vaults #19–#25** had no matching ledger inbound transactions. The 1.13 ICP they claimed as collateral was never deposited — these were phantom entries from the canister reinstall recovery.

### What Happened to Each

| Vault | Collateral | Status | Effect |
|-------|-----------|--------|--------|
| #19   | 0.10 ICP  | Closed | 0.10 ICP paid out from pool (phantom) |
| #20   | 0.10 ICP  | Liquidated | 0.10 ICP seized by liquidator (phantom) |
| #21   | 0.10 ICP  | Closed | 0.10 ICP paid out from pool (phantom) |
| #22   | 0.09 ICP  | Closed | 0.09 ICP paid out from pool (phantom) |
| #23   | 0.08 ICP  | Closed | 0.08 ICP paid out from pool (phantom) |
| #24   | 0.56 ICP  | **Open** | Still inflating tracked obligations |
| #25   | 0.10 ICP  | Closed | 0.10 ICP paid out from pool (phantom) |

All vaults belonged to the developer principal. Phantom payouts came from and returned to the same depositor.

### Resolution

Vault #24's phantom collateral (0.56 ICP) was absorbed into the vault #33 correction (see below). No separate correction was needed since the developer owns all affected vaults.

---

## Root Cause 3: Untracked Historical Outflows (1.66 ICP)

### The Problem

Cross-referencing all 59 outbound ledger transactions with recorded events revealed 17 transfers (totaling 1.66 ICP) with no matching event. These were early liquidation rewards, test transfers, and protocol fee movements from before event tracking was comprehensive.

### Resolution

These outflows were already reflected in the actual balance — the ICP left the canister long ago. The deficit was absorbed into the vault #33 correction.

---

## Ledger Reconciliation

### ICP Inflows

| Source | Amount (ICP) |
|--------|-------------|
| Ledger inbound transactions | 315.16 |
| Event-recorded deposits (excl. block_index=0) | 312.19 |
| Matched push-deposit sweeps (block_index=0 with real ledger match) | 2.97 |
| **Total reconciled inflows** | **315.16** |

### ICP Outflows

| Source | Amount (ICP) |
|--------|-------------|
| Ledger outbound transactions | 25.78 |
| Event-tracked (withdraw_and_close + partial_withdrawal + margin_transfer) | 24.12 |
| Untracked historical outflows | 1.66 |
| **Total reconciled outflows** | **25.78** |

### Fee Accounting

ICRC-2 `transfer_from` fees are paid by the backend canister (the spender), not deducted from the deposit amount. ~150 inbound transactions at 0.0001 ICP each = 0.015 ICP in fees — negligible and correctly reflected in the actual balance.

---

## Final Correction

After deploying the phantom pending fix (commit `1bdf5c0`), the remaining gap was:

```
actual:  289,374,967.02 e8s
tracked: 292,018,192.19 e8s
gap:     2,643,225.17 e8s (2.64 ICP)
```

This 2.64 ICP gap represents:
- 0.56 ICP: vault #24 phantom collateral (still open)
- 0.47 ICP: closed phantom vault payouts (#19, #21–#23, #25)
- 0.10 ICP: liquidated phantom vault #20
- 1.51 ICP: untracked historical outflows + transfer fees

Since all affected vaults belong to the developer, the gap was absorbed by reducing vault #33's collateral by 264,322,517 e8s (from 88.00 ICP to 85.36 ICP) via `admin_correct_vault_collateral`.

Post-correction verification:

```
actual:  28,937,496,702 e8s
tracked: 28,937,496,702 e8s
gap:     0
```

**Books balanced to the e8s.**

---

## Lessons Learned

### 1. Never Reinstall a Canister

Every issue in this investigation traces back to the early canister reinstall. Reinstalling wipes stable storage (events, state, ID counters), creating:
- Phantom collateral from state reconstruction
- Vault ID collisions (vault #16, documented separately)
- Orphaned ICP from lost event history

**Always upgrade. Never reinstall.** This rule is documented in CLAUDE.md and the project's auto-memory.

### 2. Event Replay Must Mirror Live Operations

The `CollateralWithdrawn` replay was a no-op because "vault.rs already handles it." But vault.rs only runs during live operation — during replay, only event handlers execute. Every event handler must independently produce correct state.

### 3. `close_vault()` Should Be Minimal

State mutation functions like `close_vault()` should do the minimum: remove the vault from maps and indexes. Side effects like creating pending transfers should be explicit at the call site, not hidden inside a generic close function.

### 4. Forensic Tooling Matters

The `admin_sweep_to_treasury` function (which surfaced this issue) and `get_event_log` (which enabled the investigation) were essential. Without the ability to compare actual balance vs tracked obligations, this discrepancy would have remained hidden.

---

## Files Changed

| File | Change |
|------|--------|
| `src/rumi_protocol_backend/src/state.rs` | Removed phantom `pending_margin_transfer` insertion from `close_vault()` |
| `src/rumi_protocol_backend/src/event.rs` | Fixed `CollateralWithdrawn` replay to zero vault collateral |

## On-Chain Corrections

| Action | Vault | Before | After | Reason |
|--------|-------|--------|-------|--------|
| `admin_correct_vault_collateral` | #33 | 8,800,000,000 e8s | 8,535,677,483 e8s | Absorb 2.64 ICP reinstall-era deficit |
