# ckSOL ICP processing payment implementation

User-authorized scope: clearer cost checks before sending SOL, a way to finish existing SOL deposits without sourcing cycles externally, and separate XRP deposit-tag guidance. On November 1 Rob selected the ICP escrow model and a retained 5% service fee. The interim proposal to convert the user's entire temporary budget to cycles is superseded; it was not implemented or deployed.

## Accepted payment behavior

The user's default ICP account approves a maximum allowance. Rumi confirms an ICP transfer_from into an isolated operation subaccount, fronts 1T cycles from a finite dedicated allocation, measures the official minter's attached-minus-returned cycles, then automatically returns unused ICP at the frozen accepted quote rate. The retained service fee is 5% of that measured net minter charge. The 1B-cycle operating fee and ICP approval, collection and refund transaction fees are disclosed separately. The net minter charge is not a measurement of internal computation alone.

Worst-case charge cycles = 1T + ceil(1T * 5%) + 1B. Maximum deposit e8s = ceil(worst-case charge cycles / xdr_permyriad_per_icp) + 2 ICP ledger fees. The positive refund reserve permits one refund even when the full minter attachment is charged. The upfront wallet debit also includes approval and collection fees. Settlement cannot exceed this approved deposit; the quote rate never changes after acceptance.

Persist collection, dispatch and refund lineage before effects. Uncertain funding never collects again; uncertain minter dispatch never submits again; a refund retry uses the same timestamp and memo. BadFee and aged refunds stay held for reconciliation rather than silently increasing the customer's fee. Refundable ICP stays in the isolated account and is distinct from earned ICP. Periodic revenue conversion or extraction is deferred, with no live transaction authority.

Existing cycles-paid receipts remain valid and their credits/backing are protected against the new allocation. Cross-mode holds apply to the same owner. A settled processing payment does not establish a completed ckSOL mint. The new reserve defaults to zero; source delivery authorizes neither its enablement nor a deployment/funding action.

## UI and existing deposits

Use ICP by default; keep an explicit existing-cycles option for previous users and recovery. Show maximum deposit, total wallet debit, balance, fee schedule and fixed quote rate before SOL send instructions. A user who already sent SOL sees the recorded address/balance and can finish the same mint. Unknown or quarantined results retain their receipts and block replacement spending. Redeem SOL is explained as redemption of minted ckSOL; no unsupported unprocessed-SOL refund is invented.

XRP is independent on branch codex/xrp-deposit-tag-guidance. Incoming per-vault addresses need no destination tag; exchange users should select No tag or contact exchange support if its form insists. Outgoing numeric destination tags remain supported. No client identifiers are committed.

## Execution and checks

- Backend worker owns additive escrow state/API/consent and generated Candid. Focused accounting/state checks plus source-generated Candid compatibility; integration review must inspect actual asynchronous ledger/minter boundaries.
- Integrator owns ICP payment service, mounted page controls, owner/session journals and recovery; focused tests/build/auth assets and browser fixtures.
- XRP worker owns its separate source branch and focused UI regression/build.
- Lookout is authoritative for task status: separate ckSOL and XRP subprojects, with plain-English and technical status. Preserve preexisting cards/priorities.
- No new real-wallet financial canary, production installation, reserve enablement or client-fund recovery occurs without separate authorization. Record actual omissions in DEMO_DEFERRED_WORK and the existing feature lane.

Baseline: 66 untouched ckSOL service/page tests passed. Worktree inventory and disk-pressure helper removed zero eligible worktrees. Model choice: native Terra for backend durable financial/state integration; native Luna for bounded XRP copy and board metadata; coordinator integrates and reviews. No new recurring process or monitor.
