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
