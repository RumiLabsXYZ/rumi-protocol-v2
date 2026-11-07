# SEC-09 — backing observability and conservative-remedy preparation

Preparation only. No execution approval, exact top-up quote, client receipt data,
or new financial observations are contained in this note. Ellis owns the case.
Existing released source reference: `6847982b44f247e0c99f0a453b583d3fab8107a2`.
Fresh private read receipts remain in the originating diagnostic chat pending
scoped disclosure approval; they must not be copied into this packet indirectly.

## Scope correction — exact remedy does not require a minimal amount

The earlier version treated an exact liability census as the remedy gate. That
was narrower than the user request. A fixed conservative top-up can be an exact
executable remedy if a complete source-proven liability upper bound, current
source-account availability, verified fee, burn/concurrency cushion, debit
ceiling and finite freshness window prove sufficiency. Label bounds as bounds;
do not claim a measured total. The query below is optional observability backlog,
not a required gate unless an actual missing safe-bound condition is demonstrated.
No implementation, deployment, funding or processing is authorized by this note.

A higher operational refill trigger/target is separate from the protected
operating floor. Raising the protected floor would increase the funding needed
to process deposits; earlier replenishment can instead preserve headroom without
changing that floor. No threshold, floor, target or automatic refill is changed.

## Existing read surface cannot return the exact total

The installed interface has these query methods:
`get_cksol_payment(owner)`, `get_cksol_deposit(owner)`,
`get_cksol_icp_payment(owner)`, `get_cksol_payment_quote()`,
`get_cksol_icp_terms()`, and `get_cksol_relay_budget()`.

At the source reference:

- `cksol_paid.rs::liabilities` sums **all** retained `refundable_cycles`.
- `cksol_paid.rs::has_backing` compares current canister balance with the
  operating reserve plus that sum plus `cksol_icp_paid::icp_protected`.
- `cksol_icp_paid.rs::icp_protected` includes its reserve allocation and
  `liquid_reserved` journal protection. ICP terms expose neither the complete
  journal sum nor legacy refundable-credit sum.
- Per-owner lookups and an opaque availability flag cannot enumerate those totals.
  Sequence counters can support a conservative bound under source invariants;
  that bound is **not** an exact liability total or a current funding quote.

Controller `canister_status` reads balance/settings/module identity, not these
application heap maps. Candid metadata reads the API schema, not current map
contents. Existing logs do not prove completeness of retained liabilities.
An existing snapshot cannot be assumed contemporaneous; creating a new snapshot
is a state-changing management action explicitly excluded from this task. No
snapshot creation/download, impersonation, speculative owner enumeration or
unbounded ledger scan was performed. No existing controller read returning the
complete application backing predicate was established.

The conservative argument and remaining execution conditions are preserved in
[the source-bound refill proof](cksol-conservative-refill-proof-20261107.md).

## Optional exact-total observability — proposal only

Add one authenticated read-only backing-status query, reusing existing operator/
controller authorization without adding roles. Compute, in **one synchronous
state read with no await**:

- `balance_cycles`, `operating_floor_cycles`, `legacy_refundable_cycles_total`;
- `icp_reserve_cycles`, `icp_liquid_reserved_cycles`, `icp_protected_cycles`;
- `required_processing_backing`, signed/split headroom or deficit, and
  `processing_backing_sufficient`;
- `required_refund_backing` and `refund_backing_sufficient`;
- observation time and available canister/build/upgrade version markers. Existing
  sequence counters may be included but must not be labelled a complete state root.

Use the exact existing arithmetic/phase filters; no owner identities or payment
rows in this aggregate response. Do not change reserves, retry behavior, custody,
funding or authentication while adding observability. Canonical Candid generation
and normal scoped verification/release controls would apply. **No code/API change
or deployment is authorized or implemented by this note.**

This query would provide exact-total backing evidence. It is not a required
prerequisite to a conservatively sized remedy or unrelated source work. The
selected remedy must clearly distinguish measured values from proven bounds.

## Prepared execution shape — incomplete, do not run

Preferred subsequent path remains the owner's **same retained funded sequence**:
source `prepare` returns false on that sequence, skipping recollection. The actual
owner must authenticate; the operator cannot impersonate that caller. Current
backing/capacity must pass before persisted relay dispatch. Unknown outcomes stay
on the same retained attempt. Refund is a separate owner choice with a different
backing predicate, not an interchangeable processing retry.

For a later approved backend operating-cycle top-up, the CLI shape is:

```text
icp canister top-up tfesu-vyaaa-aaaap-qrd7a-cai --amount APPROVED_INTEGER_CYCLES --network ic --identity rumi_identity
```

Asset/source: cycles from the named identity's default cycles-ledger account,
not ICP, the customer's wallet, or a reserve-allocation setter. Amount and total
debit ceiling are **unset**: a contemporaneous authoritative requirement, funding
account availability, current fee and explicit burn/concurrency allowance are
still needed. Future total-debit ceiling must include amount plus the verified
ledger fee. Do not execute a placeholder or infer an exact amount from one receipt.

No CLI retry-idempotency guarantee was established from the installed command
surface. Any later approved operator must persist the attempt parameters/output,
send once, and hold an uncertain result until authoritative ledger/receiver proof
reconciles that attempt; a changed balance alone is insufficient. Do not mint,
create a new funded sequence or rerun top-up to overcome uncertainty.

References: installed `icp 1.3.0` help; official CLI v1.3
[cycles/top-up distinction](https://cli.internetcomputer.org/1.3/guides/tokens-and-cycles/#cycles-transfer-vs-canister-top-up),
[cycles ledger operations and fees](https://docs.internetcomputer.org/concepts/cycles/).
No funding-account balance query or state-changing financial call was executed.

## Separate public fee observation

On November 7, 2026, immediately before 17:20:50 UTC, Ellis ran:

```text
icp canister call um5iw-rqaaa-aaaaq-qaaba-cai icrc1_fee '()' --query --network ic --identity rumi_identity
```

Native exit 0 returned `(100_000_000 : nat)`. This is the public cycles-ledger
fee observation, not source-account availability, a top-up receipt, a locked
future fee or proof that a particular CLI send used that fee. Revalidate the
applicable top-up fee before executing an eventual fixed-debit authorization.
No private customer or internal balance observation is contained here.
