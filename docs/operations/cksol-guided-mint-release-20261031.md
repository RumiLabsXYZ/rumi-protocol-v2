# ckSOL guided mint release — 2026-10-31

Rob explicitly requested implementation and deployment of the guided Send SOL → Mint ckSOL → automatic progress flow. This authorizes the scoped backend upgrade and frontend asset publication; wallet approvals and financial dispatch remain user actions.

## Behavior and verification

- The existing paid endpoint checks the previous payment and official minter before retiring terminal relay metadata. Owner guard, exact funding cap, global sequence, refund accounting and ambiguous-outcome holds remain in place.
- The frontend restores the account-bound address, reads an advisory finalized SOL balance, guides the two wallet consents from one Mint action, polls on a timer/focus, and shows completion without acknowledgement. Technical receipt and funded cancellation remain available in details.
- Background journal recovery uses the same Web Lock as signing and rejects changed session/storage responses. A new deposit invalidates the old address balance and blocks a further paid request until its own minter status is terminal.
- 101 focused frontend tests, 1,337 backend unit tests (one ignored), backend Candid test and frontend production build/auth assets checks passed. Whole-frontend svelte-check reports 30 errors and 66 warnings in 25 unrelated files; affected files have no diagnostics.
- Chrome fixture interaction tested the real page on desktop and 390px width with simulated wallet/funds: returning deposit, one-click submission, queued → automatic minted, uncertain request, RPC unavailable. This does not establish a new real-wallet approval roundtrip.
- Live preflight Candid is byte-identical to the source file. Backend source at the deployed release `63b127b15ece8a79168b802803a4846cdf5dcc31` is identical to the branch baseline; this upgrade adds only the two ckSOL module changes. No persisted state or public interface shape changes.

## Publication

Targets: backend `tfesu-vyaaa-aaaap-qrd7a-cai`; existing frontend `tcfua-yaaaa-aaaap-qrd7q-cai`; network `ic`; existing operator `rumi_identity`. Pre-upgrade backend module is `981d1fd88980cfed322556d5458db1122d8add6c88bef049d7c4f2f54dc13325`. Frontend Wasm and controllers must remain unchanged.

Publication is pending final independent review and artifact verification. Private state/receipt captures stay outside Git. No live paid canary or snapshot rollback is part of this release. Asset synchronization must read every inventory page, preserve old assets, update mutable identity/gzip encodings atomically, and add only the explicit PublicNode connect-src destination to existing HTML CSP headers.
