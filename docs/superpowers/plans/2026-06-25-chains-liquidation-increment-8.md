# Chains Liquidation Increment 8: Retry-Safe SP Chain Absorb Foundation

## Goal

Make Stability Pool absorption of escalated chain vaults safe to retry and observable before enabling any unattended auto-burn timer.

Increment 7 repairs bot-failed liquidation markers and escalates timed-out bot work to `sp_attempted`. Increment 8 must not naively loop over those vaults: the current SP absorb path burns IC-native icUSD before the backend call, and a fresh ledger `created_at_time` on retry can create a second burn.

## Scope

- Add SP-side chain absorb candidate discovery for already-escalated (`sp_attempted = true`) vaults.
- Add durable SP-side chain absorb intent state keyed by vault id.
- Persist the intended icUSD amount, chain/vault metadata, burn memo timestamp, minting account, optional burn proof, optional backend result, and last error.
- Reuse the persisted ledger timestamp and proof on retries so one vault intent cannot produce a second burn.
- Keep pool balance/opt-in mutations blocked while a burned-but-unfinalized chain absorb intent exists.
- Make local SP finalization idempotent by vault id so a resumed backend result does not double-deduct depositor icUSD or double-credit CFX claims.
- Expose pending/completed chain absorb state for operators.

## Non-Goals

- No unattended timer auto-burn yet.
- No automatic foreign-chain representation retirement.
- No CFX payout retry/rollback automation.
- No standalone backend result-recovery query; exact same-proof retries return the stored backend result from `stability_pool_liquidate_chain_vault`.

## Safety Invariant

For a given chain vault id, the SP may create at most one IC-native icUSD burn intent, with one stable `created_at_time` and one accepted burn proof. Any retry must resume that intent; it must not recompute the burn timestamp or draw a new live depositor denominator after the burn.

## Acceptance

- Tests prove candidate scanning filters to `sp_attempted` vaults with full icUSD coverage.
- Tests prove pending intent decode is upgrade-compatible from pre-Inc-8 snapshots.
- Tests prove an existing pending intent reuses the stored `created_at_time` and rejects conflicting recomputation.
- Tests prove completed local finalization is idempotent and does not double-deduct depositor balances or double-credit CFX claims.
- Tests prove balance/opt-in mutation guards observe pending chain absorbs.
- Tests prove backend same-proof result replay accepts only the exact same caller/vault/amount/burn proof shape and rejects conflicting proof reuse.
- Tests prove completed SP absorb journals and backend replay-result caches are bounded.
- Tests prove post-await deposit credits recheck the pending-absorb guard before mutating balances.
- Tests prove replay-cache retention never evicts the result that was just accepted.
- Tests prove burn-watch defers user foreign-chain burns while a vault is SP-escalated.
- Tests prove backend SP absorb rejects stale burn amounts that no longer match live debt without mutating chain debt, pending burn, supply, or claims.
- Tests prove SP local finalization rejects partial backend results without deducting depositor icUSD or crediting CFX claims.
- Tests prove SP chain absorb refuses to start while a deduct-before-transfer balance operation is awaiting a ledger response, so failed withdrawal rollback cannot restore stable balances during a pending chain absorb.
- Tests prove a pending burned chain absorb blocks new vault absorb planning until that original intent is retried/finalized, so the same still-recorded icUSD balance cannot be burned twice.
- Full relevant Stability Pool and backend tests pass.
