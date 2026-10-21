# Proposed retained-route inspection release

## Purpose and limits

This is a proposed single backend upgrade to expose already-retained interest-delivery routes through the existing developer-only recovery query. The source patch is `c98db171c7d7722e50ee8ed8f41313a1f4350800`, based on the deployed backend source `cc224b3edf0f098779cb87d1c09bb245affd8c55`.

Exact source collateral is needed to reproduce the delayed pool packets' recipient allocations. The current query omits that retained field. This additive query is one concrete inspection path; equivalent authenticated retained-state evidence would also work. A query-only source change still requires a real canister upgrade to become available: the unchanged backend lifecycle runs during that upgrade, rearms its normal timers, and schedules immediate durable payout/refund processing via `process_pending_transfer` (`src/rumi_protocol_backend/src/main.rs:1211`). This is a real lifecycle action, not a passive query invocation; fresh pending-journal checks and post-upgrade reconciliation must cover that ordinary processing.

This proposal does not authorize a pool upgrade, global pool resume, legacy receipt promotion, new mint, manual notification replay, allocation application, journal deletion, timer setter, cap change or fund transfer. The pool's existing emergency pause and earlier ambiguous notification holds stay in place. The completed points recovery and Treasury upgrade/backfill are outside this operation and must not be repeated.

## Earlier-request risk and authority

The earlier backend release record preserves a terminally rejected known upgrade wrapper and a claimed other request whose internal identity and terminal disposition were never established. A later normal-reviewed reconsideration succeeded on the current backend, but that success is not a cancellation receipt for the unidentified earlier request. This risk remains separate from the completed Treasury operation and the pool's own earlier installation hold.

Before this proposed single backend upgrade, obtain either authoritative earlier-request disposition or Rob's explicit authorization for this exact new attempt accepting that unresolved backend-request risk. Existing recovery intent and the consumed Treasury-only grant must not be presented as that risk acceptance. Normal approval review still applies. There is no alternate executor or indirect installation path in this proposal.

## Exact target

- Canister: `tfesu-vyaaa-aaaap-qrd7a-cai` (existing production backend).
- Network: `ic`; operator identity alias: `rumi_identity`; install mode: `upgrade`.
- Expected current uploaded-module SHA256: `3b51aca404bb8da8994b55273114571a33486b99c3275547c809746254128566`.
- Candidate uploaded-module SHA256 after one authorized upgrade: `aad0b7e133a27938ed25ebebbd5d4de5d7bb1793cf0d5ccbad5210c4644da259`.
- This is not a Stability Pool or Treasury install. Fresh identity/controller validation is still required at execution.

## Exact artifact

| Item | Exact value |
| --- | --- |
| Raw Wasm SHA256 | `0ab4426b04a7ad1b18338af65c278792a5a7c58f205273f4502f485e7e0b7311` |
| Upload gzip SHA256 | `aad0b7e133a27938ed25ebebbd5d4de5d7bb1793cf0d5ccbad5210c4644da259` |
| Raw / upload bytes | 12,914,848 / 3,069,976 |
| Executable code bytes | 12,255,617 |
| Network limit / retained project reserve | 12,582,912 / 262,144 bytes |
| Additional headroom beyond reserve | 65,151 bytes |
| Toolchain | `rustc 1.92.0 (ded5c06cf 2025-12-08)`; `ic-wasm 0.9.10` |

The raw Wasm includes the complete Rust-generated public Candid metadata and uses the repository's canonical `icp.yaml` backend pipeline: locked offline Cargo release build, public `candid:service` metadata, `ic-wasm shrink --keep-name-section`, deterministic `gzip -9 -n`. This is a local release artifact, not installed-byte evidence. Full source Candid, source/declaration copy, JavaScript and TypeScript bindings agree.

The build exited0. Embedded/source Candid equality and exact gzip decompression passed. All prior Wasm export names and kinds remain identical; executable code grew1,639bytes. Earlier source checks passed: locked offline wasm32 library check, new-service subtyping, strict route-only projection against the old complete interface, canonical declaration generation and whitespace checks.

## Bounded execution after separate authorization

1. Root remains the only executor. Confirm the explicit authorization applies to this exact single backend artifact; a consumed Treasury-specific grant is not authority for this operation. Preserve any original request evidence and use normal approval review. Never repeat an unresolved submission or change executor to avoid review.
2. Freshly revalidate the expected deployed backend module, operator identity/controllers, exact source/artifact hashes, full live interface, protocol mode/manual override, cap, bot allowlist, pause/foreign-chain/financial holds and all retained journals. Halt on unexpected drift. Ordinary activity is not authority to restore a stale financial snapshot.
3. Take a new protected compatible backup through a bounded stop/snapshot/download/start operation. Verify every downloaded payload against management metadata and verify the old backend is Running again with retained state before submitting the upgrade. Keep all older backups and the new backup; do not prune or restore them as part of this operation.
4. Generate explicit `ProtocolArg::Upgrade` arguments with `mode = null` and a meaningful retained-route-inspection description. Submit exactly one upgrade through normal review, record its exclusive request/receipt, and follow that same request to a terminal outcome. An ambiguous outcome halts execution; it is not permission to retry.
5. Verify actual installed upload hash, Running status and complete Candid equality against the candidate. Compare every retained pre-upgrade tuple, phase, amount, block, cursor and other journal field; compare operator/controllers and all holds. Reconcile ordinary concurrent changes explicitly instead of overwriting them.
6. Query the existing recovery method with bounded pages and typed decoding. Every returned route must be the stored enum; preserve routes keyed by original operation nonce and mint block. Require complete pagination, sorted unique IDs and exact joins to original ledger tuples. Do not decode the old deployed response using a required-field candidate interface and pretend missing data is a real route.
7. Confirm normal backend publications/processing continue under the existing freshness/admission guards. No forced mode or timer mutation is part of this proposal. A Running module alone is not admission or accounting proof.
8. Use the authentic routes to finish the separate pool allocation and receipt repair artifacts. No receiver credit or overall backlog completion is claimed by this inspection upgrade. The original notification objective remains open until actual matching receiver receipts and an empty live backlog are verified across a later normal processing cycle.

## Review status

Both independent source-preparation reviewers passed, followed by two independent artifact/proposal preparation reviews. Both reviewers then confirmed the explicit target, immediate durable payout/refund lifecycle disclosure and unresolved earlier-request authority framing. No blocking preparation finding remains. These are preparation verdicts; no live upgrade has been submitted for this candidate.

No unit or PocketIC tests were run for the four-line response addition. Stable-state definitions and pre/post-upgrade handlers are byte-identical to the source base; this patch does not introduce a state migration. Source/interface and artifact checks do not substitute for fresh production backup, authority or actual post-upgrade readback.
