# Exact missed-points compensation proposal — October 19, 2026

Status: candidate prepared for explicit economic acceptance and one additional points-only upgrade. Not installed, pinned on production, staged or credited. Normal processing is already running.

## Verified processing

The first expressly authorized new upgrade succeeded. A bounded catch-up and guarded V2 resume then led to real B18 capture, real close18 and automatic normal epoch20 opening at **2026-10-19 20:38:20.856258768 UTC** (`1792442300856258768` ns). Driver enabled, transition hold false, automatic ingestion progressed. Original417 canonical ledger entries are unchanged. The39 new ordinary epoch18 rows (including the legacy correction) total **496,527.80218384 points**. Normal epoch20 ends October26; its actual opening/seed commitment/bridge are recorded, with no invented epoch19 capture or summary.

## Exact proposed additional credits

The full private allocation contains **45 rows: 15 audited participants ×3 affected periods**. Every principal/period is included; the actual final census has no excluded or newly eligible historical participant. All registrations predate epoch18. A changed historical census fails closed and requires a replacement or supplemental allocation.

| Period | Target points | Already credited | Additional compensation |
|---|---:|---:|---:|
|18 (Oct5–12) |629,424.09740874 |496,527.80218384 |132,896.29522490 |
|19 (Oct12–19) |712,258.07635948 |0 |712,258.07635948 |
|20 missed prefix (Oct19 00:00 to actual opening) |87,502.28993151 |0 |87,502.28993151 |
|**Additional total** |||**932,656.66151589** |

All amounts use8 decimal places; the exact additional integer sum is `93265666151589` raw points. Original ordinary total `278613015263928` plus compensation equals `371878681415517` raw, before future ordinary earnings. Ordinary whole-epoch awards offset the18/19 targets. Prefix20 compensation ends exactly at actual opening; later normal20 awards remain separate. No participant is paid twice for overlapping coverage.

### Policy requiring explicit acceptance

Absent random snapshots and some historical valuations cannot be reconstructed uniquely. This proposal makes a deliberately generous compensation award from retained evidence and explicit historical-source assumptions. It is not a claim that the exact counterfactual random award is known. Conservative envelopes may overcredit and can change participants' relative shares of the later pro rata airdrop. Points do not transfer funds or expand the token pool.

- Epoch18 uses the saved whole-snapshot corrected ceiling `7*A + floor(7*A/3)`, then offsets actual ordinary credits per participant.
- Epoch19 and20prefix use per-source envelopes and separate integer floors over their exact covered nanoseconds.
- 3pool uses recorded composition `icUSD +4*(ckUSDC+ckUSDT)` and positive additions; reductions are ignored for an upper envelope.
- SP uses savedA18 balances plus retained positive events from beforeA, including the relevant pre-cursor deposit. Every participant receives the full global interest allowance in its envelope. This is intentionally generous. Retained source provenance assumptions remain explicit.
- AMM remains zero under the retained baseline/current positions and complete recorded event history; no synthetic historical prices are inserted.
- Debt uses later positions plus all recorded repayments since19start through the final history freeze, with global dust and consumed-burn allowances added to every participant. Unexpected debt-loss/creation events fail closed. Known source/lifecycle evidence supports the bound, subject to disclosed historical provenance limits; it is not certified continuous historical observation.
- All original417 entries, historical matched5x/unmatched correction and flat4x cutover are preserved. Compensation is separately identified as `OutageMakeWhole`, not relabeled as an observed snapshot award.

The full private evidence and participant rows are retained in the protected recovery store. This public note publishes aggregate quantities and cryptographic bindings only.

## Immutable allocation and evidence bindings

- Allocation digest (domain-separated official typed Candid): `26face060fc4e1f25f4c1614ed19e65cd47f60bedf598ac6d6ed4b85e22c61cd`.
- Canonical evidence digest: `a39618434e1677728d412cfd95db574008cc55ec0de709acf2442ee1a7f63ac1`.
- Original417 canonical Candid hash-chain: `9e9f4594e29ec2cb6223f247a03ad78b816f1d291829982573edada585d0a24d`.
- Official Candid generic encoding and mirrored Rust derived encoding match exactly (2,502bytes). Two independent numerical reviewers reproduced all45 rows, input hashes, offsets, coverage and participant totals.

## Exact candidate release

The only production-source delta from the running recovery source is the immutable allocation digest `None->Some(26face...61cd)` and corresponding comments. Types, stable regions, handlers, interfaces, source guards, seed chain and driver schedules are unchanged. Production is still on the prior unpinned release.

- Candidate raw Wasm SHA256: `279f335ab92518d3e0c6250e4f826bb765c18687c2aa5a0bfb0b119f7b208641` (2,798,952bytes).
- Candidate gzip SHA256: `1942255ef30dc04bf0090e8013983f57a224189fa35b46d52298a57ac1fa2486` (691,320bytes).
- Executable code:2,276,811bytes.
- Source file SHA256: `8e6b718d1160a174359d80f1750d7b900beb740e8c6f9a7acf6d2b0be74113a9`.
- Locked offline compile and two canonical `icp build rumi_points` builds passed with identical bytes; embedded Candid, actual extracted Rust interface, backward compatibility and all48 endpoint modes match. No tests were rerun.

## Requested one-time execution scope

The first limited upgrade authorization has been used. A second upgrade requires a **new explicit authorization** through the normal approval system.

If approved: freshly revalidate the exact release, identity/admin/controllers, root sole writer, actual state/census/source bindings and a new downloaded verified snapshot; perform one upgrade of points canister `bfnu3-6aaaa-aaaab-qhanq-cai` to the exact gzip above. Preserve normal20 opening/seed/coverage and all456 ordinary rows/current totals. Stop on unknown outcomes or unexpected drift. Do not switch executor or bypass review.

After verified preservation, stage only the bound45-row allocation and use the existing bounded continuation: at most100 ordinary rows scanned or one compensation row applied per call. State/total/audit/receipt updates are synchronous and durable. Handle reply loss by reading recorded progress, never blind replay. Once all45 rows are receipted, verify every participant's additional amount, aggregate sum, separate audit log, unchanged ordinary entries, completed receipt counts, idempotent completed continuation and continued normal processing. Do not claim completion before those actual checks pass.

This request does not authorize Treasury, backend, bot, cap, oracle, SP or funds changes.
