# Cycle Sentinel shared reserve conversion policy

Date: 2026-09-28
Status: Proposed; awaiting explicit authorization for reserve conversion.

## Result

Cycle Sentinel uses available Cycles Ledger funds first. When a fresh balance
cannot cover an eligible canister refill while preserving the protected reserve,
it may convert deposited ICP into its own default Cycles Ledger account, then
withdraw the exact configured refill amount to the target.

## Existing governed limits

- Protected Cycles Ledger reserve: 10T cycles.
- Global canister funding limit: 40T cycles per rolling 24 hours.
- Current target rules: below 3T, add exactly 2T, maximum 6T per rolling 24 hours,
  with a 3,600-second cooldown.
- Minimum ICP reserve: 0 ICP.
- Sentinel runtime recovery: below 1T runtime cycles, add 1T, maximum 10T per day.

## Explicit reserve conversion authorization requested

Authorize automatic conversion of ICP deposited into Sentinel's own account to
restore the existing 10T protected reserve and cover an eligible configured refill.
A separate durable conversion budget must limit new conversion commitments to
the already-governed global daily cap, currently 40T cycles per rolling 24 hours.
The target and global canister funding caps remain independently enforced.

Each conversion is limited to the fresh reserve deficit plus the eligible refill
and required ledger fees. With a zero cycles balance, the first 2T target refill
requires approximately 12.0002T of CMC-minted cycles: 10T retained reserve, 2T
exact target refill, 0.0001T Cycles Ledger withdrawal fee and 0.0001T Cycles Ledger
deposit fee. ICP division rounds up; any remainder stays in Sentinel's reserve.
The ICP Ledger transfer fee is also charged. No other wallet is debited.

An immutable transfer and its linked ICP hold and conversion-budget reservation
must be persisted before payment. Ambiguous results retain their reservations;
retries use the same transfer and CMC block. An upgrade cannot allocate a second
payment for the same conversion. Unknown or stale source observations cannot
trigger conversion. Stopped, uninstalled, disabled or paused targets do not fund.

This authorization changes no signer, controller, threshold, protected-reserve,
target refill or governance setting. Live upgrades preserve existing state.
