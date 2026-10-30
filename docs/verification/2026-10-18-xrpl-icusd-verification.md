# XRPL icUSD lane: verification report

- Code verified: `621dec582c9101ab2b69fb5b55c1da6f58c3622a` on `codex/xrpl-icusd`, base `2c071994`. This report and the launch checklist are documentation-only commits added after it.
- All results are local: unit, host, PocketIC and jsdom tests. Nothing was deployed, signed against a live network, broadcast, or funded.

## Release artifact

Production steps (from `icp.yaml`):
1. `cargo build --release --target wasm32-unknown-unknown --package rumi_protocol_backend`
2. `ic-wasm` candid metadata
3. `ic-wasm shrink --keep-name-section`
4. `gzip -9 -n`
5. `scripts/verify-xrpl-icusd-artifact.sh --require-mainnet` (exit 0; the marker reads production)

| Item | Value |
| --- | --- |
| Raw wasm sha256 | `f0fb0a364ee6d656fec598373d57b39270e1cb4e38284a74eb4c1ff9e8b45a02` |
| Installable gzip sha256 | `b45e38a259c7f9be513d7ac6b65f171babb8351ea3bfc4ba154e8c9dc06689af` |
| Code section after shrink | 11,616,810 bytes (IC limit 12,582,912) |
| main `2c071994` code section, same steps | 12,153,827 bytes |

The size reduction comes from `[profile.release.package.rumi_protocol_backend] opt-level = "s"`. Workspace LTO was measured larger (14,203,111 bytes) and is not used.

## Test results at `621dec58`

| Suite | Command | Result |
| --- | --- | --- |
| Backend library | `cargo test -p rumi_protocol_backend --lib` | 1436 passed, 0 failed, 1 ignored (the ignore predates this branch) |
| Backend bin, including Candid compatibility | `cargo test -p rumi_protocol_backend --bin rumi_protocol_backend` | 85 passed |
| Testnet fixture build | `cargo test -p rumi_protocol_backend --lib --features xrpl-testnet-fixture xrp` | 302 passed |
| Artifact gate self-test | `scripts/verify-xrpl-icusd-artifact.sh --self-test` | 7 cases OK |
| XRPL icUSD PocketIC end-to-end | `cargo test -p rumi_protocol_backend --test xrpl_icusd_e2e_pic` | 17 passed (458 s) |
| Native XRP PocketIC end-to-end | `cargo test -p rumi_protocol_backend --test xrp_native_e2e_pic` | 4 passed, zero `[gated]` skips |
| Frontend | `npm run test --workspace=vault_frontend` | 90 files, 1205 tests passed |
| Frontend build | `npm run build --workspace=vault_frontend` | passed, including the auth-asset postbuild check |
| Whitespace | `git diff --check 2c071994..HEAD` | clean |

### How the PocketIC suites were run

The repo's PocketIC server is 7.0.0, which caps the code section at 10 MiB. Both suites therefore ran against a size-optimised test build of the same source:
- build flags: `--config 'profile.release.package.rumi_protocol_backend.opt-level="z"' --config 'profile.release.lto="thin"' --config 'profile.release.codegen-units=4'`
- resulting code section: 10,032,116 bytes
- environment: `POCKET_IC_BIN=<repo>/pocket-ic` and `RUMI_E2E_BACKEND_WASM=<that build>`

The suites prove behaviour, not the release artifact's codegen. Plain main also no longer fits PocketIC 7.

## PocketIC scenarios (real ICRC ledger and threshold Ed25519; rippled mocked and signature-checked)

1. Issuer setup reaches Ready with signed, verified AccountSets.
2. Full lifecycle:
   - XRP deposit and vault;
   - borrow 100 (fee 1): escrow 99, debt 100, treasury 1;
   - signed issuer Payment decoded and its signature verified, then delivered;
   - repay 40, then import 1 canonical icUSD;
   - repay 60: debt 0, escrow 0, no orphan backing.
3. Borrow fee routed to an existing deficit before the treasury.
4. A stale full-close quote is held without a burn, and a top-up child then closes the vault.
5. Manual liquidation with XRPL supply outstanding; escrow backing intact, and a late return becomes a payer claim.
6. Stability Pool absorption of an XRP vault with XRPL supply outstanding.
7. Reserve redemption with XRPL supply outstanding.
8. Escrow mint reply lost: the same nonce reconciles, with no second mint.
9. Escrow mint TooOld after a loss: proven from a block scan.
10. Exclusion blocks legacy borrow, withdraw and liquidation during an unresolved mint, and survives an upgrade.
11. An upgrade after the issuer tx is signed recovers the blob and delivers exactly once.
12. Partial repay with an upgrade mid-burn: accrual stays frozen and interest is distributed once.
13. Full close with an upgrade mid-burn stays at zero after settlement.
14. A history gap holds the cursor and the debt, then recovers.
15. A sub-e8s return keeps the exact fractional liability, which the owner can reissue.
16. An excess return becomes a backed, owned claim, payable as canonical icUSD.
17. An unattributed return is refunded to the verified sender and never credited.

## Independent reviews

All reviews were Sonnet, independent of the implementers.

- Accounting and custody: no critical findings. Two medium findings and six low or operational ones, all fixed in `84f6cb4f`.
- End-user and recovery: 11 findings (3 high), all fixed in `a0f52012`.
- State, Candid and migration: no blocker. The medium and low items are fixed in `84f6cb4f` or documented in the launch checklist (one-way-door rollback, frontend redeploy before activation).
- Re-review of the fix commits: no double-credit, nonce-reuse, double-mint or double-refund path. One medium and four low findings, all fixed in `621dec58`.

## Not proven here

- **R10, public XRPL testnet run.** Pending authorization for Xaman test app credentials, testnet faucet funding, and signing and broadcasting on testnet. The fixture artifact and runner are ready (`scripts/run-xrpl-icusd-testnet.sh`).
- **Real browser run.** No real-browser run against a built app with a real Xaman app. The covered equivalent is jsdom component tests rendering the real panel with mocked adapter, actor and RPC.
- **Real dedup expiry.** The 24 h ledger dedup expiry is simulated with a TooOld-after-loss ledger fixture.
- **Mid-call upgrade.** A true upgrade in the middle of a call is impossible in PocketIC. It is approximated with trap-after-commit, a stopped ledger, or a held outcall.
- **Partial liquidation with a stable token.** `liquidate_vault_partial_with_stable` with XRPL supply outstanding is not covered end to end.
- **Instruction cost.** The cycle and instruction cost of the `opt-level = "s"` backend versus the default is not measured on staging.
- **Observer cost.** The observer's real cycle burn is not measured.

## After merging main `ed99a4d7` (ckSOL collateral)

The Candid conflict was resolved by regenerating the `.did` from the merged Rust source, then regenerating the declarations. Every method on main is present; only generated `Result_N` names were renumbered.

Re-checked on the merged tree:

| Check | Result |
| --- | --- |
| Backend library | 1446 passed, 0 failed, 1 ignored |
| Bin, including Candid compatibility | 85 passed |
| Frontend | 1242 tests passed; build passes |
| Release code section after shrink | 11,712,624 bytes (limit 12,582,912) |

The PocketIC suites were not rerun after the merge. The merge touched only Candid and declaration files on this branch's side.

## After merging main `32358d23` (ckSOL user-funded deposits)

The Candid conflict was resolved the same way: regenerate the `.did` from the merged Rust source, then regenerate the declarations. Every main method is present.

| Check | Result |
| --- | --- |
| Backend library | 1465 passed |
| Bin, including Candid compatibility | 85 passed |
| Frontend | 1277 passed |
| Release code section | 11,826,498 bytes (94% of the 12 MiB limit) |

Size headroom is shrinking as main grows. Budget future backend features against it.

## After merging main `a8709fc1` (SP rails restore #590, points outage work #583)

Main independently set the backend to `opt-level = "z"`. This branch now uses main's setting instead of its own `"s"`.

`VaultLiquidationGuard::acquire` gained parameters on both sides, and both were kept:
- every caller respects the XRPL exclusion, except claim settlement (unchanged);
- main's new SP-liquidation retry respects the exclusion too.

| Check | Result |
| --- | --- |
| Backend library | 1495 passed |
| Bin, including Candid compatibility | 85 passed |
| Fixture XRP | 302 passed |
| Frontend | 1298 passed |
| Release code section after shrink | 10,123,330 bytes (80% of the 12 MiB limit) |
| `xrpl_icusd_e2e_pic` | 17 of 17 passed (437 s) |
| `xrp_native_e2e_pic` | 4 of 4 passed, 0 `[gated]` |

Both PocketIC suites ran against the shrunk production artifact itself, which now fits PocketIC 7's 10 MiB cap. They no longer need a separate size-optimised test build.

## After merging main `01615221` (backend `optimize O1` code reserve)

The `icp.yaml` conflict kept both sides: main's new `ic-wasm optimize O1` step, and this branch's final artifact gate. The production marker section (`icp:public rumi:xrpl-environment`) survives `optimize O1`, and the gate passes on the final gzip.

| Check | Result |
| --- | --- |
| Release code section | 9,648,104 bytes (77% of the 12 MiB limit) |
| Backend library | 1495 passed |
| Bin, including Candid compatibility | 85 passed |
| Fixture XRP | 302 passed |
| `xrpl_icusd_e2e_pic` | 17 of 17 passed |
| `xrp_native_e2e_pic` | 4 of 4 passed, 0 `[gated]` |
| Frontend | 1298 passed |

Both PocketIC suites ran against the optimized production artifact.
