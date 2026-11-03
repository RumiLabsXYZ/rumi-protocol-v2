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
live configuration and exact recovery authority. Execute explicitly authorizes
the bot-to-backend ckUSDC payment, backend confirmation, and the pinned
record-229 ICP treasury continuation. Preparation and installation authorize
no transfer.

```sh
dfx canister call nygob-3qaaa-aaaap-qttcq-cai get_processing_paused '()'
dfx canister call nygob-3qaaa-aaaap-qttcq-cai set_processing_paused '(true)'
dfx canister call nygob-3qaaa-aaaap-qttcq-cai get_vault158_recovery '()'
dfx canister call nygob-3qaaa-aaaap-qttcq-cai admin_prepare_vault158_recovery '()'
dfx canister call nygob-3qaaa-aaaap-qttcq-cai get_vault158_recovery '()'
```

Review the durable evidence and `EvidencePrepared` status before separately
authorizing execution. Preparation verifies the two ledger blocks and exact
ckUSDC balance, then stores the receipts. It does not create an outgoing
payment tuple, call the backend, remove the active claim, or unpause processing.

```sh
dfx canister call nygob-3qaaa-aaaap-qttcq-cai admin_execute_vault158_recovery '()'
dfx canister call nygob-3qaaa-aaaap-qttcq-cai get_vault158_recovery '()'
dfx canister call nygob-3qaaa-aaaap-qttcq-cai get_pending_payment_journals '()'
dfx canister call nygob-3qaaa-aaaap-qttcq-cai get_pending_treasury_intent '(229 : nat64)'
```

The first execution creates and persists the ckUSDC timestamp, fee, amount, and
generation-bound memo before dispatch. It also promotes the exact claim out of
the pending-claim map in the same stable-state mutation. Repeated execution
uses this saved tuple. After backend confirmation, execution creates and
continues only the saved treasury tuple for record 229 while processing stays
paused. If a tuple is ambiguous, inspect its pending journal and reconcile the
exact ledger block through the existing admin reconciliation endpoint. A
`NoEffect` payment remains held; the generic retry endpoint is refused for
vault 158 because changing its timestamp would change the authorized tuple.
Do not create a fresh payment tuple or manually remove the claim.

Repeat `admin_execute_vault158_recovery` to continue the same saved treasury
tuple when its readback remains pending. Confirm `TreasurySettled` in
`get_vault158_recovery` and completed history record 229 before separately
calling `set_processing_paused(false)`. No install, restart, timer, or
preparation step automatically transfers funds or unpauses the bot.
