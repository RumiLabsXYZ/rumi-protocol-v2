# P12-01 legacy chain burn compensation

**ID:** P12-01-DEF-01
**Feature:** Protocol-funded compensation for legacy ambiguous Stability Pool chain burns
**Owner:** Rumi protocol engineering
**Concern / skipped work:** Archive-backed ICRC-3 receipts are not accepted as proof. The compensation and later original-burn reconciliation paths require a directly served block; ledger-specific cryptographic archive-membership verification is not implemented.
**Reason for deferral:** The bounded repair uses the ledger canister's direct response only and keeps unavailable, incomplete, or archived proof held.
**Demo limitation / containment:** If the exact compensation block has moved to an archive before it can be directly read, the original intent remains pending and continues to block pool operations. No archive callback or missing-history result can complete it.
**Before-client-money verification:** Implement and review ledger-specific authenticated archive proof if archived receipts must support recovery; otherwise retain the hold until an exact direct receipt is available.
**Status:** Open.
**Evidence / disposition:** P12-01 source change; no archived receipt is treated as proof.
