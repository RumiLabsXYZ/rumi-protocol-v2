# Cycle Sentinel and public telemetry design

Date: 2026-09-13
Branch: `codex/cycle-sentinel-telemetry`
Status: approved by Rob on 2026-09-13; implementation authorized

## Decision summary

Build a new `rumi_cycle_sentinel` canister for cycle observation and automatic
maintenance. Publish sampled operational data at exactly
`https://rumiprotocol.com/telemetry`. Provide authenticated management at
`https://app.rumiprotocol.com/telemetry`.

The existing `rumi_treasury` canister, its Candid interface, its controller
configuration, and the existing `/treasury` route are not changed. The existing
canister may receive normal cycle top-ups as one registered target, just as it
does today.

The Sentinel is never a controller of a monitored canister. Anyone can top up a
canister, so funding authority does not require controller authority. Generic
canisters that lack Rumi's self-report method use one pinned, immutable status
proxy as an additional controller. Adding that proxy is a separate live action,
not part of source implementation.

## Why the first inventory had eight targets

`rumi_analytics.cycle_manager_targets()` currently lists eight Rumi Rust
canisters that implement the public `cycles_status` interface. It intentionally
does not include generic ledgers, indexes, asset canisters, or other canisters
that cannot self-report.

CycleOps can display all 15 because the legacy deployment script added the
CycleOps principal to target controller lists, allowing it to call management
`canister_status`. Eight was therefore an interface limit in the existing Rumi
discovery list, not a Sentinel product limit.

The Sentinel has its own stable, governed registry. It can hold the current 15,
the new Conflux frontend, and later canisters without a code upgrade.

## Initial inventory

Every seed record begins with `enabled = false` and `auto_topup = false`.
Observation and funding are enabled only after the configured mode passes its
preflight and the target policy has been approved.

| Name | Principal | Initial observation mode | Notes |
|---|---|---|---|
| `rumi_points` | `bfnu3-6aaaa-aaaab-qhanq-cai` | `SelfReport` | Existing Rumi interface |
| `arb_bot` | `ucjxv-nqaaa-aaaaj-qrsaq-cai` | `PinnedBlackhole` candidate | Verify authoritative adjacent-repo ID |
| `icusd_ledger` | `t6bor-paaaa-aaaap-qrd5q-cai` | `PinnedBlackhole` candidate | Generic ledger |
| `rumi_treasury` | `tlg74-oiaaa-aaaap-qrd6a-cai` | `SelfReport` | Observe and fund only; no source or controller changes |
| `rumi_stability_pool` | `tmhzi-dqaaa-aaaap-qrd6q-cai` | `SelfReport` | Existing Rumi interface |
| `rumi_protocol_backend` | `tfesu-vyaaa-aaaap-qrd7a-cai` | `SelfReport` | Existing Rumi interface |
| `vault_frontend` | `tcfua-yaaaa-aaaap-qrd7q-cai` | `PinnedBlackhole` candidate | Asset canister |
| `rumi_homepage` | `t2xrh-2aaaa-aaaap-qreaa-cai` | `PinnedBlackhole` candidate | CycleOps labels this `rumi_protocol_frontend` |
| `icusd_index` | `6niqu-siaaa-aaaap-qrjeq-cai` | `PinnedBlackhole` candidate | Generic index |
| `test_icp_ledger_canister` | `pspic-iaaaa-aaaap-qrkna-cai` | `PinnedBlackhole` candidate | Remains disabled until explicitly approved |
| `rumi_3pool` | `fohh4-yyaaa-aaaap-qtkpa-cai` | `SelfReport` | Existing Rumi interface |
| `threeusd_index` | `jagpu-pyaaa-aaaap-qtm6q-cai` | `PinnedBlackhole` candidate | Generic index |
| `liquidation_bot` | `nygob-3qaaa-aaaap-qttcq-cai` | `SelfReport` | Existing Rumi interface |
| `rumi_amm` | `ijlzs-2yaaa-aaaap-quaaq-cai` | `SelfReport` | Existing Rumi interface |
| `rumi_analytics` | `dtlu2-uqaaa-aaaap-qugcq-cai` | `SelfReport` | Existing Rumi interface |
| `conflux_public_frontend` | `a52ri-naaaa-aaaas-qgy4a-cai` | `Unobserved` | Sixteenth candidate; currently documented as uninstalled |

Before any live activation, a reviewed manifest must contain the exact CycleOps
threshold and refill values in unambiguous fields. The compact `X TC @ Y TC`
badges in the supplied screenshot are not used to guess which number is the
threshold. This does not block source implementation because all seed entries
are disabled by default.

## Components and hosting

### `rumi_cycle_sentinel`

A new Rust canister owns the governed registry, hourly sampler, bounded history,
funding policy, stable funding outbox, alarms, and public cached query surface.

### Root-domain telemetry

`src/rumi_homepage/src/routes/telemetry/+page.svelte` serves the public dashboard
at `rumiprotocol.com/telemetry`. It reads only anonymous public Sentinel queries.
It contains no mutation controls and links authorized operators to the app.

### Authenticated management

`src/vault_frontend/src/routes/telemetry/+page.svelte` serves the operator view at
`app.rumiprotocol.com/telemetry`. Public data may remain visible without login.
After wallet login, a signer can create and approve proposals, add or edit
targets, pause automation, request a policy-bounded manual top-up, and perform
safe reconciliation actions.

Every update method enforces authorization in the canister. Hiding a button or
requiring a frontend login is not treated as a security boundary.

## Authority and registry policy

The stable registry is the only source of spending authority. Analytics
discovery and target-reported fields are metadata only and can never introduce a
recipient or change a funding amount.

Each target record contains:

- exact target principal and immutable registry revision;
- bounded display name, environment, criticality, and tags;
- observation mode;
- cycle threshold and refill amount;
- per-target rolling 24-hour cap and cooldown;
- anomaly limit;
- `enabled`, `auto_topup`, and pause state.

Registration rejects anonymous, Sentinel-self, management, ICP Ledger, CMC,
Cycles Ledger, duplicate, and other explicitly reserved principals. It also
rejects oversized names, tags, registry counts, thresholds, refill amounts, and
caps. Environment and criticality are never authorization signals.

Registration, removal, enabling funding, spend-widening edits, global policy
changes, signer changes, and unpausing use the delegated-vault multisig plus
timelock model. Signers must be non-anonymous and unique, and the threshold must
be valid. One signer may pause immediately. Manual top-up requires a signer and
still obeys the target policy, caps, cooldown, reserve, and one-operation rule.
There is no trusted factory or automatic analytics-import hook in v1.

Editing or removing a target stops new attempts. It cannot alter an in-flight
operation, whose exact recipient and policy snapshot remain immutable until the
operation reaches a terminal state.

## Observation modes

### `SelfReport`

The Sentinel calls the existing Rumi `cycles_status` method. The report is
advisory because target code can change or be compromised. Sentinel derives the
funded state using the registry threshold and displays target-reported
operational health separately. Per-target and global caps bound the effect of a
false low report.

### `PinnedBlackhole`

There is no configurable proxy field. Source pins one reviewed immutable proxy
principal and its expected module hash.

The pin is established by an off-chain reproducible source-to-Wasm review.
Before a target becomes eligible, Sentinel asks the pinned proxy for the proxy's
own status and requires:

- the observed module hash equals the compiled pin;
- the proxy controller list is exactly the proxy itself;
- the target controller list contains the proxy as an additional controller;
- the target is `Running`;
- the target has a nonempty module hash.

The target is fundable only when every invariant holds and `auto_topup` is
enabled. `Stopped`, `Stopping`, uninstalled, unreachable, or proxy-mismatch
targets are not funded. A failed read is `Unreachable`, never a zero balance and
never a top-up trigger.

Adding the proxy to a target is a separately approved live operation. The
procedure must read the full current controller set, verify capacity, append the
proxy without removing or replacing any current controller, write the exact new
set, and verify the result. Sentinel itself is never added.

### `Unobserved`

The target appears in inventory but cannot be automatically funded. This is the
default for a target that has neither self-reporting nor a verified proxy.

## Funding accounts and self-liveness

Three balances are distinct in state, policy, and UI:

1. Sentinel runtime cycles keep the Sentinel executing. They are never sent to
   ordinary targets in v1.
2. Sentinel's Cycles Ledger account is the primary T-cycle funding reserve.
3. Sentinel's ICP Ledger account is the CMC fallback reserve.

Users fund the second account by transferring T-cycles to the Sentinel principal
on the Cycles Ledger. They fund the third by transferring ICP to the Sentinel's
ICP account.

Sentinel self-maintenance is a hard-coded lane outside the dynamic registry. It
checks its own runtime balance before target maintenance and can withdraw from a
protected portion of its Cycles Ledger account only to `ic_cdk::id()`. The
destination is not configurable. This lane has a governed threshold, refill
amount, rolling cap, one in-flight operation, durable retry state, and alarms.

Ordinary targets cannot consume the protected self-recovery reserve. The runtime
threshold has a compiled minimum above the freeze and execution safety margin,
even if governance proposes a lower value. An unknown self-recovery operation
suppresses all target distributions until it is reconciled. If the protected
reserve is insufficient, Sentinel alarms and does not distribute funds that
would worsen its own recoverability.

## Funding selection

For an eligible target whose sampled balance is less than or equal to its
registry threshold:

1. Reserve policy capacity before any await.
2. Prefer a direct Cycles Ledger `withdraw` from Sentinel's ledger account to the
   target canister.
3. Use ICP-to-CMC fallback only after the cycles attempt is proven unavailable,
   not unknown.
4. Settle or retain reservations according to the durable result.

This deliberately replaces the delegated-vault sequence that first withdraws
cycles into its runtime balance and then performs a raw `deposit_cycles` call.
Direct Cycles Ledger withdrawal has ledger-side duplicate detection and avoids
an ambiguous, non-idempotent raw deposit reply.

### Task 4 amendment: `Duplicate` is not delivery proof

The pinned Cycles Ledger source at commit
`29d98de5131918649a4c1cdd47fc176dea8770ef` records and deduplicates a
withdrawal before attempting the management-canister deposit. Therefore an
`Err(Duplicate { duplicate_of })` reply proves only that an earlier request was
recorded. It does **not** prove that the destination received the cycles, and
it must never be treated as `Confirmed`, must never attach its duplicate block
as delivery proof, and must never settle a reservation. The first direct
`Duplicate` and a `Duplicate` after `Unknown` both enter `Quarantined`, retain
their reservations, suppress self-recovery when applicable, and do not fall
through to ICP.

Quarantined Cycles operations have one bounded reconciliation path. A future
signer-gated Task 6 adapter may call the core state machine only after
independently verifying an authoritative delivery block, a proven no-spend
outcome, or a known source debit. The decision must be explicit; a
`Duplicate` value alone is not an accepted decision. Delivery requires a
matching verified block and settles the immutable amount plus fee. No-spend or
known-debit evidence releases/settles ordinary reservations conservatively.
Self-recovery accepts only verified delivery and remains suppressed until that
proof resolves the operation.

The deterministic Task 4 compatibility evidence is vendored at
`src/rumi_cycle_sentinel/tests/vendor/cycles_ledger_v1_0_6.did` and
`cycles_ledger_v1_0_6_withdraw_behavior.txt`; the Task 6 gate remains required
for a real ledger canister query, rejection/lost-reply trace, upgrade, and
end-to-end settlement proof.

The ICP fallback persists an exact ICP transfer into the CMC top-up subaccount,
then calls CMC `notify_top_up` with the confirmed block index. The exchange-rate
snapshot must be nonzero, fresh, and within per-attempt and rolling caps.

## Stable exact-once outbox

Only one unresolved funding operation may exist per target. Timer and manual
paths share the same stable reservations and executor state.

Every operation snapshots:

- a monotonically increasing operation ID;
- immutable target principal and registry revision;
- funding rail, exact amount, source account/subaccount, and destination;
- one globally monotonic `created_at_time`, computed as
  `max(now, last_created_at_time + 1)` and persisted before the call;
- fees, memo where supported, rate snapshot, reservations, attempts, and
  timestamps.

Cycles Ledger lifecycle:

`PlannedReserved -> Submitted -> Confirmed(block) | Unknown -> Complete | Terminal | Quarantined`

An unknown result retries only the exact persisted arguments while the ledger
deduplication window remains valid. `Duplicate` is record evidence only, not
delivery proof: it identifies a prior recorded request but is quarantined
until an independent verified outcome exists. An unknown cycles operation never
falls through to ICP. `TooOld` without proof remains quarantined and reserved.

ICP and CMC lifecycle:

`PlannedReserved -> LedgerSubmitted -> TransferUnknown | TransferConfirmed(block) -> NotifyPending -> Complete | Refunded | Terminal | Quarantined`

ICP retries use the exact original ledger arguments and timestamp. `Duplicate`
is treated as the original block, and every CMC retry uses that block. No second
ICP transfer is started while the first is unknown.

Reservations cover spendable Cycles Ledger balance, ICP balance, global and
per-target rolling caps, cooldown, and one in-flight operation. Completion
settles a reservation. A proven terminal no-spend releases it. An unknown result
keeps it reserved across awaits and upgrades.

Signer reconciliation is fail-safe:

- `ResolveAsSpent` conservatively settles an unknown operation as spent;
- `AttachBlockProof(block)` succeeds only after Sentinel reads the ledger block
  and verifies it matches the immutable operation;
- a quarantined Cycles `Duplicate` requires the same independent block or
  explicit no-spend/known-debit evidence; the duplicate response itself is
  never sufficient;
- capacity is released only after authoritative proof of no-spend or refund.

## Sampling, health, and burn

The hourly timer samples targets, processes self-recovery first, resumes pending
operations, and then evaluates new target maintenance. Public queries return
cached data only. Each row includes observation mode, `as_of`, last successful
sample, stale age, and next scheduled sample.

Public states are distinct:

- `Healthy`: running/operational and above the registry threshold;
- `Low`: observed balance is at or below the registry threshold;
- `Stopped`: installed but not running;
- `Uninstalled`: no module hash;
- `Unreachable`: the latest observation failed;
- `Unobserved`: no active observation mode.

Self-report target operational health is shown separately from funded state. A
stale sample never triggers a new top-up. A failed read never becomes zero or
healthy.

Burn is derived from timestamped balance deltas corrected by confirmed top-ups:

`burn = max(0, starting balance + confirmed top-ups - ending balance)`

An interval containing an unknown top-up is indeterminate. Target-reported burn
may be displayed as advisory detail but never drives spending. Configured burn
anomalies pause new target top-ups and raise an alarm; resuming is governed.

## Public data and login decision

The root telemetry page is public. A login would not hide the underlying public
Candid queries and would add friction without creating a meaningful security
boundary.

Publishing balances, burn, thresholds, and stale age reveals operating runway
and may help an observer infer deployments or usage spikes. It does not reveal
user positions, keys, controller credentials, signer identities, pending
governance payloads, or grant mutation authority. That disclosure is accepted
for the transparency benefit.

The public summary shows healthy, low, stopped, uninstalled, unreachable, and
unobserved counts; total observed cycles; reserve availability; alarms; last
sample; and next sample. The table shows name, principal, environment,
criticality, source mode, balance, threshold, refill amount, burn/runway, state,
and recent top-ups. Big integers are formatted without JavaScript `number`
conversion.

## Bounded storage and queries

Initial safety bounds are constants and are tested:

- at most 128 registry targets;
- 2,160 hourly samples per target, about 90 days;
- at most 1,024 alarms;
- at most 256 open or retained governance proposals;
- at most 512 retained funding operation summaries after terminal compaction;
- public pagination of at most 100 records per call.

Stable storage preserves registry, proposals, samples, alarms, reservations,
the monotonic timestamp, and every nonterminal outbox operation. Upgrade tests
resume each state without creating a second spend. Candid compatibility checks
cover the handwritten interface and generated declarations.

## Public and authenticated API shape

Exact type names may change during implementation, but the behavioral boundary
is fixed.

Anonymous queries:

- `get_public_overview()`
- `list_public_targets(cursor, limit)`
- `get_public_target(principal)`
- `list_public_samples(principal, cursor, limit)`
- `list_public_topups(principal, cursor, limit)`
- `list_public_alarms(cursor, limit)`

Authenticated queries and updates:

- `get_my_permissions()`
- `list_governance_proposals(cursor, limit)`
- `propose_register_target(args)`
- `propose_update_target(args)`
- `propose_remove_target(principal)`
- `propose_set_global_policy(args)`
- `approve_proposal(id)` and `execute_proposal(id)`
- `pause_target(principal)`
- `manual_top_up(principal)`
- `resolve_unknown_as_spent(operation_id)`
- `attach_block_proof(operation_id, block_index)`

All authenticated methods reject unauthorized and anonymous callers in canister
code.

## Verification requirements

Implementation is not ready until the following pass:

- governance and authorization negatives for every update;
- registration, field-size, principal, count, and policy bounds;
- exact `balance <= threshold` behavior;
- false self-report bounded by per-target and global caps;
- pinned proxy principal, hash, self-controller, target-controller, target state,
  and installed-module checks;
- timer/manual concurrency and stable reservation tests;
- lost reply, exact retry, conservative `Duplicate` quarantine, `TooOld`,
  refund, and explicit reconciliation tests for both funding rails;
- upgrade and restart at every outbox state;
- target edit or removal during an in-flight operation;
- Sentinel self-recovery destination immutability, protected reserves, low-fund
  behavior, and unknown-operation suppression;
- official Cycles Ledger Candid and fee/duplicate-before-delivery contract
  integration test in addition to mocks and the source-pinned Task 4 fixture;
- CMC account and memo vectors;
- all 15 current entries plus Conflux inventory coverage;
- public anonymous reads and private mutation rejection;
- truthful stale, unreachable, stopped, and uninstalled rendering;
- bounded stable history and pagination;
- Candid/interface compatibility, focused Rust tests, frontend tests, builds,
  and adversarial verification on the final diff.

## Delivery and activation boundaries

Source delivery may add the canister, tests, Candid, declarations, configuration,
root dashboard, and app management page. It does not authorize:

- creating or deploying the Sentinel canister;
- sending it ICP or T-cycles;
- adding or removing any controller;
- enabling automatic top-ups;
- changing the existing `rumi_treasury` implementation or `/treasury` route;
- removing CycleOps;
- any other mainnet mutation.

A later activation runbook must verify authoritative target IDs and exact policy
values, deploy and fund Sentinel, observe in shadow mode, onboard the pinned
proxy one target at a time where required, enable targets incrementally, prove
top-up and recovery behavior, and only then propose removing CycleOps. Each live
step requires separate approval and evidence.

## Adversarial review record

Two independent reviewers initially rejected the draft. Their blockers were
incorporated: durable exact-argument operations, stable reservations, fixed proxy
authority, disabled-by-default registration, in-flight policy snapshots,
separate funding balances, Sentinel self-recovery, exact root-domain hosting,
installed/running eligibility, fail-safe reconciliation, bounded telemetry, and
an explicit Cycles Ledger contract test.

Both reviewers passed the corrected architecture with no remaining design
blocker. Residual risks are public runway disclosure, proxy availability,
bounded trust in self-reporting canisters, conservative reservations that may
remain locked after an unknowable result, and the need for careful per-target
controller onboarding.
