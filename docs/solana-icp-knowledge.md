# Solana ↔ ICP working notes

Status: maintained research and code index. Last reviewed: 2026-10-27.

This file is for agents working in Rumi or another repository. Add a dated,
source-linked note when a Solana/ICP integration teaches something reusable;
correct stale notes in place. Keep protocol choices, observed failures, and
current official behavior distinct. Check the owning code and live network
again before using a historical observation for a release.

## Product paths are different

| Path | Asset held by Rumi | Where icUSD circulates | Rumi status at review |
| --- | --- | --- | --- |
| ckSOL collateral | ICRC ckSOL on ICP | ICP | The chosen SOL collateral direction; source is in `src/rumi_protocol_backend/src/cksol.rs`, `cksol_paid.rs`, and the registration endpoint in `main.rs`. Confirm live registration, minter terms, and release state separately. |
| Native SOL collateral | SOL at threshold-derived Solana addresses | ICP | Retired after ckSOL became available. PR #322 was merged, then reverted by PR #368. Do not revive this rail by default. |
| Solana-native icUSD | Potentially any supported collateral | SPL icUSD on Solana | A separate future objective. The committed `chains/solana` M1/M2 code is a devnet-oriented foundation, not a production mint/burn service. |

The user retired **native SOL collateral**, not the idea of issuing icUSD on
Solana. A direct Solana collateral rail would add its own custody, gas, observer,
liquidation, and recovery system. ckSOL lets Rumi use its existing ICRC
collateral path. That is a product decision, not evidence that the direct rail
was defective or that the ckSOL flow has completed live verification.

## Code and evidence worth keeping

- **Future Solana-native icUSD:** `src/rumi_protocol_backend/src/chains/solana/` is committed on main. Its `tx.rs` hand-encodes SPL `MintTo`, idempotent associated token account creation, and durable nonce transactions; `adapter.rs` signs mint transactions with threshold Ed25519; `settlement.rs` and `deposit_watch.rs` provide submission, confirmation, and deposit scaffolding. See `tests/solana_m1_seam_pic.rs`, `solana_m2_sign_pic.rs`, `solana_bootstrap_pic.rs`, and `solana_m2_pic.rs`. The observer currently logs supply drops; burn proof application is explicitly deferred to M3 (`deposit_watch.rs`). Config defaults to devnet and `test_key_1` (`config.rs`). None of these source files alone establishes mainnet readiness.
- **Retired direct SOL collateral:** the full implementation, tests, and design are already recoverable in Git history at merge `bb1f7670` (tip `879fc717`), including `chains/sol/`, `tests/sol_native_e2e_pic.rs`, and `docs/superpowers/specs/2026-07-24-native-sol-collateral-design.md`. Revert `c029d666` removed it from the active tree. Reuse ideas or pure tests selectively; do not cherry-pick the rail wholesale into current state.
- **Historical integration playbook:** [`icp-solana-integration-playbook.md`](icp-solana-integration-playbook.md) records difficult field lessons from a separate Motoko/React integration. It is valuable as a seam checklist, but its deployment runbook, package assumptions, and provider observations are historical and are not Rumi instructions.
- **Initial Rumi plans:** `docs/superpowers/specs/2026-06-01-solana-integration-design.md` and the two `docs/superpowers/plans/2026-06-01-solana-*.md` files explain the M1/M2 design. Treat their unchecked tasks and proposed M3 behavior as plans, not completion proof.

These committed Git objects preserve the difficult work. No new copy of the
retired rail is needed; this index makes the useful parts discoverable.

## Lessons to reuse

1. **Keep key role and chain domain in every derivation path.** The retired rail used `[501_le, "collateral", owner, vault_id_le]` to avoid colliding with the older Solana chain-vault path `[501_le, owner, vault_id_le]`, whose vault IDs came from another counter. See the retired design §1.1 and `chains/sol/ted25519.rs` at `879fc717`. Review every future signing role for cross-product path collisions.
2. **Bind network selection to signing configuration.** The retired rail coupled its RPC cluster to the configured Schnorr key name. The existing M1/M2 `chains/solana` code is devnet/test-key oriented. A future Solana icUSD release needs an explicit, verified cluster/key/mint-authority relationship; merely changing an RPC URL is insufficient.
3. **Treat broadcast ambiguity as durable state.** `sendTransaction` may have reached Solana even if the ICP reply is rejected, inconsistent, or fails to decode. Rumi's M2 settlement persists the deterministic signature before broadcast and reconciles that attempt before signing a new transaction; inspect `settlement.rs` before extending the flow. Retaining canonical signed bytes is a design option for future changes, not a claim about M2's current storage. The older playbook §1 documents the reply seam. A successful build does not prove retry safety.
4. **Use finalized chain evidence for credits and releases.** Rumi's M2 observer polls finalized balances for each `AwaitingDeposit` vault and makes the state transition idempotent; it does not use a per-address signature cursor. Other integrations in the older playbook use signature cursors and explicit unmatched-deposit handling. Choose the observer model deliberately and test its duplicate, partial-deposit, and recovery behavior. A mint-supply decrease is only a signal; it does not identify the burner or vault debt. Rumi's current M2 supply gate logs that signal and leaves burn proof verification for later work.
5. **Separate browser and canister roles.** Wallets sign user transactions; a browser RPC provider supplies reads/blockhashes; a canister calls SOL RPC and signs its own transactions. The older playbook records CSP, provider-domain, SIWS, and Phantom/Ledger pitfalls. Reverify each against the current wallet and provider versions before implementation.
6. **Test serialization and identity at the real boundary.** Rumi's Solana tests exercise PocketIC threshold signatures, signed wire bytes, bootstrap errors, and settlement outcomes. Keep byte-exact transaction vectors and Candid declarations alongside any future changes, especially for SPL mint authority and nonce handling.

## Before Solana-native icUSD work resumes

Start with the current `chains/solana` source and tests, then write a new
acceptance plan for mint authority, burn-to-debt attribution, replay protection,
cross-chain supply accounting, nonce/reply-loss recovery, authorized callers,
and mainnet configuration. Check current ICP [Solana integration](https://docs.internetcomputer.org/guides/chain-fusion/solana)
and [chain-key tokens](https://docs.internetcomputer.org/guides/digital-assets/chain-key-tokens)
guidance, plus the repo's ICP skills, before implementation. Verify actual
canister state and deployed artifacts independently of the source tree.

## How to maintain this file

Add a short dated entry with: the observed behavior, environment and version,
evidence link or file/commit, what changed, and whether it remains verified.
Prefer updating a stale claim over appending a contradictory one. Never put
secrets, wallet seed material, or unreviewed live action instructions here.

## 2026-10-30 — ckSOL OISY channel ownership

Rumi bundles an older PNP signer and a direct `@icp-sdk/signer` 5.3.1 signer. Both default postMessage transports name the popup from the OISY origin, and closing either channel closes that physical window. Independent SDK timers can therefore interfere even after the ckSOL click opened its own channel. The direct SDK also retains its establishing promise after an opening failure; reusing that Signer can yield out of the next click before attempting another opening.

The ckSOL source isolates each paid action in a fresh Signer and uniquely named popup, opened synchronously before awaiting any network call or Web Lock. Its own auto-close is disabled only for that action and the page closes the channel in `finally`. Default SDK behavior elsewhere and existing vault-recovery records remain unchanged. See `src/vault_frontend/src/lib/services/cksolOisySigner.ts`, its `.spec.ts`, and the `/solana` page handlers. Installed-SDK reproductions and 81 focused ckSOL checks support this source behavior; real wallet consent and mint/refund receipts remain separate proof.

For assets-only releases, read the live asset interface. This frontend's `list` accepts optional `start`/`length` and defaults to 100 records. An older uploader using an empty record reads only the first page and can trap on duplicate creates. Full pagination and updating every existing encoding of mutable entry files (especially `/index.html` gzip) are necessary for a correct content update; an independent `/index.html.gz` asset key is not that gzip encoding. The October 30 release receipt is `docs/operations/cksol-processing-ui-release-20261030.md`.

### 2026-10-30 — observed funded mint and refund

One OISY-funded mainnet deposit completed through the deployed isolated-signer flow at source `e153c1cfc7f7d623b3b65672bfedef09d25b6038`. The official minter progressed from submitted/finalized to Minted; its amount and ledger block matched a positive readback of the owner's ckSOL balance. Cycles Ledger funding and refund blocks were read separately, and the refund recipient matched the payer. The private transaction receipt is retained with this chat, outside Git; user financial identifiers and amounts are intentionally omitted here.

This is one observed paid mint/refund lifecycle, not evidence for SOL redemption or every wallet/edge case. The payment card still retains an obsolete approval-only message after its receipt becomes Complete; the minter status and ledger receipt are the completion evidence. Treat correction of that stale message as a UI follow-up, not a reason to repeat a settled payment.

## 2026-10-31 — guided deposit reconciliation

The guided-mint change retires a retained terminal relay record inside the next authenticated paid request, before funding, under the owner guard. A settled cycles receipt does not establish mint finality: the backend reads the official minter and rechecks the captured record after awaiting it. Unknown relay/payment/refund outcomes remain held. The frontend independently gates new requests on the tracked minter status, invalidates pre-sweep balances when adopting a new deposit, and derives completion copy from authoritative state rather than an old intent message.

Background receipt recovery must share the paid-request Web Lock with signing, and recheck wallet generation and the original journal before writes. A delayed old receipt must not replace a newer uncertain request from another tab. The isolated signer already keeps its channel open, so the same user action can guide sequential approval and dispatch without a second page click. PublicNode balance reads are advisory only; the minter independently verifies SOL. App-origin testing showed Solana's default shared RPC returned HTTP 403, whereas the documented PublicNode endpoint accepted the same read.

## 2026-10-31 — shared minter recovery invariants

The BTC, DOGE and ckERC20 retail flows now use the isolated action helper in `src/vault_frontend/src/lib/services/minterOisySigner.ts`; ckSOL keeps its compatible import alias. Popup cleanup is idempotent across page/service ownership. Open the scoped channel synchronously in the click before any Web Lock or network read. A cleanup rejection must never replace an accepted financial receipt.

Automatic progress can retire only the exact stored journal under the submission lock. BTC/DOGE `WillReimburse`, `Unknown` and `AmountTooLow` remain held; only confirmed payout or completed reimbursement retires the request. For ckERC20, a ledger `TemporarilyUnavailable` can represent an uncertain inter-canister burn reply; preserve the predispatch intent even without a returned burn index. Damaged local journals also remain held. Ethereum receipt success or an ICP balance delta alone is not mint proof: join the minter's exact accepted deposit event to its minted event. A reverted Ethereum receipt releases its lock only after finalized height and canonical block-hash verification. See the retail minter release record and mounted recovery regressions.
