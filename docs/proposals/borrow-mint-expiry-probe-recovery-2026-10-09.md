# Borrow-mint expiry-probe recovery proposal (2026-10-09)

Status: **proposal only**. This document changes no canister behavior and
authorizes no deployment, live recovery, debt write, or collateral release.
It is separate from the approved owner-or-developer caller change in
`borrow-mint-recovery-authorization-2026-10-09.md`.

## Problem and risk

The backend reserves a vault while the outcome of its icUSD borrow mint is
uncertain. The current negative-proof path starts only after replaying the
original mint and receiving a typed `TooOld`. Replay is subject to fresh
borrow-admission checks. A changed price, collateral ratio, or protocol mode
can therefore prevent recovery even when the ledger transaction has expired.
The configured developer's new caller authority does not bypass those checks.

Clearing the reservation on elapsed time or a single `TooOld` result would be
unsafe: an earlier mint could have succeeded despite a lost reply. Recovery
must establish the outcome without creating another positive mint.

## Proposed source-only state machine

1. Apply only to a journal with an immutable mint tuple and durable
   pre-dispatch ICRC-3 log floor. Legacy journals lacking that floor remain
   positive-receipt-only. Keep the existing exact positive-receipt path.
2. Before any await, durably latch the journal against another positive mint
   replay. The latch and exact zero-value probe tuple must survive upgrades;
   repeated recovery attempts reuse that tuple. A concurrent owner or
   developer action must not clear or replace it.
3. Send a domain-separated, zero-amount and zero-fee mint probe to the same
   configured icUSD ledger, from the same backend canister, with the original
   mint timestamp. Only a decoded, typed `TooOld` is expiry evidence. Success,
   `Duplicate`, another ledger error, a reject, or an undecodable reply keeps
   the borrow held. The probe has no borrower debt effect.
4. Persist `TooOld` with a compare-and-set against the same journal and probe
   tuple. Only after that response, pin one ledger-advertised ICRC-3 log tip.
   Scan every block from the pre-dispatch floor through that fixed tip,
   advancing at most 64 blocks per call and preserving a durable cursor.
   Source-matched ledger-advertised archive pages may be used only under an
   explicitly documented ledger/archive integrity trust assumption.
5. An exact original mint receipt commits the journal's recorded owner's
   debt once. A plausible but incomplete mint, missing page, conflicting
   archive response, changed journal, or failed call leaves the reservation.
   Only complete contiguous absence through the post-probe tip plus a final
   journal-and-cursor compare-and-set removes the unminted reservation.
6. The scan has no finite total-span rejection; each call remains bounded.
   Recovery remains permissioned to the journal owner or configured developer.

This order matters. ICP orders delivered same-sender requests to a canister,
but their replies can arrive in a different order. The original mint request,
if delivered, must execute before the later same-sender probe. The scan tip
must therefore be requested **after** the probe's typed `TooOld` response.
The `TooOld` result alone does not prove the original mint was absent.

## Verification before merge

- Unit tests for the durable latch, journal and probe tuple compare-and-set,
  owner/developer concurrency, exact positive receipt, no-effect and ambiguous
  probe outcomes, cursor persistence, long histories, malformed/missing
  archive pages, final compare-and-set, and legacy positive-only behavior.
- Source-matched PocketIC test against the configured icUSD ledger Wasm for a
  lost reply after a committed mint, a never-delivered original mint, zero
  amount/fee acceptance, timestamp expiry, `TooOld` decoding, block ordering,
  upgrade interruption, and no double debt or accidental release.
- Independently verify the installed icUSD ledger's module/source provenance
  and the archive integrity assumption before any production use. ICRC-1's
  stale-timestamp behavior is phrased as `SHOULD`, so another ledger's behavior
  cannot be assumed.

This proposal changes only borrow-mint recovery behavior and its tests. It
does not authorize an ordinary borrow-policy bypass, timer, permissionless
keeper, live call, installation, or recovery of a historical vault.

References: [ICP message execution properties](https://docs.internetcomputer.org/references/message-execution-properties/),
[ICRC-1 transaction deduplication](https://github.com/dfinity/ICRC-1/blob/main/standards/ICRC-1/README.md#transaction-deduplication),
[ICRC-3 block log](https://github.com/dfinity/ICRC-1/blob/main/standards/ICRC-3/README.md).
