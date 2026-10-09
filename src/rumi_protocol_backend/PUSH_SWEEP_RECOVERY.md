# Caller-subaccount push sweep recovery

The backend stores one exact in-flight push-sweep intent per owner before it
dispatches `icrc1_transfer`. The intent binds the owner and requested vault
operation to the ledger, caller-derived source subaccount, protocol main
account, amount, explicit fee, memo, and `created_at_time`. An unresolved
intent blocks another push sweep for that owner. For add-margin intents it
also blocks competing vault mutations until the owner retries the same
operation and the ledger returns a block or `Duplicate` for the exact tuple.

`BadFee` on the first dispatch is a definitive no-effect and permits a new
intent with the refreshed fee. After any ambiguous attempt, a changed fee,
`TooOld`, or another non-positive response does not clear the journal or
change its tuple. This may leave an intent held indefinitely after the ledger's
deduplication horizon; no absence-of-transfer inference is made.

This is forward-only migration. Older stable snapshots decode with an empty
journal. They do not contain enough operation data to enumerate or reconstruct
historical ambiguous sweeps, and this change does not backfill, auto-credit, or
certify recovery for them. Those pre-journal cases remain unresolved historical
liabilities requiring independent ledger-history reconciliation and owner
attribution before any manual remedy. No such inventory or reconciliation is
performed by the journal migration.
