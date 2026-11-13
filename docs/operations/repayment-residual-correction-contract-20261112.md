# SEC17 repayment residual correction — accepted source/preparation scope

Owner: Ellis, with Rowan backend implementation and Iris independent accounting review. Rob's direct CTO instruction authorizes fixing and double-checking repayment logic, not deploying a new backend. The existing live frontend240/version1794522798107, backend730f5471 and prospective Season2 remain unchanged.

## Accepted behavior

- Ordinary repayment minimum is0.1icUSD (10000000e8s). The new repayment rule forgives a strictly positive residual below0.1 only after a successful authorized repayment. Exactly0.1 remains debt. No percentage-based threshold and no increase above the authorized debit.
- Clearing debt does not automatically withdraw collateral or close a vault. Preserve explicit repay-and-close intent, claim/native-XRP/collateral semantics and existing authorization/unknown-outcome safeguards.
- Check all three ordinary repayment paths, explicit close, min-vault-debt configuration, interest accrued across awaits, stable-token decimals/fee rounding, frontend Max/projection/close detection and withdrawal dust clearing. Resolve coupled conflicts without stranding debt.
- Failed, rejected or unknown repayment transfers cannot trigger the new forgiveness rule. Successful debit, debt reduction, forgiven principal/interest, supply/reserve effect and replay must be coherent and exactly-once. Additive event/state compatibility and Candid/declarations discipline apply if touched.

Scope clarification accepted by Merritt: preserve the existing <=0.0005 icUSD withdrawal/close exceptions verbatim, including the native-XRP zero-debt guard. Removing those exceptions could strand historical tiny balances. They do not become a new unpaid <0.1 exception. Their pre-existing withdrawal/replay asymmetry is separate backlog; SEC17 does not claim every forgiveness path requires a successful payment.

Historicalc37639e1 introduced near-full upward debit to tolerate interest between UI read and execution; this rationale does not authorize exceeding today's approved amount. Current withdrawal dust and replay behavior require source verification. Prior memory is only an investigation lead, not proof about historical customer outcomes.

## Delegation and resource boundaries

Reuse isolated checkout on branchcodex/repayment-residual-forgiveness-20261112 at240. Backend content equals released corrective312ebe03 (gitdiff backend empty). Rowan owns backend Rust/event/state/test edits; Iris is read-only and independent, Ellis owns integration/frontend/board/release packet. No overlapping writers, production actions, private customer-data export or broad audit.

Requested economical Sonnet probe returned actualSonnet5.5 but plan-only and unable to establish the included-subscription envelope, so no source was sent for implementation. Backend fallback is Terra, justified by money-bearing Rust async settlement, replay and accounting integration beyond a small mechanical task. Sol is used only for bounded independent review. No shell/network/test/build delegation to an external provider.

Use demo-first: demonstrate one synthetic repayment crossing the residual boundary with exact debit and unchanged collateral, then essential focused checks for actual accepted risks: failure/unknown, boundary0.1, replay/idempotency, across-await interest and stable rounding. Reuse test stack; no new dependency/harness framework, broad audit, repeated full-suite/CI campaign or speculative gates. Heavy compile/test execution is coordinated with release/shared resources before running; workers initially edit/read only. Essential test source can be prepared now.

## Release boundary

Prepare a readable pushed implementation/test/review receipt and exact build/install proposal through Dorian. Existing137 frontend approval covers only240assets; it does not authorize a new backend module, migration, live repayment, config change or funds movement. Any genuinely needed new release decision is pushed once and centrally coordinated. Source, tests, build, merge, install/readback and financial acceptance remain distinct.

## Candidate checkpoint

Implementation source: `9662fbe73c0356ddaecd180a8d4f03229e383bd8`, branch `codex/repayment-residual-forgiveness-20261112`. Rowan completed backend edits and six focused test sources; Iris found no confirmed accounting defect in the bounded changed-source review. Ellis coupled frontend Max, fixed repayment minimum and settlement projections without enabling automatic close for wallet-limited payments. Tests and generated-interface proof are pending a fresh coordinated local lane; source review is not test or deployment proof.


## Focused verification rubric

- Three payment routes cap to the authorized amount and current debt. Fixed ordinary floor is 0.1 icUSD; exactly 0.1 residual remains payable even if collateral borrowing minimum is higher.
- At successful callback, accrue through current settlement time before deciding strict residual forgiveness. Record paid and forgiven amounts separately; only paid interest funds treasury/pool distributions. Forgiveness reduces recorded debt without claiming a ledger burn, stable reserve receipt or revenue for that amount.
- Replay new optional settlement checkpoints exactly (debt, interest, accrual clock, forgiveness counter), retain old omitted-field CBOR/replay behavior, preserve collateral and explicit close/claim/native-XRP rules.
- Source-check all three transfer-result branches: failed/rejected/unknown results return errors without invoking settlement. Synthetic tests do not prove a real ledger transport-uncertainty lifecycle. A distinct authorized retry is not claimed to be the same ingress; no new journal/idempotency guarantee.
- Frontend Max reserves one approval fee and never increases an entered debit. Wallet-limited near-full payment remains ordinary repayment; previews distinguish residual/forgiveness and warn that settlement-time interest can change estimates. Stable-token precision must be reflected in estimates.
- Essential deterministic scope is six backend tests, the existing stable-rounding tests, canonical generated Candid/declarations and old-service compatibility, and the three focused repayment frontend suites. Source review, test execution, production build, merge, deployment and actual customer outcome are separate proof grades.

Fresh Cody allocation `02436e59-f97a-46d1-9cae-9a8d0a8db253` pins test input `3a4ff44f229d02481629a5455a4d886d08e88534`. One sequential offline local batch started; no paid CI, PocketIC, installation or financial action. The resource coordinator owns independent closure/release confirmation. Test outcomes will be appended only after reading native exits and counts.


## Executed checkpoint and review corrections

Backend content at `38b3983a29fecf7084899dadc389a8da933f3398` is unchanged from the six-test passing candidate. Native result: six passed, zero failed, zero ignored. The runner then failed in its own regular-expression result parser (`Match` slicing); it stopped before subsequent commands, with no owned processes or tracked changes remaining. Parser corrected for a fresh continuation; no backend retest is needed absent further backend changes.

Iris identified stable-token authorization truncation could change the exact0.1 preview boundary, and the API client still used the configurable borrowing minimum for repayment while also blocking explicit subminimum repay-and-close. Candidate38b3983a corrects all three coupled frontend points and adds four-suite focused coverage, including real API-method dispatch with mocked actors and higher borrowing minimum. Those new checks are not yet executed. No production build, merge, deployment or customer transaction is implied.


## Final source/preparation outcome

Implementation00cf550d, receipt2c217622: six backend plus four stable tests passed; twenty frontend tests/four suites passed; canonical generated interfaces and source/prior service compatibility passed. Both independent reviewers returned PASS, and both consumed local allocations were independently released. Production build/merge/install, financial action and reserve enablement remain unexecuted. Dorian owns the one concrete release proposal, including the repeatable per-settlement economic writeoff implied by the requested policy.
