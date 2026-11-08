# Sui publish, registration and bind: PREPARED, NEVER EXECUTED

Card BB-53 (53b). A runbook for a future, separately reviewed test packet. **Nothing here was run.** No Sui CLI was
invoked against any network, no wallet or key was read, no canister was called, and the command lines below were not
checked against a live CLI. The Move placeholder (`CHAIN_IDENTIFIER` all zero) is unchanged; the first step below is the
one that would change it, in that future packet. Every step needs D6 (who holds which capability) decided and the
provider contract (`sui-rpc-provider-contract`) in place first. Placeholders are in `<angle brackets>`.

Rules for whoever runs it: use `sui move` and `sui client` only with a keyless or purpose-made client config and an
explicit `-e <env>`; never the default client; never `--yes`; the quarantined bootstrap key is never used or funded.

## A. Publish (Sui side)

1. In a scratch copy of `move/rumi_icusd_gate`, set `CHAIN_IDENTIFIER` to the target network's 32-byte genesis digest
   (values and derivation: `move/rumi_icusd_gate/README.md`, "Corrections and choices", item 4). Build and run the 47
   unit tests: `sui move test`.
2. Publish from the publisher address D6 names:
   `sui client publish --gas-budget <MIST> --client.config <config> -e <testnet|mainnet>`.
   Record from the effects: `<PACKAGE_ID>`, the shared `Gate` (`<GATE_ID>`), `MintCap` (`<MINT_CAP_ID>`), `AdminCap`
   (`<ADMIN_CAP_ID>`), `MetadataCap`, the `UpgradeCap`, and the `Currency<RUMI_ICUSD_GATE>` object that `init` sent to
   the coin registry address `0xc`.
3. Register the currency (anyone may; pinned `coin_registry.move`: `public fun finalize_registration<T>(registry: &mut
   CoinRegistry, currency: Receiving<Currency<T>>, ctx)`): a PTB calling `0x2::coin_registry::finalize_registration`
   with type argument `<PACKAGE_ID>::rumi_icusd_gate::RUMI_ICUSD_GATE`, the registry object `0xc` mutably and the
   `Currency` object as a `Receiving` input. Until this runs, `coinMetadata` is null and the canister's bind refuses.
4. Move the capabilities to their roles once the canister has derived them (section B): `MintCap` to the treasury
   address, `AdminCap` and `UpgradeCap` and `MetadataCap` to the upgrade-custody address (or as D6 decides):
   `sui client transfer --object-id <ID> --to <ADDRESS> --gas-budget <MIST>`. The canister's bind records the AdminCap's
   actual owner and does not require it to be ours.
5. The `UpgradeCap` policy (compatible, additive, dependency-only, immutable) is D6's, not part of this packet.

## B. Bind (canister side; `icp` CLI, controller or admin identity as noted)

Fresh install only (there is no supported upgrade from a BB-52 image): install with the saga install arguments, then:

1. Controller: `derive_role_keys` (free calls, no signing). Read `get_roles` for the three Sui addresses. Fund the gas
   address with SUI and note one SUI coin object it owns (`<GAS_COIN_ID>`); nothing funds it automatically.
2. Controller: `set_gate_binding (record { package_id = "<PACKAGE_ID>"; gate_id = "<GATE_ID>"; mint_cap_id =
   "<MINT_CAP_ID>"; admin_cap_id = "<ADMIN_CAP_ID>" })` (set once), then `set_gas_coin ("<GAS_COIN_ID>")`.
3. Install-time or upgrade-time provider set: `UpgradeArgs { mirror = opt record { providers = vec { record { id =
   "<id>"; url = "<https url, no key>" }; ... }; min_quorum = 2; min_independent_providers = 2; max_response_bytes =
   16384; enabled = true }; admins = null }`. Providers must be keyless and independent; none is contracted yet.
4. `AdminCap` holder (a Sui transaction, not the canister): `set_limits(gate, admin_cap, <supply_cap>, <min_bridge_out>)`
   to leave the on-chain DISABLED state (`supply_cap` starts at 0). Raise it only as far as the demo needs.
5. Admin: `observe_deployment ()`. It must return the observed deployment, with the gate pristine (the first observation
   refuses any activity), the `MintCap` owned by the treasury address and 8 decimals with symbol `icUSD`.
   `get_observed_deployment` shows what was agreed; `module_digest_observed = false` is expected.
6. Admin: `admin_set_pauses (record { deposits = false; mints = false; releases = false; observer = false })`
   (refused until the binding, the role keys and the observation exist). `get_mint_lane` must report `ready = true`.

## C. What a rehearsal must check that source tests cannot

The shapes of the real GraphQL responses (`UInt53` as number or string, the `repr` address form, the AddressOwner
fragment), the cycles per outcall and per signature, checkpoint cadence against `HEAD_ROUNDING`, whether an aborted
transaction advances the `MintCap` and gas-coin versions, gas used by a mint (the default budget is 0.05 SUI),
duplicate-submission replies of `executeTransaction`, and provider retention. All are open in MC-DEF-13.
