# Composed XRP correction: installed and verified

**Live and verified on November 18, 2026 at 12:17:51 PM PST. Stability Pool new liquidations remain paused.** Rob's direct approval message `01a1f040-0d30-7150-acc8-0b87ca78ec7b` authorized [exact packet 39d7baef](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/39d7baef2d3bf049fafd2ec9659578ed15555c48/docs/operations/xrp-composed-partial-deployment-20261118.md), including both named private exports and its preservation procedure. The frozen approved artifact and equivalent fresh arguments were used without rebuilding.

Only backend `tfesu-vyaaa-aaaap-qrd7a-cai` on `ic` was upgraded using existing `rumi_identity`. Final management status is **Running**, installed module SHA256 **`4409bfb7d3bd04aab2fd48f22a850656a0fefda471f285b616f6562444effdde`**. Controllers/settings are unchanged. Embedded Candid SHA256 is `0279b1f5395cf323e479ba18d842419232b371e8850eb246429fad4e8792ebb6`; fresh typed upgrade arguments match frozen hex `26cbae560b9d7020ff305b4775f0429b5164fb490a11e7aee711ba246a406cb3` and preserve the pause before timers. Source-matched build remains 67a5be7a / runtime 1165a897. No hosted CI or main merge is claimed.

## Preservation performed

- Fully exported and verified oldest snapshot `00000000000000170000000001f088fe0101` to the exact approved private directory. Complete metadata, declared component sizes, full hashes, fsync and private permissions passed. No restore test was performed.
- Stopped only backend on the expected `ce014cc2…` baseline, then replaced only independently archived 17 with actual fresh stopped snapshot **`00000000000000210000000001f088fe0101`**. The other nine IDs `18`, `19`, `1a`, `1b`, `1c`, `1d`, `1e`, `1f`, `20` and all their timestamps/sizes remain exactly unchanged. No standalone delete or restore was called.
- Fully exported and verified fresh snapshot 21, including complete metadata, component lengths, full hashes, fsync, normal empty CLI controls and exact saved ce014 module. Raw state remains local/private; only hashes and sanitized results are pushed.
- Reconciled all 51 active vaults from the structurally linked tree in the complete stopped heap, rather than using stale serialized stable-State. IDs, owners, collateral types and raw collateral match the refreshed pre-stop anchors. All 51 pre-stop/stopped debt values match.
- Dispatched one exact 4409 upgrade and verified module/Candid/settings while Stopped, then started only backend. Stop, single replacement, install and start each dispatched once and exited 0. No unknown outcome was blindly replayed.

## Post-start verification

All 51 public vault IDs/owners/collateral types/collateral remain unchanged. **Every post-start debt and accrued-interest value exactly matches the active stopped image**, with aggregate debt `1553251078443` e8s; per-collateral debt totals also match. The earlier cached protocol-status debt was `1553250376787` e8s. Its snapshot predates the refreshed pre-stop vault query; it is not treated as debt changed by the upgrade.

All 4 XRP claim tuples are unchanged. Pending payouts are 0 and reserve journal rows are 0. Protocol mode, static risk/collateral parameters, retained operation/outbox/recovery/configuration surfaces, bot allowlist/claim IDs/tolerance and paid-rail settings pass scoped comparison. Vault 51 remains ClaimConfirmed with no error, original collateral fenced and no settlement/adoption observed; only its existing authorized automatic reconciliation timestamp changed.

Observed oracle prices/provider timestamps, cached snapshot time and derived CR/weighted-interest/resolved-fee/recovery-threshold projections changed through the existing timer/cache paths. The actual deltas are recorded explicitly; risk parameters and curve multipliers remain unchanged. Current status ICP price/time match current collateral-provider fields. No post-start debt/accrual drift was excused as a timer change: all those values match the stopped image exactly.

Authenticated immediate and post-check queries read pause=true. Adrian's independent public read confirms the exact 4409 module and approved public controllers; his actual anonymous app-declared query returned `(true)`. Merritt independently queried the pause as true as well. Adrian's evidence is limited to public module/controllers and pause; Running and full retained-state/backup reconciliation are established by the executor's management/private readbacks.

## Proof boundaries and retained holds

The deployed code includes native-XRP recovery sizing, expired-new-admission refusal and preserved exact legacy post-burn settlement, together with the prior automatic replacement-signature bound and pause support. Installation/state verification is not a funded liquidation or payout demonstration. No production burn, signature, payment, claim retry or compensation was used as a test, and this does not repair Vault 208's historical liquidation. Receipt polling remains possible; zero future cycle/RPC use is not claimed.

No SP resumption, funds/reserve/cap activation, Vault 202 recovery, bot upgrade/worker resumption, frontend publication, controller/permission change, snapshot restore or automatic rollback occurred. All separate financial/liquidation/UI holds remain. Existing independently authorized Vault 51 automatic work is preserved. Any later backend writer must compose against installed 4409 and retain both fixes and pause support.

[Sanitized machine-readable execution receipt](evidence/xrp-composed-partial-20261118/live-verification-20261118.json), SHA256 `7b71d9ec4758d9bc6191f17a525c40a336d2eebcfb84f525adf879745fcf505d`, contains all operation exits, backup component hashes, retained-state checks, exact observed deltas and query-evidence hashes. [Independent public module/pause receipt](evidence/xrp-composed-partial-20261118/independent-live-result-20261118.json) binds the separate readback. Original private command/response/snapshot receipts remain preserved. Worktree inventory/eligible cleanup removed 0; no eligible worktrees were deleted.
