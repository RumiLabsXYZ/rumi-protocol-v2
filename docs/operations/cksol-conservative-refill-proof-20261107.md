# SEC-09 — conservative refill proof and remaining conditions

Source-only preparation. No new live reads, private financial values or customer
fields are included. No funding, reserve mutation, processing, refund or deployment
is authorized here. Exact liability observability is OPTIONAL, not a remedy gate.
Source reference: `6847982b44f247e0c99f0a453b583d3fab8107a2`.

## Legacy credit bound

Let S be the global paid sequence, K the owner cap, A the fixed minter attachment,
and F the fixed refund-fee reserve. Then retained legacy refundable credits obey:

`L <= min(S, K) * (A + F)`.

Source reasoning:

- State initialization creates zero counters and empty maps together (`state.rs`).
  Sequence/map fields were introduced together in `aa9e39a4`; both are persisted
  with additive defaults. No production writer resetting only the counter while
  retaining rows was found. Replay initialization starts both empty; the normal
  restored snapshot preserves both.
- `cksol_paid::prepare` inserts at most one map row per owner, enforces owner cap,
  and accepts a fresh sequence only through checked predecessor + 1. The same
  sequence returns without insertion or recollection. Insert and high-water mark
  update occur in one synchronous mutation. Eviction removes only settled rows
  with zero refundable credit; replacing a row does not increase row count.
- Rows begin at zero. Funding success sets A+F. `finish_deposit` only writes zero
  or returned attachment + F, rejects an excessive returned attachment, and
  runtime returned cycles cannot exceed the fixed attachment. Refund completion
  sets zero; pending/uncertain refund keeps the prior credit. No production
  mutation grows an existing credit beyond A+F.
- History search shows attachment/cap and refund-fee constants originated in
  `cc224b3e`/`aa9e39a4` without later value changes through the reference pin.
  Historical minter responses used the same fixed attachment. Test-only fixtures
  with arbitrary larger credits are not production migration/import paths.
- Interpret a saturating quote carefully: normally S=next_sequence-1, but at
  saturation use the conservative maximum, not an invalid subtraction inference.

This is a source-invariant upper bound, not enumeration of current rows and not
an independent attestation of arbitrary out-of-protocol state modification.

## ICP protection bound

Let I be the ICP global sequence, R its observed reserve allocation, and Ai its
fixed attached amount. Liquid journal protection is bounded by min(I,K)*Ai, so:

`P <= R + min(I,K)*Ai`.

ICP sequence and journal map were introduced together in `2066692d`. The only
production insertion advances the checked global sequence in the same state
mutation. Eviction removes settled journals; upgrade marking changes retained
phase/refund metadata and increments the guard epoch, without injecting journals
or resetting their sequence. Initialization/default state has no journals.
All reserve writers are allocation setter, pre-dispatch debit and bounded
reconciliation returns; observing R already includes those prior changes.

In particular, an initial global sequence proves no admitted ICP journal under
these invariants, giving P=R. A reserve allocation alone is not a cycle top-up.

## Fixed-ceiling remedy preparation

With current balance B, unchanged protected operating floor O and proven bounds
Lmax/Pmax, a sufficient snapshot target is:

`T = O + Lmax + Pmax + C`, where C is an explicitly accepted operational cushion.

A proposed fixed top-up D must satisfy `B + D >= T` under declared preflight
conditions. A higher refill trigger/target is an OPERATIONAL allowance; do not
raise the protected floor O or change a reserve setting to obtain it.
The total source debit ceiling is D plus the contemporaneously verified ledger
fee. Asset is cycles in the approved identity's default cycles-ledger account,
with target/backend binding from the case packet, not a new customer charge.

Required remaining packet inputs (not observed or selected in this source-only
step): source-account spendable balance, actual current fee, approved fixed D and
cushion C, and explicit maximum elapsed preflight window. Proposed operational
window: at most 60 seconds from final preflight to first send; expired inputs
require a fresh preflight, not a fresh payment attempt. This is a proposal, not
an assertion that a historical balance remains spendable.

Revalidate module/interface, counter ceilings, reserve ceiling and available
source funds immediately before execution. Stop rather than increase approved
D if conditions fail. A cushion is an allowance, not a proven adversarial maximum
burn rate; concurrent changes may still cause processing to refuse safely.
The on-chain backing predicate stays the final dispatch gate. Do not promise
customer completion solely from topping up the backend.

One later approved top-up attempt only: persist exact parameters and output;
no CLI retry-idempotency guarantee has been established. An uncertain result must
be reconciled through authoritative ledger/receiver evidence before another send.
After backing is available the original owner resumes the SAME funded request;
operator preparation does not impersonate that owner or authorize processing.

No new query, account lookup, ledger scan, tests, implementation or live action
was performed in this source-verification step. The backing-report handoff is now approved. Private funding-account bindings
remain in Ellis's authorized review channel and are not disclosed to other
reviewers. The remaining execution requirements are the fixed packet
preconditions and action approval, not a new aggregate getter.
