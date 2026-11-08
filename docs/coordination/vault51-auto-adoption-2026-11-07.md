# Vault 51 automatic ICP collateral adoption

Rob approved: “make it automatic when the ICP arrives. keep the debt and everything
in vault 51, just switch it to normal ICP collateral.” Adrian owns implementation;
Bram's existing SEC-08/PRs 612–613 and Dorian's deployed-source release evidence
are reused. Vault 158's completed recovery is unchanged.

## Observed baseline (November 7 Pacific)

- Backend Running module `b81583ae81421b88d3bd64bf27976b338b2b09748e9dd6b863849b40ac6fd736`,
  source `0eff84c93742ef6679a2ef09b1bd72e34b511665`; preserve SEC-11 origin/receipt
  maps and private journal records. Older main is not a valid rollback.
- Read-only observation 2026-11-08T03:40:34Z: claim 51/withdrawal 1696/burn 3435761
  ClaimConfirmed, WaterNeuron WaitingDissolvement, dedicated ICP balance zero,
  settlement/release absent. Recorded receivable 194,282,931,553 e8s
  (1,942.82931553 ICP); no actual settlement receipt exists yet.
- Vault 51 owner unchanged; recorded debt 287,535,228,114 e8s, accrued-interest
  component zero at observation. This is a dated balance, not a frozen debt.
- Current ICP fee 10,000 e8s. Current ICP collateral is Active, base APR 2%,
  liquidation ratio 133%, borrow threshold 150%, healthy CR 200%. Existing nICP
  base APR is 3%, liquidation ratio 125%; dynamic rates can differ. Future
  ordinary ICP operations use the then-current normal ICP rules.

## Chosen design

Use physical custody movement from the dedicated claim subaccount into the
backend's default ICP account; existing ordinary collateral payouts spend there.
Merely changing the collateral label while funds remain isolated would require
changing every ordinary spending path and is rejected as unnecessary scope.
Automatic checks run hourly and once shortly after upgrade, only for the exact
approved vault/owner/withdrawal/burn/subaccount. They never submit another nICP
redemption or cancel the existing withdrawal.

1. Verify the unique WaterNeuron request and its referenced native ICP payout
   block, recipient, amount and dedicated balance; use actual received amount,
   never the quote or an aggregate wallet balance. No settlement means no spend.
2. Keep the vault fenced; reject any existing owner-payout journal. Pin one
   adoption tuple before sending: ledger/source/default destination, nonce,
   memo/time, fee (maximum current 10,000 e8s), and received-minus-fee amount.
3. Reuse only that tuple on safe exact retries. Preserve ambiguous outcomes,
   never allocate replacement delivery parameters. A canonical-block
   reconciliation method can prove the exact transfer after a lost reply.
4. After the sweep receipt, accrue through the cutover under the old terms,
   then apply one durable replayable transition: same vault ID, owner, debt
   including all accrued interest, preserved history; collateral becomes ICP
   with exactly the net sweep amount. Move collateral/CR/debt attribution to ICP
   and recalculate protocol totals. Retain a terminal adoption tombstone;
   disable both claim valuation and owner payout without deleting history.

Add only an optional serde-defaulted adoption journal; retain existing claim
stage variants for old-client return compatibility. Terminal adoption is the
completed journal marker, not deletion or reinterpretation of old Settled rows.
Public vault projection returns ordinary ICP after cutover; dashboard claim
valuation must become zero so it cannot double count the same backing.

## Implementation steps and proof

- [ ] Add focused failing regressions for collateral adoption/debt preservation
  and terminal claim fencing; implement the replay transition.
- [ ] Implement exact-target automatic settlement/sweep, immutable fees and
  receipts, once-only completion and reconciliation; retain payout exclusion.
- [ ] Wire bounded hourly timer and update owner/transparency/dashboard copy.
- [ ] Run focused claim/state/replay tests, legacy CBOR decode, negative receipt
  and conflicting-payout tests, focused frontend tests. Reuse current test stack.
- [ ] Regenerate canonical Candid/bindings; verify deployed API compatibility,
  one source-matched release build/artifact and scoped independent review.
- [ ] Push concrete release materials and update existing SEC-08 source board.

No broad audit, new team or framework is planned. Deployment, timer activation
on production and actual future adoption are not claimed by this document.
A release must preserve the deployed SEC-11 state/journals and current custody
obligations; implementation approval is not evidence of installation.
