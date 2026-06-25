# Chains Liquidation Increment 7 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire automatic bot-to-Stability-Pool escalation for stale chain liquidation swaps, without automatically burning SP depositor icUSD.

**Architecture:** Keep the burn/absorb path SP-owned and opt-in gated. The backend observer will only perform the missing state transition for bot liquidations that have timed out: fail the bot swap op, restore reserved collateral, clear `pending_liquidation`, remove `bot_pending_chain_vaults`, and set `sp_attempted_chain_vaults`. The existing SP pull path then discovers the vault with `sp_attempted = true` and performs its current admin/SP-owned burn-proof flow.

**Tech Stack:** Rust 2021, `ic-cdk` timers/update methods, existing EVM observer and settlement queue, Candid-compatible persisted state, Cargo unit tests.

## Global Constraints

- TDD is mandatory: write failing tests first, run them red, then implement the smallest change that turns them green.
- Do not add automatic SP burns in this increment. A timer must not repeatedly burn IC-native icUSD if the backend rejects after a ledger burn.
- Do not change the SP opt-in model: CFX remains explicit opt-in and `prepare_chain_absorb_plan_in_state` remains the capacity gate.
- Preserve the chain supply invariant. Timeout escalation only changes routing/marker/collateral reservation state and settlement op status; it must not move debt, reserve backing, pending burn, or chain supply.
- Do not re-route `sp_attempted` vaults back to the bot.
- Do not add a new state version unless tests prove an additive persisted field is required. The intended implementation reuses existing V6 fields.
- Use `icp`, not `dfx`, for build/deploy instructions.
- Do not add `@rollup/rollup-darwin-*` to any `package.json`.

---

### Task 1: Backend Timeout Escalation State Helper

**Files:**
- Modify: `src/rumi_protocol_backend/src/chains/liquidation.rs`

**Interfaces:**
- Consumes:
  - `MultiChainState::bot_pending_chain_vaults`
  - `ChainVaultV1::pending_liquidation`
  - `SettlementQueueV1` / `SettlementOpStatus`
  - existing `should_escalate_to_sp`
- Produces:
  - `pub const DEFAULT_BOT_TO_SP_TIMEOUT_NS: u64`
  - `pub struct ChainBotSpEscalation { pub vault_id: u64, pub op_id: u64, pub reason: String }`
  - `pub fn escalate_timed_out_bot_liquidations_in_state(state: &mut MultiChainState, chain: ChainId, now_ns: u64, bot_timeout_ns: u64, max_per_tick: usize) -> Vec<ChainBotSpEscalation>`

- [x] **Step 1: Write failing timeout escalation tests**

Add tests in `src/rumi_protocol_backend/src/chains/liquidation.rs` proving:

```rust
#[test]
fn timeout_escalation_restores_collateral_fails_op_and_sets_sp_attempted() {
    let mut s = MultiChainState::default();
    seed_cfg_unchecked(&mut s);
    insert_vault_liq(&mut s, 7, 1_400, 100, ChainVaultStatus::Open);
    let op_id = enqueue_bot_liquidation_swap(&mut s, 7, 10);
    mark_bot_liquidation_reserved(&mut s, 7, op_id, 100 * E18, 10);

    let escalated = escalate_timed_out_bot_liquidations_in_state(
        &mut s,
        ChainId(71),
        10 + DEFAULT_BOT_TO_SP_TIMEOUT_NS,
        DEFAULT_BOT_TO_SP_TIMEOUT_NS,
        10,
    );

    assert_eq!(escalated.len(), 1);
    assert_eq!(escalated[0].vault_id, 7);
    assert_eq!(escalated[0].op_id, op_id);
    assert!(s.sp_attempted_chain_vaults.contains(&7));
    assert!(!s.bot_pending_chain_vaults.contains_key(&7));
    let v = s.chain_vaults.get(&7).unwrap();
    assert!(v.pending_liquidation.is_none());
    assert_eq!(v.collateral_amount_native, 1_400 * E18);
    let op = s.settlement_queues.get(&ChainId(71)).unwrap().pending.get(&op_id).unwrap();
    assert!(matches!(op.status, SettlementOpStatus::Failed { .. }));
}
```

Also add tests proving no mutation before timeout and no double-restore when the op is already `Succeeded`.

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rumi_protocol_backend timeout_escalation --lib -- --nocapture`

Observed: FAIL because `DEFAULT_BOT_TO_SP_TIMEOUT_NS` and
`escalate_timed_out_bot_liquidations_in_state` did not exist.

- [x] **Step 3: Implement the helper**

Implementation rules:

- Collect candidates in a read-only pass from `bot_pending_chain_vaults`.
- Candidate must be on the requested chain and have a `pending_liquidation` marker with `tier == LiquidationTier::Bot`.
- Candidate escalates when `should_escalate_to_sp(bot_pending_since_ns, now_ns, bot_timeout_ns, op_failed)` returns true.
- Treat `SettlementOpStatus::Failed` as terminal. Treat `SettlementOpStatus::Queued` as timeout-eligible. Treat missing/pruned ops as timeout-eligible repair cases. Treat `SettlementOpStatus::Inflight` and `SettlementOpStatus::Succeeded` as ineligible to avoid racing a live submit/confirm or double-restoring an already-settled bot liquidation.
- For an eligible candidate, restore `collateral_reserved_native`, clear the marker, remove `bot_pending_chain_vaults`, insert `sp_attempted_chain_vaults`, and mark the settlement op failed with a timeout/escalation reason if it still exists.
- Return one `ChainBotSpEscalation` per actual mutation. Respect `max_per_tick`.

- [x] **Step 4: Run task tests**

Run: `cargo test -p rumi_protocol_backend timeout_escalation --lib -- --nocapture`

Observed: PASS, 5 passed after adding the inflight-swap and missing-op repair regressions from adversarial review.

### Task 2: Observer Wiring and Audit Event Emission

**Files:**
- Modify: `src/rumi_protocol_backend/src/chains/evm/deposit_watch.rs`
- Test: `src/rumi_protocol_backend/src/chains/liquidation.rs`

- [x] **Step 1: Wire the observer**

In the chain liquidation detection block, call `escalate_timed_out_bot_liquidations_in_state` before routing fresh bot candidates. For each returned escalation, record:

```rust
crate::storage::record_event(&crate::event::Event::ChainSettlementFailed {
    chain_id: chain,
    op_id: escalation.op_id,
    reason: escalation.reason.clone(),
    timestamp: now_ns,
});
```

Then continue existing price freshness and routing behavior. Stale price should still emit `ChainLiquidationDeferred`, but timeout escalation does not need a fresh price because it only restores already-reserved collateral and hands the vault to the existing SP/manual path.

- [x] **Step 2: Run focused observer-adjacent tests**

Run: `cargo test -p rumi_protocol_backend timeout_escalation --lib -- --nocapture`

Observed: PASS.

### Task 2b: Settlement Submit CAS For Queued Swap Race

**Files:**
- Modify: `src/rumi_protocol_backend/src/chains/evm/settlement.rs`
- Test: `src/rumi_protocol_backend/src/chains/evm/tests_settlement.rs`

- [x] **Step 1: Fix reviewer blocker**

Adversarial reviewers found that observer timeout escalation could race a
settlement worker that had cloned a still-`Queued` `LiquidationSwap` before
awaiting RPC/sign/broadcast. The fix adds
`claim_liquidation_swap_submit_in_state`, which atomically verifies the live op
is still `Queued`, the live vault marker still belongs to that op, and then
marks the op `Inflight` with local tx hash + nonce immediately before the
broadcast await. If the observer already cleared the marker or another worker
claimed the op, submit aborts before broadcasting.

- [x] **Step 2: Add focused CAS tests**

Observed:

- `cargo test -p rumi_protocol_backend timeout_escalation --lib -- --nocapture`: PASS, 5 passed.
- `cargo test -p rumi_protocol_backend claim_liquidation_swap_submit --lib -- --nocapture`: PASS, 3 passed.

### Task 3: Verification and Review

**Files:**
- Modify: `docs/superpowers/plans/2026-06-24-chains-liquidation-increment-7.md`

- [x] **Step 1: Run deterministic checks**

Run:

```bash
cargo test -p rumi_protocol_backend timeout_escalation --lib -- --nocapture
cargo test -p rumi_protocol_backend detect_skips_bot_failed_sp_attempted_vaults --lib -- --nocapture
cargo test -p rumi_protocol_backend --lib
cargo test -p rumi_protocol_backend --bin rumi_protocol_backend
cargo test -p rumi_protocol_backend check_candid_interface_compatibility --bin rumi_protocol_backend -- --nocapture
git diff --check
```

Observed:

- `cargo test -p rumi_protocol_backend timeout_escalation --lib -- --nocapture`: PASS, 5 passed.
- `cargo test -p rumi_protocol_backend claim_liquidation_swap_submit --lib -- --nocapture`: PASS, 3 passed.
- `cargo test -p rumi_protocol_backend detect_skips_bot_failed_sp_attempted_vaults --lib -- --nocapture`: PASS, 1 passed.
- `cargo test -p rumi_protocol_backend --lib`: PASS, 663 passed, 1 ignored.
- `cargo test -p rumi_protocol_backend --bin rumi_protocol_backend`: PASS, 16 passed.
- `cargo test -p rumi_protocol_backend check_candid_interface_compatibility --bin rumi_protocol_backend -- --nocapture`: PASS, 1 passed.
- `git diff --check`: PASS.

- [x] **Step 2: Adversarial verification**

Run two independent reviewers against:

- Diff for `liquidation.rs`, `deposit_watch.rs`, and this plan.
- Deterministic test output.
- Rubric:
  - Timeout escalation cannot double-restore collateral.
  - Timeout escalation cannot re-route an SP-attempted vault to the bot.
  - Timeout escalation does not move debt, chain supply, reserve backing, or pending burn.
  - Existing SP burn/absorb remains manual/SP-owned and opt-in gated.
  - Rejection/stale-price paths do not wedge a bot marker forever.

Observed: two independent reviewers found and then cleared the race/liveness
issues after fixes:

- Blocker 1: observer timeout escalation could restore collateral for an
  `Inflight` swap that might still confirm. Fixed by skipping `Inflight` in the
  observer helper and adding `timeout_escalation_skips_inflight_swap_to_avoid_live_submit_race`.
- Blocker 2: observer timeout escalation could race a settlement worker holding
  a stale cloned `Queued` op across awaits. Fixed by
  `claim_liquidation_swap_submit_in_state`, which claims the live op as
  `Inflight` with local tx hash + nonce immediately before broadcast.
- Liveness finding: missing/pruned failed swap ops could leave markers wedged.
  Fixed by treating missing ops as timeout-eligible repair cases and adding
  `timeout_escalation_repairs_missing_swap_op_after_timeout`.

Final reviewer status: no blockers. Residual risks are bounded to manual repair
for corrupted marker-only state without `bot_pending_chain_vaults`, and to
normal confirm-timeout liveness for already-`Inflight` swaps.

- [ ] **Step 3: Open PR**

Run:

```bash
git status --short
git add -f docs/superpowers/plans/2026-06-24-chains-liquidation-increment-7.md
git add src/rumi_protocol_backend/src/chains/liquidation.rs src/rumi_protocol_backend/src/chains/evm/deposit_watch.rs
git commit -m "feat(chains): Inc7 auto-escalate timed-out bot liquidations"
git push -u origin codex/chains-liquidation-inc7
gh pr create --title "feat(chains): Inc7 auto-escalate timed-out bot liquidations" --body-file /tmp/inc7-pr-body.md
```

## Notes For Future Increment

SP-side auto absorb is still valuable, but it needs a durable burn-retry ledger or a backend/SP two-phase lock before a timer can safely burn IC-native icUSD. The current `burn_icusd_for_chain_writedown` uses a fresh `created_at_time` per call, so a naive retry loop could burn more than once if the backend rejects after the first ledger burn.
