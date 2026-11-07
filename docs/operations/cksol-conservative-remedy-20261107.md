# SEC-09 — conditional 8T backend refill approval packet

Preparation only; NOTHING has executed. Ellis owns the case. This packet is a
single manual refill proposal, not automatic replenishment or customer processing.
Source: `6847982b44f247e0c99f0a453b583d3fab8107a2`; see the linked
[conservative bound proof](cksol-conservative-refill-proof-20261107.md).

## Requested approval and private binding

Approve exactly ONE cycles-ledger withdrawal/top-up delivering
**8,000,000,000,000 cycles** to backend **tfesu-vyaaa-aaaap-qrd7a-cai** on **ic**.
Ledger: **um5iw-rqaaa-aaaaq-qaaba-cai**. Fee must equal **100,000,000 cycles**;
total source debit must not exceed **8,000,100,000,000 cycles**.

Source is the existing default cycles-ledger account fixed in the private
execution appendix retained in Ellis's authorized review channel. That appendix
fixes the identity alias, derived principal, default subaccount and source balance
read evidence. It must be reviewed together with this public packet, not selected
by the executor at run time. No source/customer principal or private balance is
published here. Re-derive the principal and require the exact private binding.

Approval covers that one top-up and the read-only preflight, ledger reconciliation
and post-checks below. It EXCLUDES ICP conversion, transfers to the customer,
reserve allocation/activation, changed operating floor/settings, deployment,
processing/mint calls, refunds, recovery, automatic refills and sending messages.
The owner's later processing interaction is explicitly separate and never
performed by impersonating that owner.

Authorization expires **2026-11-08 20:00:00 UTC**. No execution before approval.
A failed guard does not authorize a larger refill, another source or another send.

## Exact mathematical guard

Let O=10,000,000,000,000 be the unchanged operating floor. Require the legacy
quote's next_sequence=5 and unchanged attachment/refund terms, so its monotonic
sequence is 4 and the proven legacy upper bound is
Lmax=4,000,400,000,000. Require ICP next_sequence=1 and reserve_available_cycles=0,
which imply Pmax=0 under the verified initialization/write invariants.
These are execution PRECONDITIONS, not an exact census of actual liabilities.

Use an explicit operational cushion C=500,000,000,000 cycles. Then:

- required target O+Lmax+Pmax+C = **14,500,400,000,000**;
- delivering D=8,000,000,000,000 requires preflight B >= **6,500,400,000,000**;
- B+D >=14,500,400,000,000 covers the bound and the 0.5T allowance.

The cushion is an allowance, not a worst-case adversarial burn guarantee. Runtime
backing and capacity checks remain authoritative. No guarantee of mint completion
or continued affordability after a long owner delay is made.

## Preflight — all must pass, otherwise STOP

Record wall-clock UTC start/end and raw native-exit results privately. All reads
below must be no more than **60 seconds old at the first top-up submission**.
If expired before submission, repeat ONLY read-only preflight. After any send or
uncertain send, do not restart as a fresh attempt.

1. Backend Running, module exactly
   `d38887253da67b405f5baf9cb3a53b377a39f7f1a227aced422f97565bceaa10`.
   No concurrent module/config/reserve change is authorized by this packet.
2. Legacy quote: next_sequence=5, funding_cycles=1,001,000,000,000,
   refund_fee_reserve=100,000,000 and original fixed attachment semantics.
   ICP terms: next_sequence=1, reserve_available_cycles=0, available=false.
   Bracket other reads with a repeated quote/terms check; stop on any change.
3. The privately bound existing customer payment is still Funded at its retained
   sequence, with the same funding reference/refundable credit, no deposit/refund
   ID and no relay attempt. These are backend references, not independent ledger
   proof of original funding. Query only, never start a new request during preflight.
4. Final backend cycles B >=6,500,400,000,000. This is an individually timed status
   read, not an atomic application snapshot. No protected-floor change is allowed.
5. Bound source identity/principal/default account match the private appendix,
   spendable cycles >=8,000,100,000,000, public ledger fee exactly100,000,000.
   Check installed CLI/top-up semantics and abort if the fee/ledger behavior or
   identity binding changed. Existing fee is a fixed ledger charge, not a quoted
   ICP price. No ICP minting is authorized.
6. Persist a unique private local attempt record BEFORE sending: approved packet
   commit/hash, target/network/amount/source binding, time, preflight receipts and
   status=Prepared. Ensure no previous submitted/unknown/successful attempt exists.
   Do not infer safety from a missing terminal response.

Concurrency is not globally frozen: a state change after reads may still make
processing refuse safely. Recheck terms after delivery and immediately before
any later owner processing guidance; never silently widen this authorization.

## One approved financial action — not run during preparation

After loading the frozen private identity binding, the exact CLI shape is:

```sh
icp canister top-up tfesu-vyaaa-aaaap-qrd7a-cai --amount 8000000000000 --network ic --identity "$APPROVED_SOURCE_IDENTITY"
```

This is a cycles-ledger withdrawal delivering cycles to the canister, not an
ICRC transfer credit to the backend's ledger account. `icp cycles transfer` and
reserve setters are not substitutes. BEFORE invoking the CLI, durably persist and flush
status=SubmittedOrUnknown with the SAME attempt ID and exact parameters. Then
invoke once and capture stdout, stderr and native exit. A crash leaving Prepared
is NOT proof that no send occurred; reconcile the attempt before deciding anything.
No retries or wrappers that automatically repeat the command.

The CLI has no exposed fee-cap or retry-idempotency flag in the examined help.
The ledger withdraw interface charges its fixed fee and has no fee argument;
this proposal relies on unchanged ledger semantics and the preflight fee check.
If the ledger/CLI semantics cannot be established unchanged, stop before sending.
An external ledger upgrade race is a residual dependency; no ledger-upgrade
administration is authorized. Never charge more than the approved ceiling by
choosing a different operation or increasing the amount.

## Verification, failure and unknown outcome

- Require an authoritative successful withdrawal block/receipt binding the private
  source, target canister, amount and charged fee; follow only that receipt for
  failed delivery/refund evidence. Preserve raw response and ledger block proof.
- Independently verify receiver status and source debit. Receiver delta alone is
  insufficient because concurrent burn and other funds can change the balance.
- Require post-delivery backend B >=14,500,400,000,000 and unchanged guarded terms
  before treating this as a usable processing window. If lower, report the actual
  result and STOP; no second refill is authorized.
- Transport timeout, crash, non-final response or uncertain delivery => mark
  Unknown; read/reconcile that attempt through authoritative ledger and receiver
  evidence. Do NOT resubmit, switch identity, mint cycles, issue a refund or create
  a new payment sequence to resolve uncertainty. Escalate an unresolved result.
- A proven rejection can still charge a fee. Preserve any failure/refund receipt;
  a second attempt requires a new exact decision. No blind retry even on exit!=0.

## Expected customer step — separate authorization/owner interaction

After verified backing and communication approval, explain that the original
cycles payment is retained. The same Internet Identity owner resumes the SAME
existing funded request in Bridge > Mint ckSOL > Continue minting; do not resend
SOL, approve another cycles payment, or select a new sequence. The backend call
is `start_cksol_deposit_paid(existing_sequence, 1001000000000)` as that actual
owner. Same-sequence prepare returns false, so it skips funding collection.

The minter/relay submission is a separate financial interaction by the owner;
not part of this operator top-up approval. Capacity, backing and current payment
state must still pass. Unknown minter outcome stays on the same retained attempt.
Processing, payment refund and minted ckSOL status must be verified separately;
this packet does not prove any of them merely from an increased backend balance.
Refund is a different owner choice, not silently substituted for processing.

## Read evidence and product boundaries

The November 7 preparation reads and exact private account/customer bindings are
retained in Ellis's review channel; they are not a future freshness guarantee.
All execution guards must be re-read after approval. Public ledger fee read was
reconfirmed during preparation; raw private balances/IDs are omitted here.

XRP source PR #628 is merged, with deployment/customer outcome still unverified.
Existing XRP address/repeat-credit UX remains deferred; do not advertise safe
repeat deposits from an address display alone. No XRP action is approved here.

A higher operational refill trigger/target could reduce recurrence while leaving
O unchanged. Example future policy: trigger on headroom above protected backing,
target a disclosed additional operating cushion and notify on sustained burn.
This is backlog discussion only; no monitor, automatic refill or setting change.

## Primary references

- Rumi bound/source proof linked above and pinned source in this packet.
- Installed `icp canister top-up --help` and `icp cycles balance --help`.
- [Official cycles-ledger withdraw interface](https://github.com/dfinity/cycles-ledger/blob/main/cycles-ledger/cycles-ledger.did).
- [Official fixed-fee source](https://github.com/dfinity/cycles-ledger/blob/main/cycles-ledger/src/config.rs).
These upstream references explain semantics; they are not proof of live deployed
ledger module identity. An unexpected live behavior/fee is a stop condition.
