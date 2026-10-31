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

PR #601 merged as `5992bdaf` after CI run `41224861787` passed and both final source reviewers passed. The scoped backend upgrade and frontend publication are deployed and independently read back. Private state/receipt captures stay outside Git. No live paid canary or snapshot rollback is part of this release. Asset synchronization must read every inventory page, preserve old assets, update mutable identity/gzip encodings atomically, and add only the explicit PublicNode connect-src destination to existing HTML CSP headers.

Artifact prepared from reviewed source `ce679ee107e2ab44f308c8e1f5039aaeeaf7031e`: raw SHA256 `d4b220377539019b378b7a4d616222d912801dd4c83deb0cc05e09f2b84fc0c7`, upload gzip SHA256 `b8f9687ffd55e02eec2d6893e4c6cbd1e1d751b10ece4ccf19173e14263d27ad`; executable code 8464081 bytes, leaving 3856687 bytes beyond the 262,144-byte project reserve. Independent final reviewers both passed after the first-round fixes.


## Deployed readback

- Backend is Running with the pinned gzip hash above. Source/live Candid and all 390 Wasm export names are unchanged. Controllers, allocation/freeze settings, mode, emergency state, mint cap, fixed fees, sponsorship-off setting and retained ckSOL receipts were preserved. Calculated risk thresholds changed slightly with live collateral weights; they are not fixed configuration fields.
- The oldest snapshot slot was archived locally with payload length checks and recorded hashes before replacement. The management call returned fresh snapshot `00000000000000150000000001f088fe0101`; the original CLI response is the authority for this ID.
- Frontend immutable staging used batches 629–634; final atomic entry/version/gzip/CSP publication used batch 635. All 1,116 previous keys remain, total 1,416; 300 staged contents and all 307 changed encodings were checked against the build. Existing properties were preserved except the explicit PublicNode HTML connect-src addition.
- Served index SHA256 is `7717232ca79ac818037afa0a87c822a70d3027ffab47958d759673034e0b8154`; served ckSOL node bytes also match the build. Frontend remained Running with Wasm `04e565b3425fe7510ee16b02adcfe3f01abc9a2725c82a21cb08969241debd62` and existing controllers.
- The live Chrome page restored the connected account, displayed its existing completed mint and settled/refunded payment receipt, and omitted both acknowledgement and the obsolete approval-only message. Only read-only receipt expansion/collapse was exercised; no paid action was clicked. User financial identifiers, amounts and screenshots remain outside Git.
- No new real-wallet confirmation/dispatch roundtrip was run through the newly combined action. The existing real receipt proves its previous transaction; fixtures prove the revised orchestration. Background checks can briefly disable controls while reads complete. Existing unrelated missing-token-canister wallet-balance errors and a Phantom extension injection error remain outside this change.
