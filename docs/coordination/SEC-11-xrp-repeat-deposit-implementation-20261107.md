# SEC-11: existing-vault XRP deposits

Rob reopened this scope on November 7 in the existing Ellis customer case.
Implementation base: live backend source `6847982b44f247e0c99f0a453b583d3fab8107a2`.
This source lane does not deploy, sign a customer transaction, move customer
funds, or send the held customer message. Dorian owns release composition.

## Selected accounting model

Credit a specific validated inbound Payment exactly once, never a difference
between account balance and recorded collateral. Preserve the original custody
derivation (original owner and vault nonce) and the original OpenVault ledger
as a fixed exclusion floor. Transactions at or before that ledger were already
included in the initial balance credit and cannot be credited again. Later
receipts may be presented out of order; permanent hash receipts prevent replay.

Verify the requested transaction hash, network, validated ledger index/hash,
tesSUCCESS, Payment type, exact custody destination, non-self source, and positive
native delivered_amount. Require quorum over consumed fields. Amount/DeliverMax
is not evidence of delivery. The first implementation accepts direct native
payments only, rejecting swaps, paths, SendMax and partial-payment flags.
No receipt or outflow tombstone is evicted to make
a previously consumed payment eligible again.

This model does not need to reconstruct historical gross payouts: a paid claim
disappearing, or a stale pre-payout balance, cannot create an inbound receipt.
Existing claims, reserve and settlement accounting remain intact. It retains
the existing provider-quorum and consensus event trust assumptions, rather than
claiming cryptographic XRPL inclusion verification.

## Legacy origin reconciliation

New opens retain their original owner and opening ledger. Replay reconstructs
the same origin. Old snapshots safely default the new origin map to empty.
For an existing vault, the frontend locates its exact retained OpenVault event
using the existing bounded forward event API. The authenticated backend reads
that exact stable event index and checks the vault, original/current owner,
native collateral, and usable ledger before recording an origin. Missing or
inconsistent evidence is an actionable hold, never a zero-history baseline.

## Interfaces and ownership

- Backend worker `xrpl_final_source_0b91`: owner-authenticated address and receipt
  methods, strict separate deposit RPC proof, additive state/events/replay,
  bounded stable event lookup, focused Rust tests and canonical declarations.
- Miles: frontend service, existing VaultCard Deposit flow, bounded origin lookup,
  copy address, explicit hash verification/retry, focused UI tests and integration.
- Dorian: one serial heavy build/test slot and release composition/publication.
- Ellis: unified customer case, board, exact remedy and held customer message.

APIs: `get_xrp_vault_deposit_address(vault_id, opt opening_event_index)` and
`confirm_xrp_vault_topup(vault_id, tx_hash, opt opening_event_index)`. Both reject
anonymous/non-owner callers before external work and revalidate authorization,
configuration, custody identity and vault locks/current state after awaits.
Credit, normalized hash receipt and replayable event commit atomically. A repeat
of an already consumed receipt returns its recorded amount without new credit.
Receipts have explicit non-evicting capacity; the UI must not invite new funding
when capacity is exhausted. Wallet changes clear the displayed address. Wallet
signer calls occur only from explicit actions, without background popup retries.

## Essential acceptance

Prove a second deposit after a withdrawal is credited once; stale pre-payout
balance creates no credit; old snapshot origin bootstrap and event replay retain
the exclusion floor and hash receipts. Check wrong caller/vault/destination,
self-payment, non-native/failed/unvalidated proof, partial delivery, malformed
hash/index, duplicates and casing, and a vault/config/lock change across awaits.
Use focused lifecycle/auth/serialization tests and one independent scoped review.
Canonical binding generation and enforced release checks remain required.
Do not duplicate unchanged broad suites or require a new history-scan framework.

Completion evidence will identify source, tests, review, publication, and actual
customer acceptance separately. No customer transaction is part of this source
implementation lane.

## Deliberate limitations and release cautions

- The credit and permanent receipt share an ordered private journal entry.
  Public Explorer/AddMargin history and points ingestion do not yet display
  these topups. That integration is deferred; vault collateral still updates.
- Forward old-snapshot decoding and full ordered replay must preserve the new
  state. Downgrading through an older snapshot writer can discard receipt maps;
  do not re-enable topups after such a rollback without explicit preserved-state
  and journal reconciliation. This patch does not authorize a downgrade.
- Local functional checks do not prove published frontend bytes, a successful
  live deposit/withdrawal, or customer acceptance. Those remain separate evidence.

## Local verification checkpoint

Existing Codex worker `xrpl_final_source_0b91` implemented the backend; Miles
implemented the frontend. No new team or dependency was introduced.

- `CARGO_BUILD_JOBS=1 cargo test --offline -p rumi_protocol_backend --lib sec11 -- --test-threads=1`:
  native exit 0, 8 passed, 0 failed/ignored. First compile failed on private
  READ_QUORUM; minimal sibling-module visibility correction passed the same check.
- `RUMI_REGEN_DID=1 CARGO_BUILD_JOBS=1 cargo test --offline -p rumi_protocol_backend --features self_check --bin rumi_protocol_backend check_candid_interface_compatibility -- --exact --test-threads=1`:
  native exit 0, 1 passed; compiles the corrected private-journal self-check seam.
- `bash scripts/regenerate-declarations.sh rumi_protocol_backend`: exit 0.
  Canonical DID copies match. The interface adds only a DTO, two updates and a
  transform query, with canonical result alias renumbering.
- `vitest run src/lib/services/xrpExistingDeposit.spec.ts --maxWorkers=1 --minWorkers=1 --no-file-parallelism`:
  7 passed after using the actual generated `open_vault` event shape. Coverage
  includes account changes before dispatch/clipboard copy, bounded discovery,
  malformed hash, lost reply/no automatic retry and stale response suppression.
- Both changed Svelte components parse successfully. `svelte-check` exits 1
  with 32 errors/66 warnings, no errors in changed files. A bounded diagnostic
  comparison using the exact prepatch generated declarations at `6847982b`,
  same dependencies and unchanged source files found the SAME 32 diagnostics
  (path, location and text), no added/removed diagnostics outside the new files.
  Canonical generated files were restored byte-for-byte immediately afterward.
  Existing errors are in config.ts, Explorer analytics/collateral config,
  stabilityPool.ts, VaultPullRecoveryBanner.spec.ts, docs parameters/redemptions,
  and treasury. This is not a clean project-wide typecheck claim.
- `git diff --check`: exit 0.

No production bundle, new release Wasm, installation, real customer deposit,
withdrawal, signing or customer retry was performed by this source lane. The
existing release owner handles publication and exact artifact checks; existing
case reviewers assess the frozen patch. Review verdicts and served/customer
proof remain separate from local checks.
