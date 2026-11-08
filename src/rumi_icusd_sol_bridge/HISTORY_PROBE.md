# History-availability probe (P5) runbook

Blocker: `solana-history-availability-unmeasured` (`docs/coordination/multichain/chain-solana.md`
section 4, P5). The nonce lane needs the SOL RPC canister to keep serving a transaction, or the
signature list of a nonce account, after the nonce advanced. How far back it does is unmeasured, so
`LaneSettings::history_horizon_secs` is a placeholder. This probe measures it. It has NOT been run
against any network: it is verified offline only, with a mock transport.

**Executing it against any public cluster needs separate authorization.** Nothing in this crate
contains a SOL RPC canister id, an endpoint, a cluster default or a real signature.

## Where it lives

* `src/history_probe.rs`: the probe (`run`) and its report types. Reads and reports; changes no bridge
  state; never sends a transaction.
* `src/lib.rs`: the controller-only update `probe_history_availability(ProbeArgs) -> Result<ProbeReport, BridgeError>`,
  compiled only with the cargo feature `history_probe`. Its interface is
  `rumi_icusd_sol_bridge.history_probe.did`.
* Offline tests: `cargo test --offline -p rumi_icusd_sol_bridge --lib history_probe`
  (also with `--features history_probe`).

Why a feature-gated canister helper and not a host example: the SOL RPC canister is reachable only from
a canister (a cycles-paid inter-canister call). A host example would need an IC agent dependency (not in
`Cargo.lock`) and a second transport. The helper reuses the crate's `CandidRpc` and costs one small
endpoint behind a feature; the production wasm is byte-identical with the feature off (checked).

## Inputs (`ProbeArgs`, all explicit, no defaults)

| Field | Meaning |
|-------|---------|
| `sol_rpc` | the SOL RPC canister to probe |
| `rpc.cluster` | `Mainnet`, `Devnet` or `Testnet` (required) |
| `rpc.verify` | consensus for `getTransaction`, `getSignatureStatuses`, `getSlot` (`Equality` is the strict reading) |
| `rpc.discover` | consensus for `getSignaturesForAddress` (for example `Threshold { total = 3; min = 2 }`) |
| `rpc.cycles_per_call` | cycles attached per call; unused cycles are refunded |
| `signatures` | 1 to 8 base58 signatures you believe are older than the horizon |
| `nonce_account` | base58 address that has been written to (its signature list is read) |
| `horizon_slots` | a result counts as old history only if `tip_slot - slot >= horizon_slots` (about 2.5 slots per second; 6 hours is about 54,000) |
| `list_limit` | `getSignaturesForAddress` limit (default 10, at most 100) |

At most 18 calls per run (one `getSlot`, two per signature, one listing).

## Build and run (placeholders only; each step needs authorization)

1. Build a throwaway probe wasm, never the bridge:
   `cargo build --offline -p rumi_icusd_sol_bridge --target wasm32-unknown-unknown --release --features history_probe`
2. Install it on a NEW canister you control, with a valid dummy `BridgeArg::Init` (the probe ignores the
   stored config; it only needs the install to decode) and enough cycles for `calls x cycles_per_call`.
3. Call it as a controller:
   `icp canister call <PROBE_CANISTER> probe_history_availability '(record { sol_rpc = principal "<SOL_RPC_CANISTER_ID>"; rpc = record { cluster = opt variant { Devnet }; verify = variant { Equality }; discover = variant { Threshold = record { total = opt 3; min = 2 } }; cycles_per_call = 10_000_000_000 : nat64; response_size_estimate = null }; signatures = vec { "<OLD_SIGNATURE_1>" }; nonce_account = "<NONCE_ACCOUNT>"; horizon_slots = <SLOTS> : nat64; list_limit = null })'`

## Reading the result (`ProbeReport`)

Per signature, `transaction` (`getTransaction`) and `status` (`getSignatureStatuses` with
`searchTransactionHistory: true`) each give one verdict; `listing` is the nonce account's list.

| Verdict | Meaning |
|---------|---------|
| `Present { slot; age_slots; older_than_horizon }` | Consistent and found. `older_than_horizon = true` means it is at least `horizon_slots` behind the finalized tip. |
| `Absent` | Consistent null: the providers behind this consensus do not serve it. For a signature that exists this IS the finding. |
| `Inconsistent` | The providers disagreed: the lane would wait on this call, and a horizon built on it is not safe. |
| `Error(text)` | The call failed (cycles, provider, transport). Re-run before concluding anything. |

`summary.transaction_served_old`, `status_served_old` and `listing_served_old` are true only when at
least one result was Consistent, present and older than the horizon. `false` means "not shown by this
run", never "history is absent". `tip_slot = None` means `getSlot` failed, so no age could be judged and
nothing counts as old. Use signatures of several ages, repeat the run on different days, and keep
`history_horizon_secs` strictly below the oldest age that was Consistent and present for all three
calls. Record results privately; do not commit signatures or canister ids.
