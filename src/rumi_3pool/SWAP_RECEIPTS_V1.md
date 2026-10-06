# Swap receipts v1

`swap_with_receipt_v1` is the value-moving swap entrypoint. The legacy
`swap(i, j, dx, min_dy)` wire method remains in Candid for compatibility but
fails closed before pulling tokens. Wallet callers may use the receipt method
without admin allowlisting; anonymous callers are rejected.

The receipt-backed swap, add-liquidity, and donation ingress methods currently
return `PoolLocked` in production builds. Their full paths compile only for the
`test_endpoints` PocketIC fixture while recovery is incomplete. Candidate-index
ICRC-3 reconciliation proves a matching transfer when present, but absence at
one index is not evidence that an aged ambiguous transfer never committed.
Production admission remains disabled until a bounded fixed-tip scan can prove
complete contiguous main-log/archive coverage and safely retire the old tuple.
This is an explicit availability blocker; do not describe V1 ingress as ready
for rollout.

A receipt request binds an exactly 32-byte `intent_id`, input/output coin
indices, input amount `dx`, and net minimum received `min_dy`. IDs are scoped to
the caller and must have a strictly increasing big-endian sequence in their
first eight bytes. An identical retained request returns its existing
receipt and never starts another transfer. A conflicting request is rejected.
The frontend persists the exact ID and payload per wallet/action before update
dispatch and blocks replacing an unresolved local intent.

The receipt map uses stable MemoryId 22, the reserve fence uses 23, and the
client capability set uses 24. Existing stable IDs and SlimState/legacy state
schemas are unchanged. There are at most 10,000 retained receipts and 64 active
receipts per owner. Terminal rows may be pruned only after that caller advances
its sequence; unresolved value obligations are never evicted. A bounded global
owner high-water map currently caps distinct owners at 100,000; this is an
admission limit and requires monitoring before exhaustion.

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
unresolved evidence, never permission to replay. The stable fence is also
derived from all active stable swap/ingress rows, so a stale or reset heap flag
cannot unlock reserve mutations. Exact positive ICRC-3 block evidence can be
attached through reconciliation methods in test builds; unsupported ledgers
and any operation without a matching positive block remain held. There is no
absence-based recovery yet.

`get_swap_receipt_v1` is caller-scoped. Canister consumers should use a replicated
inter-canister call for authoritative observation. An off-chain ordinary query
response alone is not a certified settlement proof.

The additive Candid fragment can be regenerated from the Rust types with
`cargo run -p rumi_3pool --example receipt_candid`. Canonical service definitions
are in `rumi_3pool.did`; client bindings are generated with `didc bind`.
