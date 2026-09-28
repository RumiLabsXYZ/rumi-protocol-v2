# Redemption run execution and legacy replay preservation — review proposal only

Status: review artifact; no event recorder or legacy-replay source changes applied. The planner's state file contains the previously authorized queue/simulator work and one independent simulator/executor precision test; this proposal does not authorize a source bypass. The event/replay patch must be submitted as one complete direct patch after reviewer inspection.

## Review history and blockers

The direct `event.rs` change was rejected twice by automatic review. The first review found persisted-event replay/payout changes with financial mispayment risk, a missing `PendingMarginTransfer.min_net_collateral_raw` dependency, and a stored floor that was not enforced. The later review still rejected the persisted event and native-unit payout change across collateral types, citing the need for validated end-to-end tests and explicit approval of this implementation. It also prohibited workarounds or indirect source changes. Rob later explicitly approved the event and reserve proposals, but that did not override the subsequent automatic-review block. This proposal keeps that outcome visible and calls for one complete, directly reviewed patch.

## Required paired source shape

The compatibility problem is that old `RedemptionOnVaults` events with `vault_redemptions: None` replay by re-running `State::redeem_on_vaults`. The current proposed planner wrapper restricts that method to the first globally health-ranked run. That changes old events whose requested collateral type was not the global first run. The committed baseline also used `Decimal::from_f64` for cached prices; replacing it with `from_f64_retain` changes native raw-unit rounding for some 18-decimal fractional-price cases.

The complete patch should preserve two intentionally separate algorithms:

1. **Historical replay only:** add an explicitly named `State::redeem_on_vaults_legacy_full_type_for_replay` by copying the exact committed baseline body from `HEAD:src/rumi_protocol_backend/src/state.rs` and renaming only the function. Do not delegate to `redeem_on_vaults_with_vault_ids`, even with filter flags disabled: that helper has different scan/sort/eligibility behavior. Preserve the old requested-collateral-type scan, CR ordering/tie behavior, zero-debt and existing bot/operation-lock exclusions, price fallback, `Decimal::from_f64` rounding, and absence of current status/native-XRP eligibility filters. It must not call `redemption_runs()` or use global ranking. The only production call site must be the `Event::RedemptionOnVaults` branch for `vault_redemptions: None`.
2. **New live execution only:** all quoted, direct, and reserve/spillover redemptions use a run-scoped recorder whose API requires `allowed_vault_ids: &[VaultId]`; it delegates only to `redeem_on_vaults_for_vault_ids`. No production call site may use a collateral-type-only recorder. Newly written events must store `vault_redemptions: Some(outcomes)` even when `outcomes` is empty, so a new no-op cannot be mistaken for a legacy `None` event during replay. Legacy `None` is reserved for already persisted events. Include the already approved reserve/spillover hunk in this same coherent patch, with the exact run's ID slice; do not leave one production collateral-only call behind.

Do not widen the historical helper to any new live caller. The reviewer should inspect the complete call graph, including reserve spillover, and confirm the one replay-only call edge plus required-ID live APIs.

## Persisted payout and minimum behavior in the same patch

For new events, record the exact actual native collateral seized (sum of post-clamp `VaultRedemption.collateral_seized`) and use that same raw amount for the pending transfer. Aggregate in `u128`; checked-convert to the stored/ledger `u64` payout and fail before pulling/burning funds when not representable. Persist the optional minimum in the new event and pending transfer with `#[serde(default)]`, and enforce the minimum against actual net payout before payout. Old serialized events and pending transfers without the field decode to `None` and retain prior behavior. Do not silently revise historical payout claims during replay.

## Regression oracle required before direct review

The independent backend reviewer found the following additional gates required for a safe complete patch:

- Quoted/direct paths must compute the simulated seized aggregate in checked `u128`, checked-convert it to `u64`, and calculate `net = gross - current ledger_fee` inside the same mutation closure. If representability or the minimum fails, reject/refund before fee-base, event, or debt mutation. If refreshed post-pull state changes the result, take the same no-mutation full-refund path. A pending processor encountering a changed fee must retain the claim and send nothing when `net < stored minimum`.
- Assert event/pending/quote parity: new run recorder `payout_collateral_raw`, pending gross raw, and quote gross raw all equal the sum of actual post-clamp `collateral_seized`; delivered net equals gross less the actual fee. Cover multiple decimals, fractional prices, and underwater collateral.
- Add reserve exact-tuple regressions for each changed field after the stable/treasury await: no event or seizure on rejected paths, stable settlement remains exactly once, and only the spillover tail is refunded. The current reserve type-only recorder at `vault.rs:852-913` must be moved to the run-scoped ID recorder in the reviewed patch.
- Update static audits (`tests/audit_pocs_red_002_redemption_deficit.rs`, `tests/audit_pocs_userop_locks.rs`) to assert the new run-scoped recorder's consumed/shortfall behavior; do not preserve a broad event-writing wrapper just to satisfy old test names.

- **Legacy `None`, other asset ranks first:** construct state with XAUT as the first global run and an old event requesting ICP. Replay must mutate only the historical ICP eligible pool, in old CR order, and preserve old pending-margin calculation. Assert XAUT remains unchanged.
- **Legacy rounding:** replay a `None` event against 18-decimal collateral with fractional cached price, including a vector where `Decimal::from_f64` and `from_f64_retain` differ in raw output. Assert exact baseline native amount and pending state. Preserve old missing-field `None` decoding.
- **New run confinement:** new events persist exact selected IDs/outcomes; a later healthier same-asset run separated by another asset is untouched. A newly written empty outcome remains `Some(vec![])` and cannot trigger legacy broad replay.
- **Quote/execution parity:** for 6-, 8-, and 18-decimal collateral with prices 0.1 and 0.3, assert pure simulation equals selected-ID execution, simulation does not mutate state, and native raw conversion uses retained event/executor precision.
- **Clamp/overflow:** underwater simulation and execution both reduce the full actual debt while seizing only available collateral; zero collateral yields zero raw seizure. Two selected 18-decimal vaults each with 10,000,000,000,000,000,000 raw collateral produce an exact 20,000,000,000,000,000,000-u128 run/simulated aggregate. Checked payout conversion rejects an aggregate above `u64::MAX` before any ledger pull or burn.
- **Minimum persistence/enforcement:** old pending/event payloads decode with `None`; new event and pending payloads round-trip `Some(minimum)`; payout one raw unit below minimum causes no transfer and leaves the pending claim retryable/durable.

## Existing planner evidence in the working tree

`state.rs` currently has an exact `u128` `RedemptionRun.eligible_collateral_raw` aggregate and tests for 20e18 run metadata, pure simulator 20e18 aggregate/no mutation, underwater clamping, existing clamp parity, old missing-field `PendingMarginTransfer` decode, and filtered selected-ID execution. An additional independent test now covers simulator/executor parity at decimals 6/8/18 and prices 0.1/0.3. These tests have not been run because the workspace owner is holding Cargo until event dependencies are coherent. `git diff --check` passed before the added parity case; rustfmt check has unrelated existing differences and a restricted first-run wrapper difference. No transaction-path or event source was edited by this proposal.
