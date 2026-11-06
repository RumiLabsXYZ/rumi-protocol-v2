# SEC-11: repeat native-XRP deposit accounting — deferred design

Status: **DEFERRED, NOT COMPLETE**. Ellis accepted deferring the reusable address
API/UI and repeat-credit action in the bounded SEC-10 patch. This is not a SEC-10
shipping gate. No interface, frontend receive flow, state migration, live read,
signing or funds action is implemented or authorized here. The example below is
synthetic and contains no customer data.

## Current source and missing evidence

- Initial confirmation credits the observed balance net of reserve, opens the
  vault at the observed ledger index, and removes the pending address entry:
  [vault.rs](../../src/rumi_protocol_backend/src/vault.rs#L3725) and
  [pending removal](../../src/rumi_protocol_backend/src/vault.rs#L3756).
- The current address derivation endpoint is developer-only, rather than a user
  reusable-deposit API:
  [main.rs](../../src/rumi_protocol_backend/src/main.rs#L10158).
- The original custody path is `[chain_id LE, owner principal, vault nonce LE]`:
  [ted25519.rs](../../src/rumi_protocol_backend/src/chains/xrp/ted25519.rs#L26).
  Existing settlement uses the claim's saved custody owner and nonce, not its
  eventual payout recipient.
- Claims retain their gross amount, custody binding and pending settlement
  hash/sequence only while present:
  [XrpClaim](../../src/rumi_protocol_backend/src/state.rs#L1997),
  [XrpSettlement](../../src/rumi_protocol_backend/src/state.rs#L2025).
  A proven paid outcome removes the claim:
  [reconciliation](../../src/rumi_protocol_backend/src/vault.rs#L5059).
- The native withdrawal event records an accounting withdrawal, using block 0
  before later settlement; it is not a retained XRPL payout receipt:
  [event recorder](../../src/rumi_protocol_backend/src/event.rs#L4408).
  No durable per-custody paid-outflow/frontier record was found in this source
  assessment. Subtracting current collateral and unresolved claims is therefore
  insufficient to determine newly received funds from an arbitrary account read.

## Stale pre-payout double-credit example

All amounts here are illustrative XRP. Start with account balance 11, reserve 1,
and recorded collateral 10. A withdrawal of 4 leaves recorded collateral 6 and
an unresolved gross claim of 4. The balance still reads 11 before payout.

After the claim pays, the account holds 7 (the gross 4 covers recipient value
plus network fee), recorded collateral remains 6, and the paid claim disappears.
A delayed/stale validated `account_info` observation from the pre-payout ledger
still says 11. The naive calculation `11 - 1 - 6 - 0 = 4` now falsely labels an
already-paid withdrawal as a new deposit. A second credit would create collateral
without custody backing. A per-call mutex alone does not prove the remote sample
is newer than historical payout finality.

## Minimal future primitive and admission shape

Before advertising a reusable receive address, define a durable record keyed by
the original custody identity `(custody_owner, vault_id)`, with:

1. The original derivation/network/key binding and current authorized vault-owner
   relationship. Re-derive from the original custody owner and vault ID; never
   substitute a newly connected principal, recipient or caller-supplied address.
2. An accounted validated-ledger frontier (index and hash, with account sequence
   where relevant), plus the balance/accounting checkpoint needed to calculate
   the next delta. Never accept a sample older than the last accounted payout.
3. Durable gross outflow and fee receipts/tombstones for paid/failed-fee-charged
   settlements, and unique transfer/sequence evidence. Claim deletion must not
   erase evidence needed to exclude previous outgoing value from a later credit.
4. A per-custody mutation generation/reservation covering withdrawal, liquidation,
   redemption, settlement and administrative reconciliation, including upgrades
   and lost callbacks. Repeated observations cannot book the same delta twice.

An eventual confirm must serialize on that custody/vault while reading the
account, reject or reconcile unresolved outflows, require a validated observation
at or beyond the stored frontier, and revalidate current owner/config/status,
claims, generation and available net delta in the same synchronous block as
credit and frontier advancement. Existing per-vault guards are useful, but need
persistent receipt/frontier evidence and invalidation on every custody mutation.
No broad accounting framework or migration is proposed by this patch.

A separate existing-owner address view can remain read-only, but must bind the
current authenticated owner to the original custody derivation and recheck after
any derivation await. Until repeat-credit accounting exists, it must not be
presented as a reusable deposit action.

## Historical reconstruction

Current vaults and economic events can help recover initial custody bindings and
credited/debited collateral. The initial open records an observed ledger index.
They do **not** independently reconstruct all historical XRPL gross outflows,
charged failure fees, final ledger positions, and removed settlement hashes.
Paid claims are removed, and native accounting withdrawal events use block 0.

Existing custody accounts therefore require explicit chain reconciliation against
complete validated account/transaction history, cross-checked with retained claims
and protocol events, before establishing a trustworthy frontier/checkpoint. If
history or mapping is incomplete, retain a hold; do not seed a new zero-outflow
record or infer that every current balance excess is a new deposit. No such
reconciliation was performed here. A separately accepted bounded implementation
plan must classify this bootstrap evidence before any reusable receive UI ships.
