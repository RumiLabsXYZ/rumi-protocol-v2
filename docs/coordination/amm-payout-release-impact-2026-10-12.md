# AMM payout-journal release impact (source review, 2026-10-12)

This note reviews the AMM source on `codex/amm-payout-integration-20261012` after merging `origin/main` at `a14bc704`. It is not an install decision or a claim about the live AMM.

## User-visible behavior of this candidate

| Entry point | Candidate behavior | Operational implication |
| --- | --- | --- |
| `swap` | Always returns a typed held error. | Legacy swaps cannot execute. |
| `swap_v2` | Runs only in a `pocketic-test` build; a production build returns a typed held error. | A production install cannot restore swaps. |
| `add_liquidity` | Always returns a typed held error. | Legacy deposits cannot execute. |
| `add_liquidity_v2` | Returns a typed held error after checking the request ID length. | A production install cannot restore deposits. |
| `remove_liquidity` | Ignores global maintenance mode, but rejects a paused pool. It journals both outbound legs before burning LP shares and pauses the pool while payouts are unresolved. | An unpaused pool can begin an exit; a paused pool requires recovery and authorized unpause before another exit. An ambiguous payout must remain held. |
| `claim_rewards` | Journals the outbound reward payout. | A failed or ambiguous payout remains held and blocks a second claim for that pool/caller until reconciled. |
| `claim_pending` | Always holds legacy pending claims that have no bound outbound identity. | Existing legacy claims need separate evidence-based resolution; the new journal does not make them payable automatically. |

The held production swap/deposit behavior is a deliberate safety containment in this branch, but it has a material service cost. Any release decision must explicitly accept that trading and new liquidity deposits remain unavailable until the journaled paths have completed lifecycle verification and are enabled in a separately reviewed build. The global maintenance flag alone does not describe exit availability: per-pool pause and unresolved payout state also matter.

## Verification completed on this integration checkout

At HEAD `26635247de6407a8779d3eb5417a4ee7bb1b1587`, with local changes limited to tests and this note, `icp build rumi_amm -e mainnet-live` produced `.icp/cache/artifacts/rumi_amm` with SHA-256 `994ad8efadf9f5fa178f2c824c68d0d200f898d414498745f924d517d61ac2b1`. The AMM source tree at that commit is `a779eed073a88de79b2bfb3e8ea4885969a0f654`, and `git diff HEAD -- src/rumi_amm/src` was empty. The populated V5 decode suite passed 7/7. The explicit `production_ingress_hold` test passed 1/1 in PocketIC 7.0.0 using that exact artifact hash. The new `populated_v5_post_upgrade` test also passed 1/1 against the same hash: it injected a populated frozen V5 snapshot behind a minimal predecessor with no `pre_upgrade` hook, executed the candidate's real `post_upgrade`, and checked query-visible reserves, LP shares/rewards, claims, event history, held ingress, and empty new journals. The embedded Candid metadata matched the checked-in interface except for a trailing blank line. These checks validate candidate bytes and the specified behavior; they do not show an upgrade of the live AMM, the installed predecessor's `pre_upgrade`, or recovery of any existing liability.

A separate ignored, hash-gated PocketIC test then installed the exact Wasm observed on the live AMM (`832485e330faa18fd3f7b2d05b3dca670698227e420e255a128b0333c2979fb2`). It created a pool, deposited through real ICRC ledgers, and notified funded rewards through the predecessor's public endpoints. The predecessor's actual `pre_upgrade` and the candidate's `post_upgrade` ran. The test passed 1/1 and preserved query-visible reserves, LP shares, pending rewards, liquidity and admin events; it also decoded the predecessor's stable snapshot to check its reward accumulator and distribution counter. All production swap/add ingress methods stayed held after upgrade, with no new request or payout journal rows. Three additional focused swap lifecycle tests passed 3/3 for ambiguous-input fee identity across upgrade, replay and stale request rejection, and the durable pool hold. These local fixtures exercise a populated representative state, not the live canister's full historical state or liabilities.

## Evidence still required before a release claim

1. Reconcile the live AMM's actual state and historical liabilities against a pre-upgrade readback. The exact installed predecessor's real `pre_upgrade` has now passed with representative populated state, but the local fixture is not a copy of the live state and cannot prove every live legacy row is recoverable.
2. Exercise native-ledger receipt and failure cases through the public recovery endpoints, including ambiguous replies and upgrade between dispatch and reconciliation. Test-only verifier adapters and direct source inspection are narrower evidence.
3. Reconcile the live canister's module hash, controllers, maintenance/pool-pause settings, balances, claims, and outstanding operations immediately before any install. Verify state and module hash after any authorized upgrade. Source, tests, build, and install are separate proof states.

This note adds no prerequisite to unrelated canister releases. It describes the service impact and evidence needed to evaluate this AMM candidate itself.
