# rumi_icusd_gate

The Sui side of the escrow-backed icUSD bridge (card BB-51). One package, one module
(`rumi_icusd_gate`), one shared `Gate` that wraps the `TreasuryCap`. Source of the design:
`docs/coordination/multichain/chain-sui.md` (BB-36), section 2. Nothing here is published,
funded or enabled; the Sui profile stays disabled.

Owned path: `move/rumi_icusd_gate/` only.

## Files

- `sources/rumi_icusd_gate.move`: the gate.
- `tests/rumi_icusd_gate_tests.move`: 47 unit tests (`sui move test`).
- `reference/transfer_id_reference.py`: an independent, standard-library-only encoder for the
  transfer id. Its printed vectors are pinned in the Move tests.
- `Move.lock`: pins the framework revision (committed on purpose, as the CLI asks).

## What it enforces

| Function | Who | Checks (abort on failure) |
| --- | --- | --- |
| `init` | publish only | chain identifier is 32 bytes and not all zero; creates an 8-decimal, unregulated currency; wraps the cap; `supply_cap = 0` (disabled); caps go to the publisher |
| `mint` | `MintCap` holder | version; cap belongs to this gate; 32-byte `mint_id` not yet consumed; amount > 0; recipient is not `@0x0` or the gate; `supply + amount <= supply_cap` |
| `bridge_out` | any holder of the coin | version; `amount >= min_bridge_out`; principal is 1 to 29 bytes and not `[0x04]`; subaccount is exactly 32 bytes; `(sender, nonce)` not used before on this chain and gate |
| `set_limits` | `AdminCap` holder | version; cap belongs to this gate; `supply_cap <= U64_MAX - 1`; `min_bridge_out >= 1` |
| `migrate` | `AdminCap` holder | cap belongs to this gate; `gate.version < VERSION` (it never lowers the version) |

`bridge_out` has no pause flag and never reads `supply_cap`: returns work while minting is
disabled, as long as the version guard is satisfied. Every state-changing function except
`migrate` requires `gate.version == VERSION`, so a stale gate (after an upgrade, before
`migrate`) and a gate that a newer package already migrated both refuse the old code.

`transfer_id = sha2_256(bcs(DOMAIN) ++ bcs(chain_identifier) ++ gate ++ sender ++ bcs(nonce))`
with `DOMAIN = b"rumi.icusd.sui.return.v1"`: `vector<u8>` is ULEB128-length-prefixed, an
address is its 32 raw bytes, a `u64` is 8 little-endian bytes (130 bytes in all for a 32-byte
chain identifier).

## Authorities and upgrade limits

- `UpgradeCap` is created by the Sui publish, not by this module, and goes to the publisher.
  Whoever holds it can upgrade under the policy it carries. A compatible upgrade can change
  any function body, so these checks bind the code published today, not a future version. The
  version guard stops stale package versions, not a new one. Inference carried from chain-sui.md
  section 6 (not re-proved here): an additive upgrade can add a function in this module that
  reads the gate's private `treasury` field, so only dependency-only or immutable close that
  path. The policy decision belongs to D6.
- `MintCap`: the only way to call `mint`; bounded by `supply_cap` and the consumed-id table.
- `AdminCap`: `set_limits` (it can raise `supply_cap`) and `migrate`.
- `MetadataCap` (from `finalize`): edits name, description and icon only.
- No `DenyCap` exists and `make_regulated` is never called.
- The `TreasuryCap` is a private field of the shared `Gate`; no function releases it.

## Corrections and choices against chain-sui.md

1. `public fun`, not `public entry fun`. The pinned compiler reports `entry` on a `public`
   function as meaningless (lint WSL01011); PTBs call public functions.
2. Both caps carry a `gate_id` and every cap-gated function checks it. The paper stored none
   because `init` makes one gate per package. The check costs one comparison and closes
   cross-gate use if a second gate ever exists (the tests create two). Tested.
3. Durable records are richer than the paper's. `mints[mint_id]` is `{recipient, amount,
   sequence}` and `returns[transfer_id]` is `{sender, amount, sequence, icp_principal,
   icp_subaccount, client_nonce}` instead of a bare sequence, so the saved destination
   survives past the 30 to 90 day event retention.
4. The chain identifier is the 32-byte genesis checkpoint digest (what a transaction's
   `chain` field uses). The RPC "chain identifier" is its first 4 bytes. The committed
   constant is all zero and `init` aborts on it, so an unbound build cannot be published
   (tested). Set it per network before publish:
   - mainnet: `35834a8ac17ca48fb14ac8f99c17c98747e95dd07294ae41a46b382246a4499b` (short `35834a8a`)
   - testnet: `4c78adacf2a2f5ad80f27ed7d54aa69d3a78f1ca67fdef9ecf5754f5b8bb77b0` (short `4c78adac`)

   Both are the base58 constants `MAINNET_CHAIN_IDENTIFIER_BASE58` and
   `TESTNET_CHAIN_IDENTIFIER_BASE58` in `crates/sui-types/src/digests.rs` at the pinned
   release, decoded by hand; the decoded 4-byte prefixes match the well-known short ids. The
   bind check on the canister side must still read the gate and compare.
5. `bridge_out` checks `amount >= min_bridge_out` once. `min_bridge_out >= 1` is an invariant
   (initial value 1, `set_limits` refuses 0), so a zero coin is refused as well.
6. Confirmed against the pinned source: there is no chain-identifier accessor in `tx_context`;
   `finalize` returns the `MetadataCap` and sends the `Currency` to the registry address
   (`0xc`) on the one-time-witness path, which is why a separate `finalize_registration`
   transaction is still needed after publish.

## Running the tests

`sui move` loads a client wallet config even for a plain build or test. If none is found it
asks to create one, and an empty answer (including a closed stdin) means yes: it generates a
keypair, writes a default config and prints a recovery phrase. Never run it that way here.
Use the supported `--client.config` with a disposable file that has no keys and no network
entries, and the manifest build environment label:

```yaml
# client.yaml, kept in a temporary folder outside the repository
---
keystore:
  InMem:
    aliases: {}
    keys: {}
external_keys: ~
envs: []
active_env: ~
active_address: ~
```

```sh
printf 'n\n' | sui move --client.config <tmp>/client.yaml test -e testnet
printf 'n\n' | sui move --client.config <tmp>/client.yaml build -e testnet
```

With no configured environment the CLI compiles offline; the only network access is the git
fetch of the pinned framework sources. The CLI still creates an empty `~/.sui/sui_config`
directory; remove it if it did not exist before.

Pinned: Sui CLI `1.82.3-bf4073918ed3` (official release asset, checked by sha256), framework
revision in `Move.lock`.

## Not covered here (deferred)

- Publishing, `finalize_registration`, wallet display of a registry-only currency.
- Gas and storage cost of `mint` and `bridge_out`; shared-object latency.
- The event type tag after a package upgrade; upgrade-policy behaviour (the `UpgradeCap`
  exists only at publish).
- The per-network chain identifier values are documented, not asserted by a test.
- An external Move review (a later step in chain-sui.md section 8).
