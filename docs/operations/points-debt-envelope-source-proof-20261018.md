# Points outage: nonnegative interest premise and debt-history limits

**Accounting evidence only. No final allocation, applied credit, deployment or
restored processing is claimed.** Missing random captures have no unique exact
counterfactual. This narrows one condition of the conditional debt worksheet;
it does not approve the pending conservative compensation policy.

## Actual inputs and complete event range

The actual backend heap configuration was queried on October 18 at
08:58:52 UTC. All nine current collateral configurations have nonnegative base
APR and no negative recovery override. Every selected per-asset/global multiplier
and recovery multiplier is positive. Decimal blobs were decoded as exact signed
96-bit coefficients plus scale after checking reserved bits and the scale bound.
Global/recovery float projections are used only for their sign, not for exact
interest or points arithmetic.

The retained unfiltered event prefix was extended from 523370 to 523430. The
combined range [512671, 523430) contains 10,759 contiguous events, with the count
frozen **after** the heap configuration query. There is no SetInterestRate,
SetRecoveryParameters, SetRateCurveMarkers, SetRecoveryRateCurve,
AddCollateralType, UpdateCollateralConfig or Init event in that range. Untimed
administrative variants were included; this is not a timestamp-only filter.

The only Upgrade is event 516236, recorded at **2026-10-12 00:24:26.944205084 UTC**, after epoch 19 began at 00:00 UTC. The retained backend snapshot contains 516237 events. This is the Upgrade event's recording time, not an independent completion receipt. The original draft's assertion that PR527 preceded epoch 19 was wrong; that draft and both independent FAIL reviews are preserved.

A separate management read confirms the backend is Running on the exact PR527
gzip module e325ce0d9ee4b5dcd1e68caafc6c69e58ec0d8cae46efd2930dfaed4f97ad352.
Running and GeneralAvailability are not price-freshness or admission evidence.

## Source argument and supported interval

Review of the immutable backend commit `1c45ded4c8ad85e396626421674a9da4f8148be1` shows that interest-rate and multiplier configuration is changed only by recorded administrative events, that accrual with nonnegative rates cannot reduce debt, and that no relevant configuration event appears in the retained range. This is a source and history inference, not an observation of every intermediate state.






Combining the actual nonnegative heap inputs, absence of relevant configuration
changes through the later frozen count, and reviewed runtime write boundaries
supports the nonnegative-interest premise **for the verified PR527 runtime
interval after the October 12 Upgrade through the captured configuration/event
endpoint**. This is a source/history inference under retained-log and ordinary
lifecycle assumptions. It is not an observation of every intermediate heap or
proof against privileged state replacement. The opening 24 minutes 26.944205084
seconds of epoch 19 are not established by the PR527 source argument. No earlier
runtime or stopped-state proof is asserted here; that interval remains an
explicit condition before an epoch-wide debt ceiling can be certified.

## Debt-decrease visibility limit

Not every historical debt reduction is visible in the public event log, so an absence of public liquidation events does not by itself exclude such a loss. This is an accounting visibility limitation, not evidence that one occurred. A strict ceiling must account for it or establish its absence with adequate evidence.


Native XRP is present in the affected data: synthetic collateral principal
5zjma-7dsov-wwsll-yojyc-23tbo-ruxmz-i has bytes
72756d692d7872702d6e6174697665. The saved Memory 4 State contains native XRP vault
IDs 194, 199, 206, 208 and 209; the later 15-participant query also contains five
native XRP vaults. The saved State is a pre-upgrade serialized blob, not a fresh
heap observation at snapshot time. These observations do not exclude historical
absorbs. Current SP pause or empty claim lists do not establish that historical
exclusion either. A strict ceiling requires accounting for this possible loss or
establishing its absence for the affected interval using adequate evidence.

## Remaining allocation limits

The existing dust worksheet ends October 18 at 07:11:06 UTC, before its vault
queries began. Its 72 distinct current/closed vaults plus one positive-borrow
cycle allow at most 3,650,000 raw icUSD (0.03650000) of unlogged dust loss under
its stated cycle/removal assumptions. Its numerical output is unchanged and
conditional. This note does not extend it to the later heap read or certify
all other debt reductions, ownership changes or removals.

Historical SP delivery/source provenance, missing valuation/verification inputs,
repayment asset eligibility, final participant census and any later missed
interval remain explicit. The full epoch 19 period has not ended. No final
manifest is sealed, the policy decision is pending, and the application pin is
unset. The original execution-request disposition is still required before a
real upgrade/replay/resume. Compensation work does not delay otherwise-reviewed
normal resume after that disposition. These accounting limitations add no gate
to ordinary borrowing, the bot or the separate Treasury cutover.

All new external actions for this note were query/status reads. Snapshot analysis
used local read-only input. No setter, install, lifecycle change, replay, credit
or financial call was made. No tests or production builds were run.
