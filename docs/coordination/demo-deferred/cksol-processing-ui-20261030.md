# ckSOL processing UI — 2026-10-30

- **ID:** DD-CKSOL-20261030-01
- **Feature / owner:** Paid ckSOL deposit UI / Codex.
- **Pending verification:** A real OISY-funded mint and unused-cycles refund roundtrip. Source tests, a successful frontend build, and asset publication do not establish financial settlement.
- **Reason / limitation:** Financial wallet consents remain user actions; no automatic paid canary or repeated real-funds attempt was added. Existing unknown outcomes stay held, except the exact locally rejected pre-send popup error after read-only reconciliation, with the original journal archived.
- **Before client money:** Obtain and reconcile the confirmed cycles approval, backend funding/deposit receipt, minter mint receipt, and any refund receipt; review and record their outcome.
- **Status:** Open; release verification is separate from mint/refund proof.
- **Source evidence:** `src/vault_frontend/src/lib/services/cksolPayment.spec.ts` and `src/vault_frontend/src/lib/utils/cksolMinterPage.fixture.spec.ts`; 79 focused ckSOL checks and the frontend build passed. A focused independent review found the SDK channel-auto-close issue; OISY now uses a separate processing click after confirmed approval, and that targeted fix passed review.

## 2026-10-30 disposition

One mainnet OISY-funded mint and unused-cycles refund is now observed. Independent readbacks of official minter Minted status, recipient ckSOL balance, Cycles Ledger funding, and refund credit to the same payer agree. Exact financial identifiers/amounts remain in this chat's private receipt outside Git. This resolves the missing observed funded-mint/refund proof for that case; it does not assert general client-money readiness or redemption coverage.

**DD-CKSOL-20261030-02 — UI follow-up:** The completed payment card can retain the earlier “processing has not started” approval message. Owner: Codex/frontend. The official mint status and ledger balance confirm settlement; no second payment is needed. Correct message precedence in a scoped UI change; do not reset or replay the completed financial request.


## 2026-10-31 UI disposition

DD-CKSOL-20261030-02 is resolved by merged/deployed PR #601. The guided flow derives messages from current payment/minter state, automatically recognizes terminal deposits, and removes the normal completion acknowledgement and refresh navigation. Source tests, CI, independent reviews and live asset/module/UI readbacks are recorded in `docs/operations/cksol-guided-mint-release-20261031.md`. This records software delivery, not an additional real-wallet mint through the new combined action.

## 2026-11-01 — ICP deposit source delivery

- **CKSOL-ICP-DEF-1; ICP processing deposit; owner: ckSOL coordinator.** No real-wallet ICP allowance/collection, minter dispatch and ICP refund lifecycle, or installed-Wasm upgrade proof was performed for the new escrow path. Source tests, generated Candid compatibility, independent source review and spending-disabled browser fixtures are the delivery evidence. The new reserve defaults to zero. Before offering this path for client money, verify the authorized installed artifacts, preserved existing receipts, approved reserve backing and a bounded operator-owned end-to-end lifecycle, including matching charge/refund/mint receipts. Status: open; source implementation only.
- **CKSOL-ICP-DEF-2; reserve/revenue operations; owner: ckSOL coordinator.** Automatic revenue extraction and periodic ICP-to-cycles replenishment are deferred. Earned ICP stays separate from refundable escrow; no new conversion, burn, reserve funding or enablement has been executed. Before operating for client money, establish and authorize the finite reserve and its replenishment procedure; do not spend refundable deposits. Status: open.
- **CKSOL-ICP-DEF-3; asynchronous lifecycle harness; owner: ckSOL coordinator.** The backend's focused tests exercise accounting, persisted result transitions, exact refund tuples, legacy coexistence and migration defaults, rather than a complete mocked asynchronous ledger/minter lifecycle. Independent review inspected the actual call paths and corrected the concrete liquid-backing defect. A general new harness is deferred for this source/demo milestone; revisit the relevant callback/upgrade seams before client-money use. Status: open.
