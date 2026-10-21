# Stability Pool interest recovery: exact execution proposal

Status: prepared for a separate human decision and normal approval review. No pool upgrade or financial credit has been executed. The earlier backend and archived-slot grants are consumed.

## Requested authority

Authorize **one Stability Pool upgrade attempt**, accepting its unresolved earlier-request risk, followed by the exact sealed **237-packet, 12.56672472 icUSD** accounting repair and interest-only processing while the global emergency pause stays true. Explicitly accept the actual-Wasm migration evidence below as the disclosed alternative to exact old deployed-source reproduction for this scoped attempt.

This grants no second backend upgrade, global pool resume, remint, token transfer, notification replay, journal deletion, held-row promotion, controller change, snapshot replacement or financial-state rollback. Root remains the sole executor. No request is retried or moved to another executor if its outcome becomes uncertain.

## Exact bytes and allocation

| Item | Binding |
| --- | --- |
| Candidate source | `7958051669bf906424e85cfba9dbd2bc39585dcb` |
| Raw Wasm SHA256 | `435d2d43379c74d54e498f35d12e92118025c1db25f98b1a176829e8a08f0177` |
| Uploaded gzip SHA256 | `b26fb0358ad3c5d34256e13f069ae3ddfe50072afffc477f7fbe2d402cb384a8` |
| Raw / gzip bytes | 3,992,246 / 1,066,908 |
| Executable code | 3,748,036 bytes; 256 KiB reserve retained under the 12 MiB network limit |
| Source archive SHA256 | `4fd2babc541f504e590ca496c0e5f74167f6def51a53ad6b3b88821a867c8243` |
| Canonical private Candid manifest | 104,620 bytes |
| Manifest SHA256 | `13fa5654846d816d8debf8de544c358f13d25874a36e31ae35225fac3fba6183` |
| Sealed total | 1,256,672,472 raw icUSD = **12.56672472 icUSD** |

Target: existing pool `tmhzi-dqaaa-aaaap-qrd6q-cai`, network `ic`, identity `rumi_identity`. The exact operator principal is `fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae`. The controller set must remain exactly that developer plus `cpbhu-5iaaa-aaaad-aalta-cai` and `mi66c-zqlu4-4kxd6-2gtp7-szg5v-6a62a-geoty-fahu5-4trje-xyfby-wqe`. The installed backend remains the pool's protocol binding. Participant payloads remain in protected evidence, with the exact manifest seal above identifying their allocation.

The chronological prefix is 194 older retained packets plus 43 modern packets. Its 13 positions include 11 positive allocations and two zero allocations. Each packet conserves its mint amount, uses historical membership and raw-principal ordering, and compounds balances before the next packet. Prior independently checked 194-packet allocations are unchanged.

Existing depositor icUSD sum: 513,891,360,673 raw. Sealed result: **515,148,033,145 raw**. Lifetime pool interest: 5,924,587,594 to **7,181,260,066 raw**. Each participant's balance and lifetime-interest increase must equal its sealed allocation. Other token balances remain exact.

The separate unexplained **25,664,678 raw (0.25664678 icUSD)** ledger excess is excluded and remains held. A fresh 05:44 UTC read additionally found one ordinary later mint, block42166, 4,285,030 raw, with its exact ledger tuple independently verified. It is outside the sealed prefix and is not added to the allocation. Such later packets remain pending until normal idempotent V2 delivery can credit them once after finalization. Ledger balance was 515,177,982,853 raw at that read; this does not change the excluded residual or authorize distributing it.

## Concrete evidence and its limits

The current actual deployed module is `b0e8569ae32b86112fcfadb4717a87fccfbac2cab6be907ae333a5b72b3f7b03`. Its complete snapshot1 payloads are protected; management Running was restored without changing that module, global pause or interest totals. No snapshot slot was deleted for the pool.

Exact old-source identity remains **unproved**. Two clean baseline source builds produced the same different Wasm, with a 135-byte executable-code difference. Matching metadata or State does not establish source equivalence.

Instead, offline execution of the actual deployed Wasm's pre-upgrade hook on copied heap/globals/stable memory produced the actual current State. The exact candidate then executed its own post-upgrade hook on that serialized stable memory. All 49 original State fields were preserved; original virtual maps0,2,3 remained byte-identical. New additive recovery fields introduce the bootstrap hold. This is the specifically disclosed migration alternative requiring explicit acceptance, not a claim to have recovered the old source.

The candidate's actual local Wasm executed 520 recorded steps: authenticated stage, 24 bounded batches, mid-batch restart, finalization, exact permanent receipts for all237 sources, and every237 duplicate acknowledgment. Independent integer accounting matched all13 balances. Duplicate calls left the entire State unchanged and performed no stable writes. Actual returned success bytes decode using the backend's current mirrored result type. Unauthorized callers, wrong seals, cursor/limit errors, locked global resume and conflicting payloads were rejected. A fictional later packet on the local copy credited once while global pause remained true.

The local harness only permits bounded local stable-memory, argument/caller/reply, clock, self, debug and timer-registration imports. Network and timer dispatch are forbidden. Candidate instrumentation changes only exports for local snapshot cloning; original code/data/function/import sections remain exact. Original inputs were not modified. These checks do not establish real timer/network integration, replica instruction metering, future uptime or actual financial credits. Those require the live verification below. No unit or PocketIC suite is claimed.

The full native Rust interface has pre-existing drift in unrelated liquidation hints and a proof variant. This repair changes neither their existing handlers nor proof checks. The published legacy64 method contracts are preserved; exactly five new Rust-derived recovery methods and generated bindings are added. Both directions of scoped compatibility passed. Full Rust/legacy equivalence is not claimed, and broad operations remain paused.

Two independent source reviewers and two independent artifact/migration reviewers returned preparation-only passes. The bounded operator was separately reviewed; a runtime-optimization issue was corrected with unconditional checks. Five local mocked scenarios verify normal and optimized execution, denied authority, wrong installed module and uncertain-call stopping; no real CLI/network was invoked. Actual fresh preflight is reviewed separately after the human grant.

## Bounded execution

1. Reverify direct authorization, root sole ownership, exact refs/files/artifacts, developer principal, exact controller set, current old module, current pause and complete positions/configuration/accounting baseline. Reconcile original211 source joins and newly accrued ordinary tails. Any unexplained baseline drift stops this plan; do not alter the seal silently.
2. Take and verify a fresh pool backup in an unused slot, preserving all older network snapshots and protected files. Restore management Running on the same old module before installation. Snapshot replacement is not authorized.
3. Through normal approval review, submit exactly one `icp canister install` against the existing pool in upgrade mode, using the exact gzip above and typed `StabilityPoolInitArgs` with the existing backend principal. No force/reinstall/alternate executor. Persist an exclusive dispatch marker before submission and preserve every outcome.
4. Verify installed upload hash, Running, embedded scoped Candid, controller set, all original positions/amounts and new bootstrap hold. Before any credit, refresh bounded readbacks and obtain two independent actual preflight reviews. The manifest must still match the exact accounting baseline.
5. Run `scripts/sp_interest_sealed_recovery_once.py` only with the actual grant and fresh reviewed readiness evidence. The 104,620-byte binary is supplied with `--args-file --args-format bin`, avoiding oversized text arguments. One stage and at most24 continuation calls apply at most10 packets per call. Every returned status and subsequent status query must agree. Any error, unknown response or decoder failure stops further updates; reconcile the original operation read-only.
6. While the recovery lock remains, verify every participant's exact sealed balance/lifetime delta, all237 permanent positive receiver receipts, their exact source tuple and allocation digest, pool totals and original211 joins. No global resume is used. Only after these checks, make one bounded `finish_interest_recovery` call with the exact seal; verify `next_packet=237`, total1,256,672,472 and interest-only mode. This unlocks ordinary V2 interest delivery under the retained global pause.
7. Observe natural backend acknowledgment of the original42 modern packets and other versioned duplicates. No manual notification replay or remint. Prove matching original-source receiver credits; observe later ordinary delivery and reconcile its actual credit/event/balances. Read complete paginated modern journals again after a later normal timer cycle, with all query/decoder exits and continuations checked. Report any new intake separately.

Installation and the sealed-credit operator have separate exclusive markers and no automatic retry. The operator deliberately stops with all237 credited but still locked; root must complete receiver/accounting verification before the reviewed finalization call. A failed or unknown update is not permission to restart the script.

## Legacy holds and completion language

The original request's169 Treasury destinations already have exact positive receiver joins. Its remaining42 pool destinations are inside this sealed prefix. Their upgrade/backfill or earlier points work must never be repeated.

The53 old unversioned notification rows remain held by the backend even after their actual pool credits receive permanent receipts. The separately prepared positive-receipt cleanup endpoint is not installed and is not authorized here. It is not a gate for crediting or naturally acknowledging the original42 modern packets. No journal is deleted or promoted from mint evidence alone. The141 old versioned rows can acknowledge exact permanent duplicates naturally.

Mark the original211 complete only when every original Treasury/pool destination has its exact matching positive receiver receipt and actual credits, with the applicable live modern backlog empty across a later normal processing cycle. Disclose any separately retained53 legacy holds, unexplained residual, new intake and incomplete verification. Do not describe every protocol journal as empty while any retained hold exists. If a wider journal-clearance objective is pursued, its additional backend artifact, fresh backup and separate exact authorization remain necessary.
