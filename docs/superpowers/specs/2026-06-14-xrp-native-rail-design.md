# Native XRP Ledger rail — design + status

Status: **integration landed, dormant (not wired up)**. Branch `feat/xrp-native-rail`.
Date: 2026-06-14.

## What this is

A native XRP Ledger (XRPL) collateral rail for the backend, living in
`src/rumi_protocol_backend/src/chains/xrp/`. It mirrors `chains::solana` (both are
threshold-Ed25519 chains) and was ported from a real, shipped integration in the
sibling `delegated-vault` project, following its field guide
(`delegated-vault/docs/guides/xrp-integration-for-agents.md`).

Like the rest of `chains::`, it is **experimental and dormant**: it compiles into
the backend and is fully unit-tested, but no production endpoint calls into it yet.
The user's framing was "get the integration possible, wire it up later."

## Mental model

There is no XRP RPC canister on the IC (unlike BTC / EVM / Solana), so the canister
does two things itself:

1. Builds and signs the XRPL transaction using a threshold Ed25519 key
   (`schnorr_public_key` / `sign_with_schnorr`, the SAME key as Solana, a DISTINCT
   derivation path: the chain-id-144 prefix).
2. Talks to a public `rippled` JSON-RPC node over RAW HTTPS outcalls (read
   balance/sequence/reserve, look up a tx, submit the signed blob).

XRP is a foreign **collateral** chain: a user funds a threshold-derived XRPL
custody address, the protocol verifies the deposit and mints icUSD **on the IC**
(icUSD is IC-native — there is NO icUSD token on the XRPL). On close/withdraw the
canister builds + signs an XRPL `Payment` back to the user.

## Module map (`chains/xrp/`)

| File | Responsibility |
| --- | --- |
| `address.rs` | `AccountID = RIPEMD160(SHA256(0xED ‖ pubkey))`; classic addr = Base58Check(0x00, RIPPLE alphabet). Decode validates + rejects X-addresses / wrong version / bad checksum. |
| `codec.rs` | XRPL binary codec for a native-XRP `Payment` (canonical field order, VL prefixes, native-amount flag bit). |
| `sign.rs` | `STX\0`-prefixed signing message (Ed25519 signs it DIRECTLY, no SHA-512Half), 33-byte `0xED` SigningPubKey, local `SHA512Half(TXN\0 ‖ blob)` tx id. |
| `ted25519.rs` | Threshold-Ed25519 derivation paths (custody/settlement) + `derive_xrp_address` + `sign_message` (hand-mirrored management-canister Schnorr structs, like `solana::ted25519`). |
| `xrp_rpc.rs` | rippled `account_info` / `server_state` / `submit` / `tx` request builders, consensus-safe transforms, parsers, and consensus-retry-wrapped async outcalls. |
| `config.rs` | `XRP_CHAIN_ID = ChainId(144)` (SLIP-44), 6 native decimals, key name, default `RegisterChainArg`. |
| `adapter.rs` | `XrpAdapter: ChainAdapter` — `sign_withdrawal` (build/sign Payment), `verify_deposit` (tx lookup), `fetch_finality`; `sign_mint`/`sign_burn` are `NotImplemented` (no XRPL icUSD). |
| `testdata/xrp_kat.json` | Golden vectors from xrpl.js. Regenerate: `tools/xrp-kat`. |

## Byte-level correctness

Every codec / address / signing function asserts equality against the xrpl.js
golden vectors (`tools/xrp-kat/index.mjs`, deterministic Ed25519 seed). The
committed `xrp_kat.json` was regenerated and verified byte-for-byte. Extra tests
cover the cases the happy-path vector misses: absent `DestinationTag`, `Some(0)`
tag (distinct from absent), the two-byte `LastLedgerSequence` field id, and address
decode rejection (X-address, empty, wrong alphabet, corrupt checksum).

## Consensus discipline (the #1 live-failure cause)

- Each `transform_*` reduces the rippled body to ONLY the consumed fields so all
  replicas converge (volatile server time / load factors / per-edge error tokens
  are stripped). Tested directly.
- Consensus runs over the FULL transformed response (status + body, headers
  excluded), so all transforms pin the status to a constant (`reduced_response`):
  a round-robin cluster can return 200 on one replica and 429/503 on another,
  which would void consensus even when the reduced bodies match. The parsers key
  only off the body, so pinning the status is loss-free. (Found + fixed in the
  adversarial review pass; regression-tested by `transform_normalizes_status_across_replicas`.)
- Reads retry on the transient consensus-miss class (`outcall_read`); `submit`
  uses a NARROWER single-attempt path (`outcall_submit`) so a tx that may already
  have broadcast is never blindly re-issued.

## What is intentionally NOT done (the "wiring")

1. **`main.rs` `#[query]` transform shims.** For the outcalls to run at runtime the
   four transforms must be exposed as canister queries the IC can resolve by name:
   `xrp_transform_account`, `xrp_transform_server`, `xrp_transform_submit`,
   `xrp_transform_tx` (each a one-line delegate to `xrp_rpc::transform_*`). Additive,
   safe; regenerate the `.did` after.
2. **Chain registration.** `register_chain(xrp_default_register_arg())` (dev-gated),
   plus the icUSD-mint-side accounting is N/A for XRP (collateral-only).
3. **CDP vault endpoints** (open/borrow/repay/close against XRP collateral) and the
   **deposit observer** + **settlement** timers (kept OFF, like Solana/Monad).
4. **Destination tags through the trait.** The shared `WithdrawalRequest` has no tag
   field; `XrpAdapter::sign_withdrawal_with_tag` is exposed so tagged exchange
   withdrawals work once wired, without a cross-chain trait change.
5. **Phase 2** (per the field guide): dynamic open-ledger fee, multi-node outcall
   agreement, treating a submit consensus-miss as indeterminate (record expected
   hash + confirm-before-resubmit), issued tokens / trust lines, the XRPL DEX.

## Verification

- `cargo test -p rumi_protocol_backend --lib chains::xrp` → 39 pass (golden vectors
  + parsers + transforms + adapter financial logic).
- `cargo test -p rumi_protocol_backend --lib` → 409 pass, 0 fail (no regression).
- KAT regenerated via `tools/xrp-kat` matches the committed vectors byte-for-byte.

Note: the pre-existing `tests/multi_chain_supply_invariant.rs` integration test does
NOT compile on `main` (a Monad `MultiChainStateV3→V4` migration left it un-updated);
this is unrelated to the XRP rail and was confirmed failing identically before these
changes.
