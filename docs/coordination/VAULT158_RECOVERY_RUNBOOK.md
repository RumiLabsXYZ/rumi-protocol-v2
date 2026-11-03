# Vault 158 receipt recovery runbook

This is an operator procedure for the exact vault 158, generation 1, history
record 229 recovery. The commands below are examples for a later authorized
operator session. They were **not executed** during implementation; no canister
state, backend state, ledger, or funds were changed.

## Expected evidence and allocation

The bot is `nygob-3qaaa-aaaap-qttcq-cai`; the backend is
`tfesu-vyaaa-aaaap-qrd7a-cai`. Preparation accepts only the saved generation-1
PaymentShortfall claim and history record 229, with the pinned single swap
intent and these two ledger receipts:

- ICP ledger `ryjl3-tyaaa-aaaaa-aaaba-cai`, block `39835236`: bot to pool,
  226,020,933 e8 ICP plus the 10,000 e8 input fee, with the pinned native ICP
  memo and transferFrom spender.
- ckUSDC ledger `xevnm-gaaaa-aaaar-qafnq-cai`, block `806697`: pool to bot,
  5,545,309 e6 ckUSDC; the pool paid the 10,000 e6 ledger fee.

The saved claim is 236,783,835 e8 gross ICP, 236,773,835 e8 net after its
10,000 e8 outbound fee, against 513,841,815 e8 debt. The claim proceeds yield
5,138,419 e6 ckUSDC net debt payment plus its 10,000 e6 fee, so the payment
debit is 5,148,419 e6. The 5,545,309 e6 swap output leaves 396,890 e6 held
surplus. The prior ckUSDC balance is 26,190,995 e6. Preparation and first
execution require the current bot balance to equal exactly 31,736,304 e6
(prior balance plus the pinned output); an unexpected deposit or debit fails
closed and cannot authorize the payment.

The separate ICP treasury gross residual is 10,742,902 e8:
236,773,835 - 226,020,933 - 10,000. At a 10,000 e8 ICP ledger fee, the
persisted treasury tuple sends 10,732,902 e8 and debits 10,742,902 e8 gross.
The bot creates that treasury tuple only after ckUSDC payment confirmation.

## Operator sequence

Run the following only from an authorized admin identity after reviewing the
live configuration and exact recovery authority. Every `dfx` command names
`--network ic` explicitly so none can fall through to a local network; keep that
flag on any additional `dfx` call, including `admin_reconcile_payment_block` and
`admin_reconcile_treasury_block`. Execute explicitly authorizes
the bot-to-backend ckUSDC payment, backend confirmation, and the pinned
record-229 ICP treasury continuation. Preparation and installation authorize
no transfer.

```sh
dfx canister call --network ic nygob-3qaaa-aaaap-qttcq-cai get_processing_paused '()'
dfx canister call --network ic nygob-3qaaa-aaaap-qttcq-cai set_processing_paused '(true)'
dfx canister call --network ic nygob-3qaaa-aaaap-qttcq-cai get_vault158_recovery '()'
dfx canister call --network ic nygob-3qaaa-aaaap-qttcq-cai admin_prepare_vault158_recovery '()'
dfx canister call --network ic nygob-3qaaa-aaaap-qttcq-cai get_vault158_recovery '()'
```

Review the durable evidence and `EvidencePrepared` status before separately
authorizing execution. Preparation verifies the two ledger blocks and exact
ckUSDC balance, then stores the receipts. It does not create an outgoing
payment tuple, call the backend, remove the active claim, or unpause processing.

```sh
dfx canister call --network ic nygob-3qaaa-aaaap-qttcq-cai admin_execute_vault158_recovery '()'
dfx canister call --network ic nygob-3qaaa-aaaap-qttcq-cai get_vault158_recovery '()'
dfx canister call --network ic nygob-3qaaa-aaaap-qttcq-cai get_pending_payment_journals '()'
dfx canister call --network ic nygob-3qaaa-aaaap-qttcq-cai get_pending_treasury_intent '(229 : nat64)'
```

The first execution creates and persists the ckUSDC timestamp, fee, amount, and
generation-bound memo before dispatch. It also promotes the exact claim out of
the pending-claim map in the same stable-state mutation. Repeated execution
uses this saved tuple. After backend confirmation, execution creates and
continues only the saved treasury tuple for record 229 while processing stays
paused. Do not create a fresh payment tuple or manually remove the claim.

## Pre-execution checks (read-only)

- Backend claim: `get_bot_claim_vault_ids` on the backend must still list 158.
  The backend releases a claim only for a payment proof or a bot-supplied,
  verified collateral-return proof. The bot refuses execution unless its claim
  has no return attempt, so nothing but this recovery can release the backend
  claim while the bot is paused.
- Treasury recipient: the ICP residual goes to the bot's configured
  `treasury_principal`. The bot interface does not expose its config, and the
  treasury tuple (readable with `get_pending_treasury_intent`) exists only after
  payment confirmation, in the same execution that attempts the transfer. Config
  changes are rejected while the claim is pending, so the value cannot drift
  before execution. This runbook has no direct bot config or init evidence for
  the value. `tlg74-oiaaa-aaaap-qrd6a-cai` (rumi_treasury) is only a
  **candidate**; the release decision must independently verify the bot's
  actual configured `treasury_principal` (for example from the last reviewed
  `set_config`/init arguments) before authorizing execution.
- ICP funding: a bot ICP ledger balance of at least 10,742,902 e8 (the gross
  residual, which includes the 10,000 e8 fee) is necessary but does not prove
  that ICP attributable to this claim remains: the bot's account is shared, so
  other funds can cover the amount. Before release, reconcile the claim's ICP
  receipt (block `39835236`) and all subsequent ICP activity on the bot account
  against the current balance, and do not treat balance alone as isolation. A
  shortfall the ledger rejects returns a definitive `NoEffect` and is handled
  as described below.
- ckUSDC balance: preparation and first execution accept only the exact
  pre-swap balance plus output (31,736,304 e6). Any unexpected deposit or debit,
  including a dust deposit by a third party, makes both calls fail closed. There
  is no operator override; clearing it needs a reviewed source change.

## Held states and how they resolve

Every held state keeps processing paused and allocates no new tuple; none
unpauses the bot. Once the ckUSDC payment has reached the backend, funds may
already have moved (payment, backend credit, possibly the treasury transfer), so
"held" does not mean funds are in place. In every case the ICRC ledger blocks and
the bot and backend journals govern what actually happened; check them rather
than assuming a state.

- **Payment `Ambiguous`, or unreceipted past the 23-hour dedup window:** inside
  the window, repeating execution re-sends the identical saved tuple, which the
  ledger deduplicates. Past the window the bot will not re-dispatch. Find the exact ckUSDC block in ICRC-3 history (bot to
  backend, 5,138,419 e6, the saved memo and `created_at_time`) and call
  `admin_reconcile_payment_block(158, <block>)`. The backend verifies the whole
  tuple before accepting it. If no such block exists, there is nothing to
  reconcile.
- **Payment `NoEffect`:** the ledger definitively rejected the saved tuple. The
  generic retry endpoint is refused for vault 158 because a new timestamp would
  change the authorized tuple, and repeating execution only reports the hold.
  There is no supported operator exit unless a block can be reconciled as above;
  the bot stays paused with funds held until a separately reviewed source change.
- **Treasury `NoEffect` or `Ambiguous`:** repeating
  `admin_execute_vault158_recovery` re-sends the same saved treasury tuple while
  it is inside the 23-hour window. After the window, or if the outcome stays
  ambiguous, reconcile the exact block with
  `admin_reconcile_treasury_block(229, <block>)`. If none exists, the same
  no-supported-exit rule applies.
- **Backend confirmation rejected after payment:** the receipt is kept, and
  repeating execution re-submits the same proof. The backend returns success for
  an already-consumed matching proof, so repeating is safe.

No operator cancellation exists in this procedure, and none should be
improvised solely because an effect failed, no receipt was found, or the outcome
is uncertain. `NoEffect` is specific to the saved tuple, a missing block in a
search does not prove the transfer never landed, and abandoning or replacing the
tuple could duplicate a payment that later appears or desynchronize the claim.
Resolve uncertainty by evidence (ledger blocks, journals, reconciliation above);
anything else needs a separately reviewed source change.

Repeat `admin_execute_vault158_recovery` to continue the same saved treasury
tuple when its readback remains pending. Confirm `TreasurySettled` in
`get_vault158_recovery` and completed history record 229 before separately
calling `set_processing_paused(false)`. No install, restart, timer, or
preparation step automatically transfers funds or unpauses the bot.
