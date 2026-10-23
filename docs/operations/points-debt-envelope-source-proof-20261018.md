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

The only Upgrade is event 516236. Its timestamp side log records
**2026-10-12 00:24:26.944205084 UTC**, after epoch 19 began at 00:00 UTC.
The retained backend snapshot contains 516237 events and 445853 timestamp entries;
the offset is 70384, so event 516236 maps to timestamp entry 445852, value
1791764666944205084 ns. This follows storage.rs:295–315. The saved stable-memory
payload SHA256 is
1fd46a6ef4521ffae2d7bd005fbf2f87b702800edaac0607d1359b65bd4974c5.
This is the Upgrade event's recording time, not an independent CLI completion
receipt. The original draft's assertion that PR527 preceded epoch 19 was wrong;
that draft and both independent FAIL reviews are preserved.

A separate management read confirms the backend is Running on the exact PR527
gzip module e325ce0d9ee4b5dcd1e68caafc6c69e58ec0d8cae46efd2930dfaed4f97ad352.
Running and GeneralAvailability are not price-freshness or admission evidence.

## Source argument and supported interval

All source references below refer to immutable backend commit
`1c45ded4c8ad85e396626421674a9da4f8148be1`, in
`src/rumi_protocol_backend/src/`.

| Runtime input | Write boundary |
| --- | --- |
| Whole collateral config / base APR | event.rs:4469–4506 records AddCollateralType or UpdateCollateralConfig before replacing a config; 4741–4752 records SetInterestRate before assigning base APR |
| Static recovery APR override | event.rs:4556–4572 records SetRecoveryParameters before assignment |
| Per-asset/global layer 1 multipliers | event.rs:4590–4642 records SetRateCurveMarkers before assignment |
| Layer 2 recovery multipliers | event.rs:4645–4679 records SetRecoveryRateCurve before assignment |
| PR527 restoration | main.rs:790–831 records Upgrade and restores saved State; State::upgrade at state.rs:5329–5333 changes only optional mode. The broader post-upgrade hook has other migrations, including test-collateral removal at main.rs:939–966; this table does not describe the entire hook as mode-only. |

Mutable config references used by fee-retry paths change ledger fees, not APR or
multipliers (lib.rs:3353; vault.rs:2920, 5912, 9076). ICP configuration sync at
state.rs:6926–6937 also leaves APR and multiplier fields intact. Price refreshes,
status, debt-ceiling, redemption-tier and haircut setters alter other fields.
Tests and genesis construction are distinct from runtime mutation paths.

State::get_dynamic_interest_rate_for (state.rs:6006–6039) selects a nonnegative
static override or a nonnegative base multiplied by selected positive layer 1
and layer 2 multipliers. Unknown collateral defaults to zero APR (state.rs:830).
Interpolation returns a positive endpoint, a convex interpolation of bracketing
positive markers, or its positive 1 fallback (state.rs:5810–5843). Changes in
collateral ratio or protocol mode select different positive multipliers without
negating the configured rates.

In accrue_single_vault and accrue_all_vault_interest (state.rs:6046–6152), elapsed
time is nonnegative and applied debt is ceil(debt*(1+rate*elapsed/year)). With
nonnegative rates this cannot reduce debt. Overflow traps/rolls back or defers,
rather than committing a wrapped value. **The saturating interest_delta alone is
not this guarantee:** the function assigns new_debt directly, so a negative
configured rate could reduce debt while recording zero interest_delta.

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

## Concrete debt-decrease visibility limit

The native XRP SP absorb function, stability_pool_liquidate_xrp_vault_in_state
(vault.rs:4168–4349), reduces debt at line 4317. It records claim/proof/result
state, including record_sp_xrp_absorb_result_bounded (vault.rs:4058–4078), without
appending a public PartialLiquidateVault event in that function. Therefore an
absence of public liquidation events does not by itself exclude this debt loss.
This is an accounting visibility limitation, not evidence that an absorb occurred
or a demonstrated financial vulnerability.

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
