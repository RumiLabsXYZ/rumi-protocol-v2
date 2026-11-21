# Vault 158: bounded execution approval packet

Owner: Adrian, replacing Bram for SEC-06 by Rob's November 7 instruction in Merritt's chat. Status: proposed; no financial execution authorized or performed by this packet. This continues the existing board and incident, not a new team or queue.

## Decision requested

Authorize **one** `admin_execute_vault158_recovery()` update on the existing liquidation bot, followed by read-only verification, with processing left paused. This single endpoint can perform both the exact ckUSDC payment/backend confirmation and the exact ICP treasury payment below. It is not merely a status query. Do not approve it if only a readback is intended.

No new deployment, preparation call, configuration change, manual ledger transfer, separate reconciliation mutation, second caller execution attempt, ordinary liquidation processing, or unpause is included. An incomplete or ambiguous outcome stops at read-only investigation; additional mutation requires a separate decision.

## Source and installed identity

- Recovery source PR [614](https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/614), merged at `99027b67a52324836dbba872a847843bf56821cc`; reviewed head `cff9f2a7d26c917431adcc8ce6db56dac42cbc09`.
- Bot: `nygob-3qaaa-aaaap-qttcq-cai`, network `ic`, existing admin identity `rumi_identity`.
- Controller-authenticated current module: `db8931273a83bb3dfbd8c516b85808b24ad5bd9daed0df9f7265b440658fe098`, Running. No rebuild or replacement is proposed.
- Backend: `tfesu-vyaaa-aaaap-qrd7a-cai`.
- Direct admin configuration readback: treasury `tlg74-oiaaa-aaaap-qrd6a-cai`; ICP ledger `ryjl3-tyaaa-aaaaa-aaaba-cai`; ckUSDC ledger `xevnm-gaaaa-aaaar-qafnq-cai`; both cached fees `10,000` raw units. This supersedes earlier uncertainty about the configured treasury.

## November 7 live preflight

Read-only checks at approximately 09:59 Pacific confirm:

- Processing paused (`true`). Recovery is `EvidencePrepared`, vault 158, claim generation 1, record 229. `payment_created_at_time` and `payment_receipt` are null.
- The only bot claim journal is generation-1 `PaymentShortfall`: gross collateral `236783835` ICP e8, received collateral `236773835`, covered debt `513841815` icUSD e8; no return attempt or proof.
- Payment journals empty; treasury intent 229 null. Backend active claim IDs contain only 158. The backend's public ID query does not expose generation; generation 1 comes from the bot's retained claim and recovery evidence. The backend independently verifies the actual payment proof before changing debt.
- Both live ledger fees are `10000`: ICP e8 and ckUSDC e6, matching the stored configuration and pinned recovery.
- Exact default-account balances: ICP `51412714` e8 and ckUSDC `31736304` e6. Subaccounts are not used by these pinned transfer tuples and are outside this reconciliation.
- ICP index had `40154111` blocks synced, matching ledger chain length `40154111`. ckUSDC index had `810526` blocks synced, matching ledger log length `810526`. Account histories were read with `start=null` and latest-first results. Thus absence of later account activity is supported by up-to-date indexes, not inferred from balances alone. Repeat these observations before execution because they can change.

## Receipt-based attribution and conservation

| Leg | Ledger block | Account amount | Additional sender fee |
| --- | --- | ---: | ---: |
| Backend to bot default account | ICP 39835209 | 236773835 e8 | 10000 e8 paid by backend |
| Bot default account to pool | ICP 39835236 | 226020933 e8 | 10000 e8 paid by bot |
| Pool to bot default account | ckUSDC 806697 | 5545309 e6 | 10000 e6 paid by pool |

The input and output share ICRC memo hex `000000000011be56`; the input spender is the recorded pool `mohjv-bqaaa-aaaag-qjyia-cai`. The persisted swap intent pins that pool, both ledgers, the input amount/fee and the pre-swap ckUSDC baseline. Its ledger receipts were verified by the completed preparation call; the current evidence record retains them. ICP 39835209 and 39835236 are consecutive transactions in this bot account's own history, and 39835236 is still its newest transaction. ckUSDC 806697 is still its newest transaction. There is no intervening or subsequent bot-account outflow to allocate elsewhere.

The claim-attributable ICP residual is therefore **236773835 - 226020933 - 10000 = 10742902 e8 (0.10742902 ICP)**. The unrelated/pre-claim balance is **40669812 e8 (0.40669812 ICP)**. Current total = unrelated balance + exact claim residual. This conclusion uses the linked claim credit, exact swap debit including fee, and no subsequent activity; a sufficient total balance by itself would not establish it.

For ckUSDC, the pre-swap baseline **26190995 e6** plus the identified **5545309 e6** output equals current **31736304 e6**. The pre-existing baseline is not allocated to recovery.

## Exact proposed effects

| Effect | Recipient / result | Net amount | Fee / gross debit |
| --- | --- | ---: | ---: |
| ckUSDC payment | Backend default account `tfesu-vyaaa-aaaap-qrd7a-cai` | **5138419 e6 = 5.138419 ckUSDC** | Fee **10000 e6 = 0.01**; gross **5148419 e6 = 5.148419** |
| Backend proof confirmation | Existing vault 158 generation-1 claim | Debt reduction **513841815 e8 = 5.13841815 icUSD**; collateral reduction **236783835 e8 = 2.36783835 ICP**; claim cleared | Only after the new exact bot-to-backend receipt passes verification |
| ICP treasury payment | Treasury default account `tlg74-oiaaa-aaaap-qrd6a-cai` | **10732902 e8 = 0.10732902 ICP** | Fee **10000 e8 = 0.0001**; gross **10742902 e8 = 0.10742902** |

The ckUSDC payment uses the ceiling conversion from eight-decimal debt to six-decimal payment. Its claim memo is `RUMI-BPAY-V2:` followed by big-endian u64 vault 158 and generation 1. Treasury memo is `RUMI:TB1:` followed by big-endian u64 record 229 and generation 1. Fresh `created_at_time` values are generated and persisted by the installed code before dispatch; no timestamp or ledger block index can be specified in advance. Approval binds those code-generated exact tuples and the stated amounts/recipients, not caller-selected payloads.

Expected after successful settlement, absent unrelated later activity:

- Bot ckUSDC **26587885 e6 = 26.587885**, consisting of preserved baseline **26.190995** plus held claim surplus **0.396890**. No sweep or allocation of that surplus is proposed.
- Bot ICP **40669812 e8 = 0.40669812**, preserving all pre-claim ICP.
- Recovery `TreasurySettled`; record 229 `Completed`; claim 158 absent from both bot pending claims and backend claim IDs; payment journal and treasury intent cleared. Paused remains `true`.
- This settles the existing partial liquidation. It does not repay all remaining vault debt or establish remaining vault health. Read and report the resulting vault state separately.

## Preconditions immediately before the approved action

Repeat the above read-only module, pause, configuration, recovery, claim, journal, ledger balance/fee and account-history queries. Require the same prepared generation-1 record, no payment timestamp/receipt or conflicting journal, same recipients/ledgers/module, exact ckUSDC balance, the reconciled ICP residual still present, and unchanged fees. On any material difference stop and report; do not deposit, sweep, change config or create another tuple to force the preconditions to pass. In particular, an unrelated ckUSDC dust deposit currently fails the installed exact-balance check. This known fail-closed liveness limitation is disclosed, not a reason to broaden approval.

## One proposed execution call, if approved

Use Candid exported from the reviewed source, with the new method present. Exact command:

```sh
icp canister call nygob-3qaaa-aaaap-qttcq-cai \
  admin_execute_vault158_recovery '()' \
  --network ic --identity rumi_identity \
  --candid /private/tmp/v158-current-approved.did
```

The Candid file is obtained without a build from:

```sh
git show 99027b67a52324836dbba872a847843bf56821cc:src/liquidation_bot/liquidation_bot.did > /private/tmp/v158-current-approved.did
```

Do not run another preparation or deployment. This endpoint persists the payment tuple and atomically promotes the local claim into its payment journal, dispatches only the pinned payment, obtains backend proof confirmation, and continues only treasury record 229. Processing stays paused; unrelated queue work does not resume.

## Readbacks and ambiguity handling

After the one update, issue read-only queries for `get_processing_paused`, `get_recovery_config`, `get_vault158_recovery`, `get_pending_claim_journals`, `get_pending_payment_journals`, `get_pending_treasury_intent(229)`, `get_liquidation(229)`, backend `get_bot_claim_vault_ids`, resulting vault state, both default-account balances, and exact newly created payment/treasury ledger blocks. Verify sender/recipient, amount, fee, memo, timestamp and block identities against the saved journals. Record all outputs and stop with the bot paused.

- A success string alone is not settlement proof; require the final states and ledger receipts above.
- If the update times out, returns an error, or its reply is lost, do not repeat it under this one-call approval. Read journals, recovery state and ledger history to determine whether either transfer or confirmation already happened. A held state may already include money at the backend or treasury.
- Do not replace a timestamp/fee tuple, assume a missing search result means no transfer, manually unlock a claim, or replay a swap. Definitive `NoEffect`, `BadFee`, incomplete confirmation, or a tuple beyond the installed safe retry window remains held. The code has no generic operator exit for every such state. Any additional execute continuation or proof-reconciliation mutation needs a separate scoped decision after exact evidence.
- No rollback/reinstall is proposed: older code could discard new recovery evidence. No unpause is included even if settlement completes.

## Existing verification reused

The installed exact bot artifact was previously built and checked: 63 bot library tests, Candid/declaration checks, Wasm endpoint check, listed PR CI, and focused independent reviews. This packet changes no source or executable. No rebuild, new test harness, broad audit or paid review was allocated. The current receipt chain closes the previously outstanding ICP attribution question. No concrete unresolved defect preventing this exact one-call proposal was found; outcome ambiguity and the disclosed liveness limits remain governed by the stop rules above.

## Approval wording

“Approve one vault 158 execution call described in this packet, including the 5.138419 ckUSDC backend payment plus its 0.01 ckUSDC fee, exact backend claim confirmation, and the 0.10732902 ICP treasury payment plus its 0.0001 ICP fee, followed by readbacks. Leave processing paused. No additional execute call, reconciliation mutation, unpause, deployment, configuration change or unrelated transaction.”

This is a proposed approval, not authority inferred from earlier upgrade-only or preparation-only approvals.
