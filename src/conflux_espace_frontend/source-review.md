# Conflux UI adversarial verification

2026-09-13. Two independent first-party Claude Sonnet 5 reviewers per round. Coordinator adjudicated findings. Actual model verified in CLI `modelUsage`, not inferred from the requested alias. Initial component work used the authorized GPT-5.6 Luna fallback while Claude authentication was unavailable; the user's subsequent login restored Sonnet review access.

## Shared rubric

Accepted design and brand fidelity; exact CFX/icUSD amounts from input to signed intent; truthful stale/error/fee/APR presentation; read-only public data; collapsed USD-based collateral grouping; preserved origin, canary envelope, EIP-712, nonce, durable locks and receipt behavior; navigation; public artifact separation. Review scope was source/UI delivery, not activation or successful live minting. Both reviewers received the same scope and deterministic evidence and were not given the other's report within their round.

## Round one

- A: PASS, session `8bdc1354-6b0b-415e-a5a9-35a27c1e751a`. Confirmed exact input/signing parity, unchanged lifecycle guards, current no-mint-deduction behavior, real APR consumption, and public artifact pruning. Non-blocking unknown-config receive copy was subsequently improved.
- B: FAIL on one asset-release concern, session `753d4264-da68-4e8a-b3db-5de99122bf13`. The copied Circular TTF embeds terms restricting public hosting; no separate webfont grant was verified. Coordinator treats this as an unresolved license provenance issue, not a legal determination about Rumi's existing site. The newly copied file was moved out of this frontend to `/private/tmp/conflux-font-hold.hXuEKJ/CircularStd-Medium.ttf`, preserving recoverability. Original homepage asset untouched. All new frontend typography now uses licensed Inter.
- Both found no blocking runtime/signing issues. Current backend chain-mint source queues full requested debt; configured `borrowing_fee_bps` is dormant, so the illustrative mock's deduction is not imposed by the UI. Exact configuration parity supports the displayed 2.00% APR; future fee wiring must update this UI contract.

## Fresh round two

- A: PASS, session `4584c29b-c517-4a84-ab1a-799a6fdf8687`. Reproduced typecheck, 57 tests and public artifact policy; confirmed Inter-only source/public assets and no remaining source blocker.
- B: PASS, session `0c89f9da-10ac-475d-8dc4-1fff0d8c872a`. Confirmed font/license/provenance resolution, amount handling, tablet breakpoint, accessible compact wallet button and preserved guards.
- Both noted a stale ignored `dist/` from an earlier build. Coordinator rebuilt the canary output after font removal and verified its fonts directory now contains only Inter and its license. Public verification always uses a fresh ephemeral directory. Reviewer attempts outside read-only scope were denied; no reviewer deletion was executed.

## Result and limits

Source review passes after two rounds. Browser/layout checks and exact skips are recorded in `design-qa.md`. No signature, transfer, deployment, DNS change or activation is proven by these reviews. No claim that every connected-wallet control was exercised. Production borrowing remains disabled in the observed live read-only status.

The public bundle retains a non-blocking size warning. A future dedicated fee/rate API would reduce frontend coupling, but is optional follow-up, not an invented release gate for this design.
