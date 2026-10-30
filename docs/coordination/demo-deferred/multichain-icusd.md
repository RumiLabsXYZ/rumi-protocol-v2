# Deferred work: Multichain icUSD groundwork

Lane file for `DEMO_DEFERRED_WORK.md` (demo-first policy, 2026-10-29). Owner: the Multichain icUSD coordinator, supervised by
Miles. It records only the shortcuts this groundwork actually took (2026-10-30). Every destination profile is disabled, so no
entry affects existing client assets; each must be revisited before that destination is offered for client money.

| ID | Feature | Owner | Concern or skipped work | Why not essential now | Limitation and containment | Before client money | Status |
| --- | --- | --- | --- | --- | --- | --- | --- |
| MC-DEF-1 | Sui gate (BB-51) | future BB-51 owner | External Move review of `rumi_icusd_gate` deferred (Miles) | The gate is specified, not written | Sui stays disabled; blocker `sui-gate-module-missing` | Independent Move review of the built package with its unit tests | open |
| MC-DEF-2 | Ed25519 signing (Solana, Sui, XRPL) | future family owners | The PocketIC threshold-Schnorr seams were cited, not rerun; `solana_m2_sign_pic.rs` is best effort | Groundwork claims capability only | No execution proof claimed; profiles disabled | Run the seams and family signing vectors; record pass or skip honestly | open |
| MC-DEF-3 | Live provider behavior (all new chains) | Miles (probe authorization) and BB-46 | No real-network probes: Robinhood `finalized` tag and keyless providers, HyperEVM historical state, Monad fast-tag consensus, XRPL non-443 outcalls, Sui GraphQL outcall consensus, Solana history retention | Probes need separate authorization | Each is an activation blocker in its profile | Authorized read-only probes with recorded evidence | open |
| MC-DEF-4 | Cycle cost (all new instances) | future family owners | Outcall and signing cycle costs not measured | No instance is built | No instance runs | Measure per instance before funding | open |
| MC-DEF-5 | Destination profiles | BB-60 owner | Fields left empty or not added: no Sui devnet record (no stable identifier), Ethereum endpoint list empty, Solana named wallets empty, Solana refund-id domain undefined, Solana token-program, memo-program, nonce-pool, history-horizon and minimum-refund fields not added | Values not sourced or not yet designed | Validator passes with these gaps; all disabled | Source or design each value, update the profile, rerun the validator | open |
| MC-DEF-6 | Groundwork design and notes | Miles | No independent adversarial review rounds of the design or chain notes; evidence is coordinator source checks plus Miles' scoped reviews | Demo-first: no extra review committee for paper groundwork | Nothing executable was produced except the host-only profile crate | Focused security review of each family's implementation before activation | open |
