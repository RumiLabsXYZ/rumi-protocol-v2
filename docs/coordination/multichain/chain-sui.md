# BB-36 Sui groundwork: escrow-backed icUSD on Sui

Card BB-36 (worker W20), 2026-10-30. Design on paper only: no code, build, test, RPC probe, canister call, wallet action or Sui CLI was run. Nothing is deployed or enabled; the Sui profile stays `disabled`.
Settled for planning (Miles under Rob's authority, no live action): **D4** = a small Move gate wrapping `TreasuryCap` with mint-id consumption, counters and recipient-bound returns, with `UpgradeCap` a separate disclosed authority; **D9** = synthetic test ledgers for future testnet demos. **D6** (controllers, custody, who holds which capability) stays open; no controller set is invented here.
Labels: **F** fact with a source key, **R** recommendation, **U** unverified or unmeasured. "Design N" = `docs/superpowers/specs/2026-10-30-multichain-icusd-groundwork-design.md` section N. Web pages were read 2026-10-30 through a fetch summarizer, so quoted wording is its extraction; pin each fact with a test at implementation.
Sources: S1 docs.sui.io/standards/currency | S2 docs.sui.io/references/framework/sui_sui/coin_registry | S3 docs.sui.io/references/framework/sui_sui/coin and raw `MystenLabs/sui` `crates/sui-framework/packages/sui-framework/sources/{coin,balance}.move` | S4 move-book.com/programmability/events and docs.sui.io/references/framework/sui_sui/tx_context | S5 move-book.com/programmability/module-initializer | S6 docs.sui.io/develop/publish-upgrade-packages/upgrade and docs.sui.io/concepts/sui-move-concepts/packages/custom-policies | S7 docs.sui.io/develop/transactions/transaction-auth/intent-signing and raw `MystenLabs/sui` `crates/sui-types/src/{crypto,transaction,digests}.rs` | S8 docs.sui.io/guides/developer/sui-101/sponsor-txn, docs.sui.io/develop/sui-architecture/epochs, docs.sui.io/guides/developer/common-errors | S9 docs.sui.io/onchain-finance/asset-custody/address-balances/using-address-balances | S10 docs.sui.io/guides/operator/exchange-integration | S11 docs.sui.io/develop/accessing-data/graphql/{query-with-graphql,graphql-rpc} and docs.sui.io/references/sui-api/sui-graphql/beta/reference/types/inputs/event-filter | S12 docs.sui.io/develop/accessing-data/rpc-providers | S13 `docs/coordination/multichain/sources-native.md` (BB-32; its Sui section carries more primary URLs) | S14 `foundry/src/BridgedIcUSD.sol` (repo; the EVM model, lines 80-130, 195-290) | S15 docs.sui.io/develop/objects/versioning.

## 1. Representation

| Item | Choice | Basis |
| --- | --- | --- |
| Standard | **R:** Currency Standard, created with `coin_registry::new_currency_with_otw(otw, 8, symbol, name, description, icon_url, ctx)` inside the gate module's `init`. It returns a `CurrencyInitializer` (hot potato) and the `TreasuryCap`. The `init` consumes the initializer with `finalize`, wraps the cap into the gate and never lets it reach an address. | F S1, S2 |
| Why the OTW path | `init` runs once, at publish only, and takes just the OTW and `TxContext` (F S5), so the package has exactly one gate and no key ever holds a bare cap. Cost: a separate transaction `coin_registry::finalize_registration` (registry is the system object `0xc`; anyone can send it) before the registry lists the currency (F S1). The non-OTW `new_currency` needs `&mut CoinRegistry`, which `init` cannot take, so the cap would sit in a PTB with no one-time guarantee. | F S1, S2, S5 |
| Decimals | 8, display only; amounts are `u64` (design 3). Symbol, name and icon follow the D8 labelling rule (escrow-backed; not decided here). | F S1, S3 |
| Regulation | **Unregulated. Never `make_regulated`** (irreversible; it mints a `DenyCapV2` and the `0x403` deny list applies). Bind check: registry regulated state is not regulated and no `DenyCapV2` exists for the type. | F S2 |
| Supply model | Unknown (cap holder mints and burns). Never `make_supply_fixed_init` or `make_supply_burn_only_init`: both take the cap, which the gate never releases, and they end minting. | F S1, S2 |
| Metadata authority | `finalize()` returns a `MetadataCap` (claimed). **R:** send it to the `upgrade-custody` holder with `AdminCap`; call `delete_metadata_cap` (irreversible) only once name, description and icon are final. | F S1, S2 |
| Wallet display | The OTW path creates no legacy `CoinMetadata`. Whether Slush and others read the registry is unverified (BB-32 question 8). Blocker `sui-wallet-display-unverified`; test on testnet first. | U |
| Cap holder | The **shared** `Gate` object holds `TreasuryCap` as a field (`TreasuryCap` has `key, store`, F S3). Shared because a user's `bridge_out` must burn through the cap and bump counters without the canister's signature; an owned gate would force custodied returns (Design 2.3 fallback, states `RC`/`BU`). Consensus sequences shared-object reads and writes and assigns their versions (F S15), so the gate itself has no owned-version contention; latency is unmeasured. | F S3, S15, R |
| Amount bound | `Supply` aborts on overflow. `balance.move` asserts `value <= u64::MAX - supply` (so supply may reach `U64_MAX`); the `coin::mint_balance` doc comment says it aborts at `>= U64_MAX`. The code and comment differ by one unit. **R:** keep the conservative bound (`supply_cap <= U64_MAX - 1`, as Design 3 and the profile already say). | F S3 |

## 2. The Move gate module `rumi_icusd_gate` (specified, not implemented)

One package and one module, `rumi_icusd_gate`. Its one-time witness must be named after the module (Move one-time-witness rule, not re-read here), so the coin type is `<package>::rumi_icusd_gate::RUMI_ICUSD_GATE` (cosmetic; wallets show registry name and symbol, U). A nicer type name would need a second module and a generic `Gate<T>`; not recommended for size.

**Objects and constants**

| Item | Layout | Notes |
| --- | --- | --- |
| `Gate` (shared, `key`) | `id`, `version: u64`, `treasury: TreasuryCap<RUMI_ICUSD_GATE>` (wrapped), `chain_identifier: vector<u8>` (32 bytes), `supply_cap: u64`, `min_bridge_out: u64`, `minted_total: u128`, `burned_total: u128`, `mint_count: u64`, `bridge_out_count: u64`, `mints: Table<vector<u8>, MintRecord>`, `returns: Table<vector<u8>, u64>` | `MintRecord = {recipient: address, amount: u64}`; `returns` maps `transfer_id` to its sequence. The tables are durable state and are the long-lived evidence (events are not, section 4). Per-entry storage fee is paid and never rebated (we never delete); unmeasured. |
| `MintCap` (`key, store`) | `id` | Owned by the `treasury` role. Sole authority to call `mint`. |
| `AdminCap` (`key, store`) | `id` | Owned by the `upgrade-custody` role with `UpgradeCap` and `MetadataCap`. Needed for `set_limits` and `migrate`. |
| Constants | `VERSION`, `DOMAIN_RETURN = b"rumi.icusd.sui.return.v1"`, `CHAIN_IDENTIFIER` (per network), `MAX_SUPPLY_CAP = U64_MAX - 1` | `CHAIN_IDENTIFIER` is the only source difference between networks (Move has no chain-identifier accessor, F S4); `init` copies it into the gate so the bind check can read it. No `gate_id` is stored in the caps because `init` creates exactly one gate per package. |

**Entry functions and exact checks** (any failed check aborts the whole transaction)

| Function | Caller | Checks | Effects |
| --- | --- | --- | --- |
| `init(otw, ctx)` | Publish only | OTW is the module's witness | Creates the currency (8 decimals), `finalize`, wraps the cap, sets `supply_cap = 0` (**no mint can succeed until an admin raises it; the on-chain DISABLED state**) and `min_bridge_out` to a per-network constant, shares `Gate`, transfers `MintCap`, `AdminCap`, `MetadataCap` to the publisher (D6 decides who). |
| `mint(gate, mint_cap, mint_id, recipient, amount, ctx)` | Holder of `MintCap` (passed by reference) | `gate.version == VERSION`; `mint_id` is 32 bytes and not in `mints` (else `EMintConsumed`); `amount > 0`; `recipient` is neither `@0x0` nor the gate address (as EVM, S14); `supply + amount <= supply_cap` | Inserts `mints[mint_id]`; `mint_count += 1`; `minted_total += amount`; `coin::mint` then `transfer::public_transfer(coin, recipient)` (no recipient-side opt-in, F S13); emits `Minted`. |
| `bridge_out(gate, coin, icp_principal, icp_subaccount, client_nonce, ctx)` | Anyone holding the coin (the user's wallet) | version; `amount = coin::value(&coin) >= max(1, min_bridge_out)`; `icp_principal` length 1 to 29 and not `[0x04]` (anonymous), as EVM; `icp_subaccount` exactly 32 bytes (all zero = default); `transfer_id` not in `returns` (else `EReturnUsed`) | `transfer_id = sha2_256(bcs(DOMAIN_RETURN) ++ bcs(chain_identifier) ++ bcs(gate address) ++ bcs(ctx.sender()) ++ bcs(client_nonce: u64))` (BCS of `vector<u8>` is length-prefixed; this mirrors EVM `computeTransferId`, S14). Inserts `returns[transfer_id] = sequence`; `coin::burn`; `bridge_out_count += 1` (sequence, first is 1); `burned_total += amount`; emits `BridgeOut`. **No pause check:** holders can always return. |
| `set_limits(gate, admin_cap, supply_cap, min_bridge_out)` | `AdminCap` holder | version; `supply_cap <= MAX_SUPPLY_CAP`; `min_bridge_out >= 1` | Sets both; emits `LimitsSet`. Setting `supply_cap` to 0 stops minting but never stops returns. |
| `migrate(gate, admin_cap)` | `AdminCap` holder | `gate.version < VERSION` | Sets `gate.version = VERSION`. The standard guard against old package versions, which stay callable after an upgrade (F S6). Run as a separate transaction after each upgrade. |

All five are `entry`. `sender` in `bridge_out` is `ctx.sender()`, never the gas owner. Deliberately absent: any function that returns or destroys the cap, pause flags, a mint rate window (the canister enforces windows; the gate is a backstop), any `burn` without a recipient. Rotation of `MintCap` or `AdminCap` is a plain object transfer (`store`).

**Events** (each `copy, drop`; a module can only emit types it defines, F S4; the system attaches sender, transaction digest, checkpoint time and the type tag)

| Event | Fields |
| --- | --- |
| `Minted` | `mint_id`, `recipient`, `amount`, `sequence` (= `mint_count`), `minted_total`, `supply` (after) |
| `BridgeOut` | `transfer_id`, `sender`, `amount`, `sequence`, `icp_principal`, `icp_subaccount`, `client_nonce`, `burned_total`, `supply` (after) |
| `LimitsSet` | `supply_cap`, `min_bridge_out` |

The observer reads `Minted` and `BridgeOut`. Filter by the full type name (package, module, struct); after an upgrade the type tag keeps the original package id (F S6 shows object types under the original id; the same for events is assumed, U).

## 3. Transactions and signing

**Keys (F S13, R roles).** Roles `treasury`, `gas`, `upgrade-custody` use path `[rumi-icusd-sui-bridge, v1, role, chain_identifier_32]` (Design 5.1). `schnorr_public_key` (`ed25519`) gives 32 bytes; Sui address = BLAKE2b-256(`0x00` ++ pubkey).
**Signing (F S7, S13; the IC mapping is an inference, not run).** Digest = BLAKE2b-256(`[0,0,0]` ++ BCS(`TransactionData`)); Ed25519 signs that 32-byte digest, so `sign_with_schnorr(ed25519, message = digest)`. Wire signature = `0x00` ++ sig(64) ++ pubkey(32), Base64. Cost per signature: `key_1` 26,153,846,153 cycles, `test_key_1` 10,000,000,000 (F S13).
**Two different hashes (correction to Design 6.3 and sources-native row 4).** The signing digest above is not the transaction digest that GraphQL `transaction(digest:)` takes. `TransactionData::digest()` is `default_hash`, and the `Signable` impl writes the type name and `::` before the BCS bytes; `TransactionData` is `BcsSignable` (F S7, `crypto.rs`, `transaction.rs`). So transaction digest = BLAKE2b-256(`"TransactionData::"` ++ BCS(`TransactionData`)), base58. Store both; look up by the typed one. Pin both with golden vectors from the Sui Rust SDK (U until run).

**Mint transaction (canister-built, sponsored).** One programmable transaction, one command: `rumi_icusd_gate::mint(Gate shared mutable, &MintCap owned, mint_id, recipient, amount)`. `TransactionData` = `sender` = `treasury` address, `GasData {payment: [one gas coin owned by the gas address], owner: gas address, price: reference gas price, budget}` (F S7 field names), `expiration` = `Epoch(current)` (below). Two signatures, sender and gas owner, over the same digest, in either order (F S8): derived about 52.3B cycles on `key_1`. Gas price from GraphQL; budget from `simulateTransaction` plus margin (F S11), within 2,000 MIST to 50 SUI (F S13). Gas cost of a mint with a table insert is unmeasured. R: the `gas` role is separate from `treasury` (Design 5.1); a single address (one signature, one fewer key) is the simpler alternative.
**Return transaction (user-built).** The wallet signs and pays gas (F S13: `sui:signAndExecuteTransaction`): optional `MergeCoins`, `SplitCoins` to the exact amount, then `bridge_out(Gate, coin, principal bytes, subaccount32, nonce)`. The canister signs nothing on Sui for a return: the burn is atomic, so `RC` and `BU` are zero (Design 2.3). The frontend can compute `transfer_id` before the transaction lands from (sender, nonce); the canister recomputes it and both share golden vectors. icUSD held as an address balance must be redeemed into a `Coin` in the same PTB (`coin::redeem_funds`, F S9; wallet behavior U).

**Equivocation hazard (F S8).** If two different transactions use the same owned-object version (the `MintCap` or the gas coin), validators split their reservations, neither reaches a quorum, and the object stays unusable until the next epoch (about 24 hours on mainnet). The documented fix is to wait, then rebuild from current object references. Sponsored transactions carry the same risk for the sponsor's gas coin; docs recommend dedicated gas pools. This is why the **same-attempt rule (Design 6.2) is a safety rule on Sui, not only an idempotency rule:**
1. Before signing, persist the exact unsigned bytes. They contain the sequencing token: (`MintCap` id, version), (gas coin id, version) and the expiration.
2. Record every signature and both digests. A lost or ambiguous sign reply means re-sign the same bytes, never rebuild.
3. Rebroadcast only the same signed bytes (`executeTransaction`). Any send error, timeout or "already executed" reply is "maybe sent": confirm by transaction-digest lookup and by `mints[mint_id]` (section 4). Never build a second transaction on the same `MintCap` or gas versions while the first is unresolved. One in-flight mint per `MintCap` follows; throughput is not a goal.
4. Replacement is allowed only when the attempt is proven dead: a finalized checkpoint in a later epoch lacks the digest and the `MintCap` and gas versions are unchanged. R: set `expiration = Epoch(current epoch)`: validators will not sign a transaction whose expiration epoch is below the current one (F S7 comment), which bounds an attempt's life to its epoch. Whether a quorum-certified copy can still execute after the epoch ends was not read here (U; a search summary of the Sui Lutris paper says locks last only until the epoch ends and transactions carry an epoch number).
5. If the versions changed and no recorded digest landed, a different transaction consumed them: halt the lane, re-sign nothing (Design 6.2 rule 4).
**Optional hardening (R, U):** make `mint` check `ctx.sender() == gate.minter` (an address set and rotated by `AdminCap`) instead of a `MintCap`, and pay gas from the gas address's balance (empty `payment`, `ValidDuring` expiration with `chain` and a `nonce`, max one epoch of range; F S9). The mint then has no owned input and nothing to equivocate; the consumed table still blocks a double mint. Not baseline: mainnet enablement of address-balance gas is not established (flag `enable_address_balance_gas_payments`), and SDK paths are newer.

## 4. Observer and finality

**Finality (F S10).** Final means included in a finalized checkpoint; "checkpoints do not fork, roll back, or reorganize"; do not credit on submission. The observer reads only checkpoints that the provider quorum serves; unlike EVM or Solana there is no tag or commitment level to choose.
**Loop per tick (R).**
1. Pick target checkpoint `T` deterministically (section 5) and require the providers to agree on `(sequenceNumber, digest, epoch)` of `checkpoint(sequenceNumber: T)`.
2. Page `events(filter: {type: <BridgeOut type>, afterCheckpoint: c, beforeCheckpoint: T+1})` (filter fields and strict bounds, F S11; page size 50). Repeat for `Minted`. Cursors are opaque and never compared across providers; compare the extracted facts.
3. Per `BridgeOut`: exact type tag; transaction effects status success and checkpoint at most `T`; `sequence` equals the last seen plus 1 (a gap holds the cursor and never skips); `transfer_id` recomputed by the canister from (domain, chain identifier, gate address, sender, nonce) equals the event's; amount in `[min, per-operation cap]`; principal parses and re-encodes. Dedup by `transfer_id` (and `(transaction digest, event index)`, the key S10 uses).
4. Read the `Gate` at checkpoint `T`: `bridge_out_count`, `burned_total`, `minted_total`, `mint_count`, `supply_cap` and the wrapped `Supply.value`. Require `bridge_out_count` equals the highest sequence seen (completeness). Read through `checkpoint(sequenceNumber: T) { query { object(...) } }` or `object(address, version)` with the version from the transaction's object changes (F S11; the consistent store keeps live state for "minutes or hours", so version reads are the fallback for old checkpoints, U).
5. Credit: `O_d -> PR_d` once, then the reused ICP release saga. The burn already happened, so there is no custody state.

**Evidence outlives events (R).** Indexers keep roughly 30 to 90 days of events (F S11), so a late event lookup can fail. Durable state does not expire: `mints[mint_id]` and `returns[transfer_id]` are dynamic fields of the table objects, read at a finalized version (query shape to pin against the live schema, U). Mint reconciliation therefore never needs history: an entry with the recorded recipient and amount means Delivered; absence is proof of non-delivery only once the attempt is dead (section 3 rule 4). The canister also keeps compact finalized evidence per operation (digest, checkpoint, sequence).

| Check | Sui meaning (Design 2.3) |
| --- | --- |
| C1 | ICP side only: `E_d >= L_d - U_d`, unchanged. |
| C2 | `Supply.value` at `T` is at most `O_d + M_inflight_d` (`RC = BU = 0`). The framework's own supply counter is the authority; event-embedded `supply` is a cheap in-feed check and must match it. |
| C3 | When quiescent and fully observed, `Supply.value == O_d`. |
| C4 | Available: `gate.minted_total` equals the canister's own cumulative minted amount, and `minted_total - burned_total == Supply.value`. A `Minted` event whose `mint_id` the canister never created, or a mismatch, halts that destination. A foreign mint needs a `MintCap` compromise or an upgrade (section 6). |

## 5. RPC path and the resulting blocker

- **F S13, S11:** JSON-RPC is disabled on Foundation mainnet full nodes (week of 2026-07-27) with full decommission planned mid-October 2026, so treat it as gone. No official ICP canister exists for Sui. gRPC (protobuf services) is the recommended backend path, but HTTP/2 or gRPC support in IC outcalls is not found in a primary source (U). **GraphQL (HTTPS POST, JSON) is the only path that plainly fits outcalls.** GraphQL RPC is generally available. Limits: transaction payload 175KB, other queries 5KB, timeouts 74 s (execute) and 40 s (read), nesting depth 20, page size 50; `serviceConfig` reports them.
- **Outcall consensus (F S13 outcall facts, R):** all replicas that send a request must produce the same response after the transform, and 13 may each POST. So (a) never query "latest": every read names a checkpoint number, version or object; (b) to find a head, ask for the latest checkpoint and have the transform round its number down to a multiple of a profile constant (cadence not sourced here, so the value is chosen after measurement), then use pinned queries; (c) the transform keeps only extracted facts (drop timestamps, cursors, extension fields); (d) set `max_response_bytes` small (a page of 50 events is a few KB; the cap is billed up front); (e) send `executeTransaction` as "maybe sent": the transform discards the body and the canister confirms by lookup, because a duplicate submission of the same signed bytes may answer differently from the first (U).
- **Provider count (F S12):** the profile needs `min_quorum` 2 and `min_independent_providers` 2. Foundation endpoints have "strict rate limits", are for development and testing and "not for production workloads"; every logical call becomes up to 13 upstream requests. Nine third-party providers list mainnet GraphQL (QuickNode, BlockPi, Ankr, ZAN, Inodra, NodeInfra, Chainstack, 01.ro, Triton One); only ZAN also lists testnet GraphQL. Testnet quorum could be Foundation plus ZAN for a demo. Whether any is keyless is not established; keys in canister state conflict with the keyless-only rule used for D1c (Design 12). **R:** same rule until a reviewed key-handling design exists.
- **Blocker `sui-rpc-provider-contract`:** at least two independent contracted GraphQL providers per network, keyless or reviewed key handling, rate quota for N-fold outcalls, and event and object retention longer than the reconcile horizon. Unmeasured: cycles per GraphQL call, checkpoint cadence, schema drift (reference pages exist for alpha and beta). The protocol cannot leave `disabled` for mainnet without it.

## 6. Authority and trust limits (D6 open)

| Authority | Holder (R roles; D6 decides custody) | Power | Lost or compromised |
| --- | --- | --- | --- |
| `UpgradeCap` | `upgrade-custody`, separate from `treasury` and `gas` | Policies: compatible (change any function body, so `mint` could lose every check), additive (add functions but not change existing ones, F S6), dependency-only, immutable; tighten-only (F S6). **My inference, not in the docs:** an additive upgrade can add a function in the same module that reads the gate's private `treasury` field and mints, so additive gives no protection. Only dependency-only or immutable close the code path. The version guard blocks stale package versions, not a malicious upgrade. | Lost: no fixes, no `migrate`. `make_immutable` consumes the cap (F S6): no fixes ever, so a gate bug stays. R: compatible during testnet rehearsal; decide dependency-only or immutable after review, before any real value (D6). |
| `TreasuryCap` | Wrapped in the gate; no function releases it | Reachable only through the gate's checks, absent an upgrade | Cannot be lost or taken as an object while the code is unchanged. |
| `MintCap` | `treasury` | Only `mint` caller; bounded by `supply_cap`, the consumed table and canister-side limits | Lost: minting stops, returns still work, holders unaffected. Compromised: bounded mint up to `supply_cap`, visible as unknown `mint_id` (C4). Rotate by transfer. |
| `AdminCap` | `upgrade-custody` | `set_limits` (can raise `supply_cap`) and `migrate` | Lost: limits frozen, and after an upgrade the new code cannot be activated (version guard). |
| `MetadataCap` | `upgrade-custody` | Edit name, description, icon (F S1) | Lost: metadata frozen. Deleted: irreversible. |
| Gas key | `gas` | Holds SUI only | Lost: the SUI on that address is stranded; fund a new gas address. |
| `DenyCap` | None exists | Never `make_regulated`; bind check enforces | n/a |
| Canister controllers | Set not established | Root of trust: every role key is canister-derived, so they reach all signing and the ICP escrow | Lost canister: all Sui caps and the escrow are stranded; users can still burn on Sui but nobody releases. |

Native limits: finality is only as good as Sui and the provider quorum; a foreign mint by upgrade is visible only after it happens (C4); Foundation endpoints are not for production. Mint authority is one cap, so whoever controls the canister, or the `UpgradeCap` under a mutable policy, controls supply.

## 7. Profile alignment (`profiles/sui.json`; read-only here, for the coordinator)

**Blockers to carry:** required by V7: `sui-gate-module-missing`, `sui-rpc-provider-contract`. Already present and still right: `sui-upgradecap-policy`, `sui-wallet-connector-missing`, `controller-model-undecided`; `testnet-synthetic-ledger-only` on testnet. **Suggested additions:** `sui-observer-lane-missing` (code: owned-version lane, return observer, nothing exists), `sui-outcall-consensus-unmeasured` (external: transform, rounding, cycles), `sui-signing-vectors-unverified` (code: both digests and the Schnorr mapping), `sui-wallet-display-unverified` (external: registry-only currency in Slush).
**Mismatches and wording to fix (none blocks groundwork; nothing edited):**
1. Design 6.3 (Sui row) and `sources-native.md` section 2 row 4 call the BLAKE2b intent digest the "transaction digest". Lookup needs the typed digest (section 3). The profile's `signed_payload` text is correct for signing but should note the second hash.
2. Design 6.3 lists the equivocation lock as unverified. It is now sourced (S8); update the row.
3. Design 7 (Sui row) says all Sui authorities are address-owned objects and that `TreasuryCap` is among them. In this design `TreasuryCap` is a field of the shared gate; `MintCap` and `AdminCap` are the owned capabilities. Also add that an additive upgrade does not protect the gate.
4. `amount_bounds.max_e8s = 18446744073709551614` is the conservative bound; `balance.move` itself allows `U64_MAX`. Consistent with the gate's `supply_cap` limit; no change needed, but the basis is "conservative", not "abort at U64_MAX".
5. `prerequisites` lacks two unavoidable steps: the `finalize_registration` transaction after publish, and SUI for the user's own gas on a return. Roles are only the `{role}` placeholder; list `treasury`, `gas`, `upgrade-custody`.
6. `rpc.endpoints` lists only the Foundation host and `min_independent_providers` is 2, which V9 accepts because `sui-rpc-provider-contract` is present. Domains (`mint_id`, `remint_id`, `return_id`, `escrow_label`) cover this design; no burn-id domain is needed because the burn is atomic.

## 8. Future tests and implementation steps (none is a groundwork gate)

**Move unit tests (`sui move test`):** init state (gate shared, cap wrapped, `supply_cap = 0`, caps delivered, `chain_identifier` copied); `mint` happy path, repeat `mint_id` aborts, zero amount, bad recipient, cap and overflow bounds, zero cap blocks; `bridge_out` principal bounds and `[0x04]`, subaccount length, minimum, duplicate (sender, nonce) aborts while another sender with the same nonce succeeds, `transfer_id` golden vector, sequence contiguity, `minted_total - burned_total == supply`; version guard (old `VERSION` aborts, `migrate` needs `AdminCap`); `set_limits` bound.
**Canister and host tests:** golden vectors for `TransactionData` BCS, signing digest, typed transaction digest and wire signature against Sui SDK crates; PocketIC Schnorr seams (`test_key_1`, Design 6.4) as capability only, with host-side signature verify; recorded GraphQL fixtures through the transform; reconcile matrix (landed, unchanged versions with epoch passed, different consumer halts, table entry adopts, null waits); observer matrix (sequence gap holds, foreign `mint_id` halts, wrong type tag, lookalike event from another package rejected, status failure); negative decoder per Design 4 (Sui has no repay path, so the check is that a `BridgeOut` lookalike is rejected).
**Ordered steps:**
1. Move package and unit tests, then an external Move review. **Medium.**
2. Instance skeleton by copy (ledger saga, accounting, admin, env, ICRC-21, stable layout; no `RC`/`BU` needed). **Medium.**
3. Roles, paths, mint-id domain, `transfer_id` vectors. **Small.**
4. BCS transaction builder, both digests, signer wrapper over the Ed25519 pattern, golden vectors. **Medium.**
5. GraphQL mirror, transform, per-provider quorum compare, retention probe. **Medium** (large if outcall consensus fails).
6. Publish and bind checks, claims; needs D6 and the provider contract. **Medium.**
7. Mint lane: persist-before-sign, expiration, reconcile, halts, gas pool. **Large.**
8. Return observer: sequence, dedup, counters, C2 to C4, release saga hook. **Large.**
9. Audit query, test matrix, testnet rehearsal on a synthetic ledger (D9), wallet display check. **Medium.**
10. Sui wallet connector in the frontend (BB-39) and any dependency-only or immutable policy step. **Large**, then **Small**.

## 9. Not grounded in source

- Whether IC Ed25519 signing is deterministic, and the `sign_with_schnorr(message = 32-byte digest)` mapping for Sui (inferred from two docs, never run).
- Whether a quorum-certified transaction can execute after its `Epoch` expiration; duplicate-submission replies of `executeTransaction`.
- Additive-upgrade access to private fields (Move visibility inference); event type tags under the original package id; the dynamic-field and checkpoint-nested query shapes and event field names (read from the beta reference only).
- Per-entry storage fee, gas cost of `mint` and `bridge_out`, cycles per GraphQL outcall and per signature at runtime, checkpoint cadence, consensus latency for shared-object transactions.
- Whether any contracted provider is keyless, serves history past the reconcile horizon, or accepts N-fold outcall volume; gRPC through IC outcalls.
- Wallet display of registry-only currencies; address-balance gas on mainnet; the publisher and every custody choice (D6).
