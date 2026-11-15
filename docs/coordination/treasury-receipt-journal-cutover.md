# Treasury receipt-journal cutover notes

This change adds receiver-side at-most-once bookkeeping for non-icUSD transfers.
The Treasury records a transfer only after the reporting canister has positively
verified the exact ledger block. The Treasury itself does not treat a reporter's
claim as cryptographic proof.

## Upgrade and enablement

1. Keep Treasury paused during the upgrade and compatibility readback.
2. On upgrade from a version predating the generalized transfer receipt index,
   inspect `get_external_token_deposit_backfill_status`. The new schema starts a
   bounded scan of all historical non-icUSD deposit rows, not only CK stablecoin
   rows. Repeatedly call `continue_external_token_deposit_backfill` until the
   status reports `Complete`; ingress stays held during the scan.
3. A newly initialized Treasury authorizes only its configured controller
   principal for transfer reports. An upgraded Treasury defaults the new
   reporter field to empty. As a Treasury controller, explicitly authorize the
   backend and liquidation bot with `set_transfer_reporter(principal, true)`.
4. Read back `get_transfer_reporters` and compare its exact principal set with
   the intended backend and bot. Do not infer authorization from controller
   membership. The reporter endpoint can book receipts but cannot withdraw.
5. Resume Treasury ingress only after the generalized block backfill is complete
   and reporter configuration is read back. Keep any transfer whose outcome or
   receipt proof is ambiguous held in its source journal.

## Replay and collision handling

`record_reported_transfer_once` binds the reporter operation ID to an immutable
ledger, block, asset, deposit type, amount, and memo tuple. A repeated exact
operation returns its original deposit ID. A conflicting tuple for the same
operation ID is rejected. The physical `(ledger, block)` receipt index also
prevents a second operation ID from crediting one block twice. Historical rows
without exact ledger provenance are retained as block tombstones and cannot be
claimed by a new receipt.

The backend liquidation-fee outbox is admitted synchronously after the
liquidation state transition and before the next await, keyed by the committed
liquidation payout nonce. It persists the exact transfer tuple before dispatch.
The bot keeps its `ReceiptObserved` journal until Treasury booking is positively
acknowledged; reply loss retries the same deterministic operation ID and block.

This procedure does not reconcile older deposits that may have been transferred
but never booked. Those liabilities require a separate ledger-to-Treasury audit.
