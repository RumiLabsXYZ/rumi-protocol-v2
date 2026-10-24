# Scoped Stability Pool interest recovery — source preparation

Status: source and allocation preparation. This file grants no production authority. Existing one-attempt backend inspection and archived-slot grants have been consumed.

## Intended result

Restore receipt-backed interest crediting while retaining the Stability Pool emergency pause on deposits, withdrawals, liquidation and conversion. The original notification list has 169 verified Treasury destinations and 42 remaining pool destinations. A subsequent normal pool packet is also retained.

The proposed accounting prefix contains 237 already-minted pool packets: 53 older unversioned, 141 older versioned, and 43 modern packets. They are disjoint and ordered by original ledger mint block. Processing that chronological prefix preserves intermediate compounding before the modern packets.

- Proposed total credit: **12.56672472 icUSD**, or 1,256,672,472 raw units.
- Participants: 13, including zero-allocation positions.
- Canonical private Candid payload: 104,620 bytes; SHA256 `13fa5654846d816d8debf8de544c358f13d25874a36e31ae35225fac3fba6183`.
- Existing aggregate depositor icUSD: 513,891,360,673 raw; proposed result: 515,148,033,145 raw.
- Pool lifetime interest: 5,924,587,594 raw; proposed result: 7,181,260,066 raw.
- Separately unexplained ledger excess: 25,664,678 raw. It is **excluded** from credits.

The private payload contains participant allocations and remains in protected evidence. It is not published in this repository. The exact seal identifies the payload; these numerical references are not authority to apply it.

## Method and safeguards

Actual mint timestamps and complete membership history determine eligibility. Each packet uses icUSD-only pro rata weights, integer floors and the first eligible principal's rounding remainder. Balances compound after every packet. The prior 194-packet allocation was retained exactly. Historical versus current eligibility produces identical monetary credits for this prefix; one EXE packet has only a zero-credit holder-set difference. Native Rust accounting independently matches every proposed participant balance and lifetime-interest increase.

The pool change adds optional durable progress and bootstrap-hold fields. The scoped upgrade begins with unrelated updates held. A sealed manifest must match the actual accounting baseline before it can be staged. All checked additions are validated before writes. Bounded continuations atomically write the exact permanent source receipt, credits, aggregates and cursor without a ledger call or await. A repeated old cursor returns current progress without applying another packet. No reset or receipt deletion is provided.

All eligibility, payout-address, registry, balance and configuration update entry points are held during the batch. Financial timers remain held by the global emergency pause. Finalization verifies every permanent source receipt and the final accounting digest before permitting normal icUSD interest deliveries. No-recipient packets remain pending while globally paused. Exact already-credited duplicates can be acknowledged without another credit.

The emergency pause stays true. This is not full pool activation, a remint, token transfer, financial snapshot rollback or permission to use other financial routes.

## Legacy notifications and separate cleanup

After credits, normal receipt-safe delivery can acknowledge the original 42 modern packets and older versioned packets. The 53 unversioned rows remain held by their existing backend rule.

A separately prepared developer-only backend method can reconcile one such row only after the configured pool returns a positive exact permanent receipt under this seal. It rechecks caller, pool, ledger, payload and retained row after the await, uses the existing delivery guard, and removes no row without that proof. It does not replay a notification, promote its version, mint or transfer funds. Repeated reconciliation still verifies the receiver receipt before reporting an already-absent row.

This cleanup is separate from crediting or acknowledging the original 42 modern packets. The consumed inspection authorization does not authorize another backend upgrade. An additional backend artifact, fresh backup and explicit execution authority would be needed for that optional cleanup; it must not become an invented gate for the original modern credit repair. Held rows must remain disclosed while present.

## Existing interface drift is preserved and disclosed

The pool's published interface metadata differs from its native Rust export in some unrelated liquidation-related methods. This repair does not change them. Broad pool operations remain paused.

The scoped interface adds only the five new recovery endpoints and keeps every existing published method. No claim is made that the full Rust service and the published metadata have been reconciled; broader interface repair is deferred.

## Evidence and remaining execution boundary

A current backup preserves the exact deployed module, heap, globals and stable memory. The same module is management-Running again and remains emergency-paused. Offline execution of its actual pre-upgrade hook on copies produces the current State; all 49 fields and 16,651 nested values are preserved. Its 106,555-byte Candid State is byte-identical to the source decoder's re-encoding. Two independent reviewers verified this bounded result.

Exact deployed source provenance remains unproved: two clean baseline source builds produce the same result, differing from the deployed module by 135 executable-code bytes. The source/State checks do not erase that limitation. Actual candidate-Wasm migration, source and artifact reviews, exact release hashes and fresh live preflight remain outstanding before an execution proposal is ready.

No new pool install, credits, broad resume or held-row promotion is authorized by this preparation.

Completion requires positive matching pool receipts and every participant's reconciled credits, continued normal interest processing, and an empty applicable live backlog across a later normal cycle. Queue absence alone and a successful ledger mint are insufficient. Any separately retained legacy holds remain explicitly reported.
