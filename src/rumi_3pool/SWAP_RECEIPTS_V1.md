# Swap receipts v1

`swap_with_receipt_v1` is additive. Existing `swap(i, j, dx, min_dy)` callers keep
its wire signature, pricing, fee allocation, and gross-output return value.
Both entrypoints use `swap_inner` for pricing and reserve accounting.

Receipt clients must first be enabled by the pool admin through
`set_swap_receipt_client_v1(client, enabled)`. The stable capability set starts
empty and holds at most 64 clients. `is_swap_receipt_client_v1` exposes this
capability. Source delivery does not enable any client.

A receipt request binds an exactly 32-byte `intent_id`, input/output coin
indices, input amount `dx`, and net minimum received `min_dy`. IDs are scoped to
the authenticated caller. An identical retained request returns its existing
receipt and never starts another transfer. A conflicting request is rejected.
Revoking a client blocks submissions, including duplicates, but preserves that
client's access to `get_swap_receipt_v1(intent_id)`.

The receipt map uses stable MemoryId 22, the reserve fence uses 23, and the
client capability set uses 24. Existing stable IDs and SlimState/legacy state
schemas are unchanged. There are at most 10,000 receipts. Rows are never evicted;
capacity is checked before any transfer and permanently rejects new intents
when exhausted. Existing receipt lookup remains available. This bound requires
an explicit future capacity/retention design before exhausting 10,000 attempts.

Each transfer records the ledger, default source/destination accounts, credited
amount, explicit fee, creation timestamp, memo, status, and the block ID returned
by the ledger (`Ok`, or `Duplicate.duplicate_of`). Input sender cost is `dx +
input.fee`. Output credited amount is `gross_output - output.fee`. `pool_fee`
is the swap fee in output-token units, distinct from ledger fees. A successful
refund credits `dx - refund.fee`, preserving the existing refund economics.
The returned block ID binds these arguments; a missing ID never proves a debit
or credit. The caller cannot supply settlement assertions or block IDs.

Memo derivation is SHA-256 over the concatenation of:

1. UTF-8 `rumi-3pool-swap-receipt-v1`;
2. one byte containing the caller principal byte length;
3. caller principal bytes;
4. the 32 intent bytes;
5. one leg byte: input=0, output=1, refund=2.

The request is reserved before execution. Each transfer's full arguments and
Submitted status are persisted before its ledger call. The stable reserve fence
is set before the input call. Only a confirmed completed swap, confirmed refund,
or definite input rejection clears it. A transport rejection, generic ledger
error, or transient ledger error is conservatively Unresolved. A definite output
rejection permits a refund; failed or uncertain refunds retain the fence. No
uncertain submission is automatically retried or compensated.

A callback trap or upgrade may retain a Submitted receipt. Such a receipt is
unresolved evidence, never permission to replay. The stable fence blocks pool
reserve mutations, including the legacy swap and pending-claim paths, even after
an upgrade. Ordinary admin unpause cannot clear it. There is deliberately no
operator-assertion recovery endpoint; clearing unresolved custody work requires
a future implementation based on verified ledger evidence and correct reserve
accounting.

`get_swap_receipt_v1` is caller-scoped. Canister consumers should use a replicated
inter-canister call for authoritative observation. An off-chain ordinary query
response alone is not a certified settlement proof.

The additive Candid fragment can be regenerated from the Rust types with
`cargo run -p rumi_3pool --example receipt_candid`. Canonical service definitions
are in `rumi_3pool.did`; client bindings are generated with `didc bind`.
