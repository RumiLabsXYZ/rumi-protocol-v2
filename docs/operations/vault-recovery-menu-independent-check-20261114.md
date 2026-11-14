# Independent live verification — compact vault recovery menu

**PASS, 2026-11-14.** The authorized frontend-only correction is live at version `1794685142389`, source `dc4b8f52a1e76a36791defb5f66005276d00d4a5` (code `8a64f8188422d1e7692fca67531782fb2f04d241`). Optional discovery is now **Wallet → Recover a vault action**. Known saved, submitted, held-collateral and error recovery branches remain, including explicit-click OISY and owner/session protections. The existing SEC19 collateral-deposit repair remains in this source.

## Source and build

- Red regression reproduced the unwanted generic unknown-state card. Final23 mounted tests/four suites and two independent final reviews passed. Connected source components were visually inspected at1280×900 and390×844: compact menu fits; opening menu makes no status/channel call; explicit recovery click performs one authenticated status read and no financial action; cross-browser submitted request remains recoverable.
- Single fresh frontend/auth build passed native0 in17.894s. Independently hashed all740 frozen and producer files,476 compression pairs, exact code/source equivalence, unchanged auth assets and preserved prior mint/repayment/recovery protections.
- Pushed publication packet `3b7fc171c89b8f03fcb9e626b5000337fd6fac97`, readable decision SHA256 `802441d8aaf2e4be6bcf215af26bec7e6d0ae678397487a47c84401e61bef78d`; final pins SHA256 `59e33b5a6c1ae45d959fbf1c021c60ccfdaee1621f25f93e8bd70edecea56028`; manifest SHA256 `c10d3fae4d1b130150d9e030687e17783a34f61aab1c1a46015e69807c1745ad`. Asset DID normalization was trailing EOF whitespace only, reflected in final pins; source/build unchanged.

## Live publication and independent proof

Dorian staged immutable assets in batch699, then activated only seven approved entry/version encoding updates in batch700. His101 staged,9 final and5 primary-domain/auth readbacks passed; all writes known once, no retry/failure. Final settings and Prepare/Commit permission lists unchanged.

Ellis independently obtained a stable double-read baseline and stable double-read final inventory. Final **5619 keys /5813 encodings** preserve **all5520 prior keys, properties and untouched encodings**, with exactly99 new immutable keys and7 existing encoding updates; no delete or property mutation. All15 primary-domain responses for entry, auth, critical recovery bundles and retained mint/repayment paths match frozen bytes. Final independent byte verification completed `2026-11-14T19:52:56.712Z`.

Certified before/after proofs match for both canisters and ordered controller lists:

- Backend `tfesu-vyaaa-aaaap-qrd7a-cai`, module `8f6f5933e54fa44205760b9a8d1c3869135df43825eadfee70b20c62d0a0bd16`.
- Backend Candid `d8b0142e705e860640557b7ea6253a2a6027d7e96f3b15cfee9ba51fff0d2c97`.
- Frontend `tcfua-yaaaa-aaaap-qrd7q-cai`, unchanged module `04e565b3425fe7510ee16b02adcfe3f01abc9a2725c82a21cb08969241debd62`.

Actual anonymous primary-domain `/vaults` and `/solana` were checked at1280×900 and390×844: correct active version, no horizontal overflow or app error overlay; Mint status retained. The connected menu flow is proven by mounted tests and mocked component render evidence, **not a live wallet signature**. Existing synthetic XRP ledger-logo400 errors and other metadata warnings remain; no clean-console claim.

[Machine evidence](evidence/vault-recovery-menu-independent-20261114) includes artifact/preservation/readback results, certified proofs, full final inventory, baseline digest and live layout observations. Source tests and connected screenshots are in the source decision atdc4b8f52.

No backend lifecycle call, snapshot change, customer allowance, deposit, swap, reserve activation or financial canary was performed. This closes the focused software publication; it does not assert a customer financial outcome.
