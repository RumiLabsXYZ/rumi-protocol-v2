# SEC17 repayment residual correction — accepted source/preparation scope

Owner: Ellis, with Rowan backend implementation and Iris independent accounting review. Rob's direct CTO instruction authorizes fixing and double-checking repayment logic, not deploying a new backend. The existing live frontend240/version1794522798107, backend730f5471 and prospective Season2 remain unchanged.

## Accepted behavior

- Ordinary repayment minimum is0.1icUSD (10000000e8s). Only after a successful authorized repayment may a strictly positive residual below0.1 be forgiven. Exactly0.1 remains debt. No percentage-based threshold and no increase above the authorized debit.
- Clearing debt does not automatically withdraw collateral or close a vault. Preserve explicit repay-and-close intent, claim/native-XRP/collateral semantics and existing authorization/unknown-outcome safeguards.
- Check all three ordinary repayment paths, explicit close, min-vault-debt configuration, interest accrued across awaits, stable-token decimals/fee rounding, frontend Max/projection/close detection and withdrawal dust clearing. Resolve coupled conflicts without stranding debt.
- Failed, rejected or unknown transfers cannot trigger forgiveness. Successful debit, debt reduction, forgiven principal/interest, supply/reserve effect and replay must be coherent and exactly-once. Additive event/state compatibility and Candid/declarations discipline apply if touched.

Historicalc37639e1 introduced near-full upward debit to tolerate interest between UI read and execution; this rationale does not authorize exceeding today's approved amount. Current withdrawal dust and replay behavior require source verification. Prior memory is only an investigation lead, not proof about historical customer outcomes.

## Delegation and resource boundaries

Reuse isolated checkout on branchcodex/repayment-residual-forgiveness-20261112 at240. Backend content equals released corrective312ebe03 (gitdiff backend empty). Rowan owns backend Rust/event/state/test edits; Iris is read-only and independent, Ellis owns integration/frontend/board/release packet. No overlapping writers, production actions, private customer-data export or broad audit.

Requested economical Sonnet probe returned actualSonnet5.5 but plan-only and unable to establish the included-subscription envelope, so no source was sent for implementation. Backend fallback is Terra, justified by money-bearing Rust async settlement, replay and accounting integration beyond a small mechanical task. Sol is used only for bounded independent review. No shell/network/test/build delegation to an external provider.

Use demo-first: demonstrate one synthetic repayment crossing the residual boundary with exact debit and unchanged collateral, then essential focused checks for actual accepted risks: failure/unknown, boundary0.1, replay/idempotency, across-await interest and stable rounding. Reuse test stack; no new dependency/harness framework, broad audit, repeated full-suite/CI campaign or speculative gates. Heavy compile/test execution is coordinated with release/shared resources before running; workers initially edit/read only. Essential test source can be prepared now.

## Release boundary

Prepare a readable pushed implementation/test/review receipt and exact build/install proposal through Dorian. Existing137 frontend approval covers only240assets; it does not authorize a new backend module, migration, live repayment, config change or funds movement. Any genuinely needed new release decision is pushed once and centrally coordinated. Source, tests, build, merge, install/readback and financial acceptance remain distinct.
