# SEC11 deposit delivery omissions

## SEC11-DEF-01 — Explorer and points projection

Owner: Miles, with the existing Explorer and incentives owners. Status: Open.

The native-XRP repeat-deposit credit and permanent hash receipt commit in a private ordered journal. The current release updates vault collateral but does not add these topups to public Explorer/AddMargin history or points ingestion. This omission does not change custody credit or replay protection; it is deferred to keep this customer repair bounded. Do not promise public topup history or points for this flow. Before relying on that history or issuing incentive claims for these deposits, implement and verify a source-bound idempotent projection with the existing owners and record its evidence/disposition here. No new audit or harness is required by this entry.

Evidence: `docs/coordination/SEC-11-xrp-repeat-deposit-implementation-20261107.md`, source `ef8a694a4490bf06e3d36086399b9261a61e84c1`, integrated onto the deployed SEC10 port as `e0de879b`. No customer deposit/withdrawal or financial canary is included in this release verification.
