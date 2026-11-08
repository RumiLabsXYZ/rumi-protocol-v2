# Vault 158 evidence-preparation decision — 2026-11-04

Status: proposed, not authorized or executed.

This document defines the smallest next live step for vault 158: one
preparation-only call that verifies and durably records the two existing swap
receipts. It does **not** authorize the recovery execution endpoint, any ledger
transfer, backend debt confirmation, treasury payment, reconciliation, config
change, unpause, or ordinary liquidation processing.

## Recommendation

Approve the preparation-only step below, followed by readback, then stop. This
is the narrowest useful next action because the installed code explicitly
separates receipt preparation from payment execution. Preparation creates no
outgoing transfer tuple, does not call the backend, does not remove the active
claim, and does not unpause processing
(`src/liquidation_bot/src/process.rs:502-609`). Execution is a different method
and remains subject to a later, separately pushed decision
(`src/liquidation_bot/src/process.rs:612-680`).

## Pinned source and current live preconditions

- Source baseline: `2a71ec646f22af3ebab3598a2830b6b3200b8022` on `main`.
- Recovery implementation: PR
  [#614](https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/614), merged as
  `99027b67a52324836dbba872a847843bf56821cc`.
- Operator procedure:
  [`VAULT158_RECOVERY_RUNBOOK.md`](./VAULT158_RECOVERY_RUNBOOK.md).
- Live readback at 2026-11-04 18:24 Pacific:
  - liquidation bot `nygob-3qaaa-aaaap-qttcq-cai` is Running with module
    SHA-256
    `db8931273a83bb3dfbd8c516b85808b24ad5bd9daed0df9f7265b440658fe098`;
  - `get_processing_paused = true`;
  - `get_recovery_config` reports backend
    `tfesu-vyaaa-aaaap-qrd7a-cai`, treasury
    `tlg74-oiaaa-aaaap-qrd6a-cai`, ICP ledger
    `ryjl3-tyaaa-aaaaa-aaaba-cai`, ckUSDC ledger
    `xevnm-gaaaa-aaaar-qafnq-cai`, and both cached fees as `10,000` raw units;
  - `get_vault158_recovery = null`;
  - the only pending claim journal is vault 158, generation 1,
    `PaymentShortfall`, with 236,783,835 e8 gross ICP, 236,773,835 e8 received,
    and 513,841,815 e8 debt;
  - pending payment journals are empty and treasury intent 229 is null;
  - the backend still lists vault 158 as an active bot claim;
  - the bot ckUSDC balance is exactly 31,736,304 e6 and the live ckUSDC fee is
    10,000 e6; and
  - the bot ICP balance is 51,412,714 e8 and the live ICP fee is 10,000 e8.

The ICP balance is not an authorization or an attribution proof. It is not
needed for preparation, and execution remains held until the claim-attributable
ICP residual is reconciled as required by the runbook.

The runbook's older statement that the configured treasury principal was still
unverified is superseded by the 18:24 Pacific `get_recovery_config` readback
above. Any later execution decision must nevertheless repeat that readback and
the live fee checks rather than relying on this dated observation.

## Exact authorized call, if approved

Run from this repository revision with the existing bot-admin identity:

```sh
icp canister call nygob-3qaaa-aaaap-qttcq-cai \
  admin_prepare_vault158_recovery '()' \
  --network ic \
  --identity rumi_identity \
  --candid src/liquidation_bot/liquidation_bot.did
```

This update method re-verifies:

- the exact vault, generation, claim, memo, single swap intent, and history
  record 229 (`src/liquidation_bot/src/process.rs:301-359`);
- ICP block 39,835,236 for 226,020,933 e8 plus its 10,000 e8 fee;
- ckUSDC block 806,697 for 5,545,309 e6;
- the exact 31,736,304 e6 bot ckUSDC allocation; and
- the absence of conflicting claim/payment recovery work
  (`src/liquidation_bot/src/process.rs:281-295,505-560`).

On success, it stores a durable recovery evidence record in `EvidencePrepared`
status containing the two receipts, the 5,138,419 e6 future net debt payment,
its 10,000 e6 fee, the 5,148,419 e6 future gross debit, and the 396,890 e6 held surplus
(`src/liquidation_bot/src/process.rs:561-609`). It does not dispatch those
amounts.

## Mandatory readback and stop point

Immediately after the call, run only these queries:

```sh
icp canister call nygob-3qaaa-aaaap-qttcq-cai get_processing_paused '()' \
  --query --network ic --identity rumi_identity \
  --candid src/liquidation_bot/liquidation_bot.did
icp canister call nygob-3qaaa-aaaap-qttcq-cai get_vault158_recovery '()' \
  --query --network ic --identity rumi_identity \
  --candid src/liquidation_bot/liquidation_bot.did
icp canister call nygob-3qaaa-aaaap-qttcq-cai get_pending_claim_journals '()' \
  --query --network ic --identity rumi_identity \
  --candid src/liquidation_bot/liquidation_bot.did
icp canister call nygob-3qaaa-aaaap-qttcq-cai get_pending_payment_journals '()' \
  --query --network ic --identity rumi_identity \
  --candid src/liquidation_bot/liquidation_bot.did
icp canister call nygob-3qaaa-aaaap-qttcq-cai get_pending_treasury_intent \
  '(229 : nat64)' --query --network ic --identity rumi_identity \
  --candid src/liquidation_bot/liquidation_bot.did
icp canister call tfesu-vyaaa-aaaap-qrd7a-cai get_bot_claim_vault_ids '()' \
  --query --network ic --identity rumi_identity \
  --candid src/rumi_protocol_backend/rumi_protocol_backend.did
```

Expected result:

- processing remains paused;
- `get_vault158_recovery` contains the exact pinned record with status
  `EvidencePrepared` and no payment timestamp or payment receipt;
- vault 158 remains the generation-1 `PaymentShortfall` pending claim;
- pending payment journals remain empty;
- treasury intent 229 remains null; and
- the backend still lists vault 158.

Stop there. Do not call `admin_execute_vault158_recovery`, either reconciliation
method, `set_config`, or `set_processing_paused(false)`.

## Failure and ambiguity rules

- If any precondition differs before the call, do not call preparation.
- If preparation returns a typed error, preserve the hold and report the exact
  error. Do not change config or balances to make it pass.
- If the update reply is lost or ambiguous, query `get_vault158_recovery`
  before considering another call. A present exact `EvidencePrepared` record is
  success evidence; a repeated preparation call is expected to reject an
  already-consumed receipt record (`src/liquidation_bot/src/process.rs:505-511`).
- Any mismatch in the post-call readback stops the procedure. Do not proceed to
  execution.

## Explicitly outside this approval

- `admin_execute_vault158_recovery`;
- the future 5,138,419 e6 ckUSDC backend payment and its 10,000 e6 fee;
- the future 10,732,902 e8 ICP treasury payment and its 10,000 e8 fee;
- backend debt write-down, claim removal, or history completion;
- payment or treasury block reconciliation;
- unpausing the bot or processing another liquidation; and
- any config, controller, Wasm, ledger, or balance change other than the single
  preparation record described above.

An exact approval can therefore be limited to: **one
`admin_prepare_vault158_recovery()` call plus the listed readbacks, with a hard
stop at `EvidencePrepared`; no execution, reconciliation, unpause, or funds
movement.**
