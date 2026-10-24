# DON-06d independent final donation audit

## Verdict and scope

**PASS for the revised, bounded conclusions in `reconciliation-report-v4.md` and the complete v4 payment inventory. No confirmed blocker remains to publishing those conclusions or the completed seven-credit repair.**

This is expressly **not** a PASS for the stronger claim that all 1,999 historical individual credits are proved, or that no depositor ever missed interest. The earliest 92 arrivals are verified, but their individual credit history remains unresolved. The final report and CSV correctly preserve that limitation. No further missed amount is positively identified by the retained evidence; unknown amounts must not be credited speculatively.

Root assigned this review before work began and owns the coordination card. I did not read or contact reviewer C or another final reviewer. I read investigator evidence, current/historical source and raw captures, independently recomputed the checks below, and made no production calls. Only this assigned review file was written. Root remains the sole production executor.

## Exact artifacts reviewed

Evidence is held in the operator's private evidence store.

| Artifact | SHA256 |
| --- | --- |
| `reconciliation-report-v4.md` | `f680b656d68150c84ad13a02a0f56a260960462bf88f393a081c29b141e7f67a` |
| `every-donation-reconciliation-v4.csv` | `9cf8a7ef1632719bfea84ee5946d346be5c7f4054650f01246284549b72bf7f8` |
| `pool/DON-02e-v3-timing-and-proof-boundary.md` | `82d1cd571b0a62942fbe27fa2d1ab356f9f59b3b1351be78faa9b3f2bb04af9d` |
| Seven-credit manifest | `30edc54ad35151a4dba7a7ff37d9d17f0bf2a7a4630a3d4a58162bdf65d011a6` |
| Executed v2 executor | `f15f94a666d0ab4fc4bd8127858a018e0262340de380e400cd085e33bc552627` |

I also verified the nested early-equation/v3 freeze manifests against their actual files. The proof-boundary supplement explicitly supersedes broader early-v3 closure claims. The original v1/v2/v3 evidence and failed executor attempt remain preserved.

## Payments and cryptographic coverage

Independently reconstructed the representation-independent ICRC3 SHA256 hash of every raw block in all 427 frozen pages. IDs are exactly 0..42696, with 42696 valid parent links, no gaps/duplicates, and parentless genesis. The computed frozen tip is `e6db178175f30f42020b09cf34b1bc2f1e7c07ae838029900b7cd08c42327679`.

Selected the exact pool default account, not merely any matching owner/subaccount. There are **1999 mints totaling14504987166 raw =145.04987166 icUSD**. Approvals are not cash flow. Independently extended the hash chain through supplemental42697/42698, obtaining `c12dc6cf26ae37bab77eec28af88ec2de53a494bb16a2201e1be58458c61e5e9`.

Inspected and independently ran the official SDK6.1.1 certificate verifier with its evidence write suppressed. BLS signature, built-in mainnet root, delegation/canister range, certificate time, witness root, big-endian index and tip hash pass for42698. The frozen ledger is bound to that certified tip. This is distinct from the stablecoin index-selected tuple proof.

For both ckstable ledgers I independently checked ICRC106 index bindings and each index's `ledger_id`, the saved full account page to its `oldest_tx_id`, and exact canonical ledger/archive amount, timestamp, owner and default-subaccount joins:49 early ckUSDT and48 ckUSDC transfers. Requests use the archive callbacks actually returned by each ledger. I do not claim certification of those entire token histories.

Every CSV row independently joins to the raw icUSD mint block, exact amount, nanosecond timestamp and memo. Counts/sums are92/3987276282 early arrival-only,1800/9963550415 later aggregate,93/481920698 persisted receipts,7/36176230 newer normal donations, and7/36063541 repaired. All1999 rows are explicit and unique.

## Post-April10 reserve reconstruction and receipts

Independently hashed retained raw Candid event values, checked complete324 swap/110 liquidity sequences, rebuilt event deltas and all434 intervals, and joined mint timestamps. Result:430 exact intervals and four short intervals424..427. Exhaustive subset enumeration gives exactly one credited subset in each short window and the same seven missing blocks40815,40820,40839,40855,40858,40870,40873, total36063541. All four ending outputs are ckUSDT, so output-fee uncertainty cannot create their icUSD deficits. Later retained intervals do not provide an offsetting excess. All23 icUSD burns predate the opening anchor.

Read the actual existing pool stable-memory backup, extracted Memory38, traversed its18 B-tree nodes and decoded all93 raw key/value pairs. They equal the exported receipt table; virtual-memory SHA is `fdf84cba1e1bd5eff5b4d856f9f056e3a95982c7859166d5869afccdb0f2fb25`. All93 join exact ledger amount, original memo/nonce and created-at-time, destination/ledger and terminal backend journal. Sum481920698. The newer seven positive logs and exact36176230 book growth are different donations, explaining93→100. All107 current receipt keys were not byte-exported.

I read all 1680 private backend journal records from the existing hashed backup; none contains a candidate original nonce. The old backend stop receipt succeeded and captured state says Stopped, and current pending maps are empty, so normal original-backend retries are excluded. Controller repair receipts are a different caller namespace.

## Early92: challenge, resolution and accepted limit

The early native flow equation and fee-model replay reproduce exactly. I independently reproduced the six early swap outputs/435799 fees and ROC633 output34580704/fee24223 using pinned integer algorithms, without convergence fallback. Amount flows, extra ledger fee debits1500000 and admin40219 are correctly separated; no double subtraction occurs.

I challenged the initial reasoning about possible unbooked early swap outcomes; the revised evidence addresses the observed alternatives rather than assuming them away.

I independently decoded the actual old Memory0 SCL/DIDL config, verifying the March17→19 last-ramp tuple and A500 for VP26. Independent128-case enumeration reproduced exactly one native-reserve match: all observed swaps and the initial stable pair booked, reserves[89397705201,1073254952,1076019217]. All127 other enumerated vectors have converged strictly increasing neighboring native VP values bracketing the target, without an exact match. I also reproduced the timing supplement's1024 cases, suppressing its output write: only the same fully booked observed-input case matches. No canonical flow falls in the truncated boundary second.

Independently classified all 55 actual early nonic outputs: 48 retained-event outputs, four early proportional output legs and three pre-liquidity-log legs. The 23 actual icUSD burns have corresponding success-side LP burn logs.

**However, they do not exhaust every historical trace.** Part of the pre-March-13 operation history was not retained, so the 560374 raw margin below the smallest early mint (9500234) is a bound within the observed-transaction model, not an unconditional proof of every early credit. The overriding supplement, final report and early CSV status state this limit clearly. It does not undermine the independently bracketed October 14 repair and does not establish another repair amount.

## Actual repair and later continuity

Inventoried all25 v2 requests/exit-zero receipts: exactly seven financial calls, each matching the frozen nonce/block/token0/amount/caller/target, with an exclusive pre-dispatch marker and exact raw/typed Ok. No unknown outcome or automatic retry appears. The preceding v1 directory has only four identity-command files and parse-error exit2, before capture/financial dispatch.

Rebuilt before/after state directly from individual raw management and typed query files:

| Quantity | Before | After |
| --- | ---: | ---: |
| icUSD book, raw |1050977906725|1051013970266|
| Physical icUSD, raw |1051033626687|1051033626687|
| LP supply, raw |1485147981057|1485147981057|
| Donation count |100|107|
| VP |1125659911286418330|1125684141892726645|

Book increase is exactly36063541; other reserve books/admin fees/module/settings are unchanged. Claims/fence/pending maps are empty. Seven matching consecutive controller logs corroborate dispatches. Residual physical minus reserves/admin is19616202, untouched. No mint/transfer/install/stop/export occurred in the repair executor.

Inspected the95-call local fixture and verified20 input/harness hashes, including exact deployed pool gzip c17b4e71312eb60cbb1dcaeeabb2efa927ee99fc9788c60a42f9bb2f56117615. Recorded replay/conflict/auth/backing checks pass; no production call or normal upgrade preservation test is claimed for that fresh fixture.

Independently ordered all2489 captured LP blocks0..2488; last change timestamp1791426538647813278 is October8, before all seven October14 candidates. No historical LP reallocation was attempted. Closing-live03 at00:55:35UTC matches post-repair books, VP, supply,107 receipts and physical icUSD; its fiat physical balances match the earlier baseline, and pending maps remain empty. This is later captured continuity, not a production restart/replay test.

## Final disposition

Confirmed blockers to the revised bounded report: **none**. Broader individual early-credit assurance: **still unavailable**, explicitly disclosed and not approved by this PASS. The0.10 initial partial-deposit claim and0.19616202 residual were not swept into yield; their ownership must remain separate from interest. The modeled2.63% APY is appropriately distinguished from actual historical payments.

Read-only worktree inventory reported zero eligible removals. I created or removed no worktree. Proposed durable lesson: an observed callback inventory must not be presented as an exhaustive historical trace when part of the operation history is unavailable.
