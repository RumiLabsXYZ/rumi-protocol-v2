# Stability Pool reopening — October 23, 2026

## Authorized scope

The operator requested completion of the reopening review, repairs to actual
blockers, independent adversarial review, and global pool resumption after
verification without another approval request. The coordinator alone performs
production operations.

The installed predecessor is the October 21 interest-recovery upload
`b26fb0358ad3c5d34256e13f069ae3ddfe50072afffc477f7fbe2d402cb384a8`.
Its 237-packet recovery is complete. No recovery batch, finish call, mint,
notification replay, legacy-record cleanup, or snapshot restoration is part of
this reopening.

## Confirmed blockers and repairs

1. **Withdrawal hardening:** a local regression exposed a backing shortfall in the predecessor's withdrawal path after an ambiguous ledger response. Withdrawals are now journaled with the exact debit before dispatch; unknown outcomes retain the debit and original transfer identity.
2. **Withdrawal fees:** the fee is now pinned per transfer.
3. **Liquidation admission:** readiness is now checked before any depositor balance is reserved.

Authenticated ICRC-1 `Ok` and `Duplicate` responses settle only their exact
persisted owner/ledger/tuple/generation. They acknowledge the dispatched
transfer under the configured ledger's existing trust model. Explicit
reconciliation of uncertain or expired outcomes still requires an exact
receipt; an unverified candidate block never authorizes settlement or prevents
a later correct proof. Existing collateral-claim proof checks are unchanged.

The withdrawal journal is additive and optional for predecessor decoding.
Upgrade handling retains interrupted tuples and debits. Empty depositor
metadata is retained while another withdrawal, claim, or deposit intent is
pending. The public withdrawal signature is unchanged; pending status and
explicit reconciliation are additive generated interfaces.

## Verification and activation sequence

1. Verify the source-matched release and its generated interface, run focused
   unit checks and local exact-Wasm real/fault-ledger tests, and preserve the old
   regression separately from readiness results.
2. On copies of a fresh actual snapshot, execute the predecessor pre-upgrade
   and candidate post-upgrade hooks. Verify every original state value, all
   participant positions, completed interest recovery, permanent receipt maps,
   and all 237 sealed receipt payloads. The new empty withdrawal journal is the
   only expected state change.
3. Verify fresh production module, controller, pause, recovery, journal,
   accounting, and service-control readbacks. Preserve all existing backups.
4. Obtain two independent adversarial passes on the final source, artifact,
   deterministic evidence, and exact live action.
5. Upgrade only the existing Stability Pool in upgrade mode. Record a durable
   dispatch marker first. Any unknown result is reconciled against the original
   request; never blindly retry or switch executor.
6. Verify the installed bytes/interface, participant state and receipt totals
   while the global application pause is still true. Call `resume_operations`
   once, then verify its result, `emergency_paused=false`, the resumption event,
   management Running, and subsequent timer/service continuity.

The pool target is `tmhzi-dqaaa-aaaap-qrd6q-cai` on `ic`, using the existing authorized identity.
Controller membership and protocol binding must
remain unchanged. Keep the pool paused and reconcile on a failed verification;
do not roll back across new pending withdrawal journals.

## Preserved controls and limits

Global resumption enables the supported pool operations and their existing
timers. The served frontend uses `deposit_with_intent`. Legacy direct deposit,
convenience conversion, and ordinary non-LP liquidation routes have independent
existing containments; this operation does not activate them. Backend 3USD
admission, reserve, bot, registry, and chain-auto configuration are not changed.

The 53 legacy interest records are already credited and remain retained.
The unexplained 0.25664678 icUSD excess remains excluded. Neither is treated as
an invented reopening gate. Existing published liquidation metadata drift is disclosed; a separate interface cleanup is not used to expand this repair.

Private snapshots, positions, raw replies, executable migration checks, and
release/review receipts are retained in the coordinator's recovery evidence.
Local runtime tests do not claim production user transactions or external
native-chain settlement.
