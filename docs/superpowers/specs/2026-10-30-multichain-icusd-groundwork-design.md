# Multichain icUSD groundwork: shared escrow accounting and destination-profile contract

Status: design only. Card BB-33 (worker W14), 2026-10-30, worktree branch `codex/base-icusd-bridge`.
Accounting corrected by the coordinator after Miles' review (sections 2.2 to 2.4 and the Solana row of 6.3).
Accepted by the coordinator with three wording edits (section 4 test labeled future implementation; section 11 decisions gate implementation, not groundwork; BB-39 marked done).
Nothing here deploys, enables, funds or signs anything. Every destination stays `disabled`.

Evidence labels: **F** = fact with a source (repo `file:line`, or a facts file under
`docs/coordination/multichain/`, whose own URLs and verified dates it carries). **R** = recommendation.
**D** = open decision for Rob and Miles (section 12). "Implementation missing" means no source exists.
PR #586 citations are on commit `b02584e4` (`git show b02584e4:<path>`); the PR is open and unmerged.
Facts files: `reuse-inventory.md` (BB-30), `sources-evm.md` (BB-31), `sources-native.md` (BB-32),
`xrpl-pr586-inventory.md` (BB-41).

## 0. What this document fixes

1. **Topology (R):** one bridge instance per destination network, the frontend choosing the actor (no Base API change); XRPL is the existing PR #586 lane, not a bridge instance.
2. **Accounting:** each destination has its own escrow account and its own disjoint buckets, outside the debt books.
3. **Amounts:** 1e-8 icUSD is exact on eight destinations (u64 or int63) and bounded by 15 significant digits on XRPL trust-line tokens; inexact amounts are rejected, never rounded.
4. **Repayment is never a bridge return, and a bridge return never repays debt.**
5. **Domains:** Base's three live signing domains are untouched; new instances get chain-qualified strings.
6. **Unknown outcomes** are held and reconciled as the same attempt. Attempt identity is the unsigned bytes plus a single-use sequencing token.
7. **Native chains need an explicit guard** (section 6.3): token primitives alone give neither `mintConsumed` nor `bridgeOut` semantics.
8. **Claims** (designed, deployed, authority-bound, mint-enabled, final, settled) are separate fields (section 8).
9. **Profile schema and validator** for BB-38 are specified in section 9.

## 1. Topology (A)

### 1.1 Facts that decide it

- The Base bridge is single-destination. It holds one `chain_id` and one `ContractBinding` (`state.rs:193-210`), two nonce lanes (`state.rs:384-390`, `ids.rs:138-142`), one observer cursor, one supply snapshot, one bucket set (`accounting.rs:1-20`) and an init allow-list of three chain ids with a key rule (`state.rs:235-247`). F
- Release indexes are keyed by the bare per-contract `sequence` (`state.rs:1262-1270`), and `flows/release.rs:65-68` halts on a reused sequence. A second contract in the same canister would halt the bridge. F (reuse-inventory.md section 2b and the coordinator collision note)
- Already separated: mint, re-mint and bridge-out ids hash chain id, contract and instance salt (`ids.rs:78-105`), and derivation paths include lane and chain id (`ids.rs:155`, test at `ids.rs:287-290`). F
- A separate canister instance has its own stable memory, configuration, binding, lanes and escrow account. It does not isolate aggregate reporting. F (collision note; Base design section 4)
- The backend EVM config knows only Monad and Conflux (`chains/evm/mod.rs:78-100`), and unknown chains are rejected by the supply code (`chains/supply.rs:118`). F

### 1.2 Recommendation (R)

| Option | Verdict | Reason |
| --- | --- | --- |
| One canister for several destinations | Reject | Five singleton namespaces collide (1.1). Re-keying them changes reviewed Base state code. |
| One instance per chain family | Reject | Same collisions inside a family (six EVM chains share the lane, binding, index and bucket singletons). |
| **One instance per destination network** | **Recommend** | No collisions without touching them. Mainnet and testnet are separate installs and separate escrow accounts, and key rules (mainnet `key_1`, test `test_key_1`; `state.rs:235-247`) stay per install. Cost: each instance pays its own signing and outcall cycles (not measured here). |
| Backend module | XRPL only | PR #586 already is one. Not a pattern for new chains (Base design section 4: wasm budget, minting-account blast radius). |

Instance naming follows the profile `network_id` (for example `base-mainnet`, `arbitrum-sepolia`).
Each instance has its own escrow account `(instance principal, subaccount)`. Escrow is never pooled across
destinations, and limits (`max_outstanding`, windows, supply cap) are per destination.

### 1.3 Reused versus new

| Piece | EVM family (6 chains) | Solana | Sui |
| --- | --- | --- | --- |
| ICP ledger saga (`ledger.rs`, `flows/ledger_ops.rs`) | Reuse as is | Reuse by copy | Reuse by copy |
| Buckets and coverage (`accounting.rs`) | Reuse; rename `*_base` fields | Reuse by copy, renamed | Reuse by copy, renamed |
| Admin, pauses, limits, ICRC-21, stable layout, env shim | Reuse | Reuse by copy | Reuse by copy |
| Ids and domains (`ids.rs`) | Reuse shape; domain slug parameter (5.1) | New (sha256 over length-prefixed fields) | New |
| Signer | `evm/signer.rs` (tECDSA) | New wrapper over the Ed25519 pattern in `chains/solana/ted25519.rs` (shared with XRPL) | New: Ed25519, flag `0x00`, BLAKE2b-256 intent digest (sources-native.md section 2 row 4) |
| Tx builder | `evm/tx.rs` (EIP-1559 only) | New; `chains/solana/tx.rs` is a legacy-tx base and notes SPL MintTo as planned | New (BCS `TransactionData`) |
| RPC | `evm/rpc.rs`; only the service mapping changes (`rpc.rs:217-223`) | SOL RPC canister; pattern in `chains/solana/sol_rpc.rs`; canister id conflict open | HTTPS outcalls to GraphQL; no official canister |
| Observer | `observer.rs`; head selection becomes per chain | New: return observer (the backend `deposit_watch.rs:114` polls balances, not burns) | New |
| On-chain guard | `BridgedIcUSD.sol` bytecode as is | Memo-bound transfer, or small program (D3) | Small Move module (D4, recommended) |
| Frontend | `baseBridge/*` wallet layer; add chain rows | No connector exists | No connector exists |

A shared Rust crate for the chain-agnostic ICP core is a follow-up (Base design section 13 defers it);
the first Solana and Sui instances copy. Whether the EVM Wasm is generalized in place or built as a
separate artifact is D1, because it touches reviewed Base source. The deployed Base canister changes only on an explicit upgrade.

### 1.4 XRPL alignment (PR #586 is the default seam)

- **R: no second XRPL icUSD token, no XRPL bridge instance, no new XRPL escrow.** The lane is `State.xrpl_icusd` in the backend (`xrpl_icusd/state.rs:20`), disabled by default (`types.rs:364-380`).
- Its escrow is `(backend, sha256("rumi:xrpl-icusd:escrow:v1"))` (`types.rs:33-41`), its issuer key path is `[rumi, xrpl-icusd, v1, network]` (`types.rs:59-66`). Both are kept; no other instance may reuse them (BB-41 Q7).
- **Debt-backed versus no-debt stays split** (BB-41 Q2). Debt-backed: `borrow_to_xrpl` (EscrowMint under a vault borrow) and repayment (`EscrowBurn`, then the debt change). No-debt: `issue_xrpl_from_canonical` (`ImportPull`, `issuance.rs:1-8`) and `CanonicalReturn` (escrow to the owner's canonical account, `issuance.rs:276-325`, `types.rs:400-411`). Groundwork scope is the no-debt pair. Units from the borrow path are debt-originated and are reported by origin, never as zero-debt backing.
- **Gap found (F, grep of the PR #586 endpoint list, `main.rs:9805-9880`):** the return side is shaped around vault repayment intents (`create_intent`, `repay.rs:137`, binds a vault, payer and never-reused destination tag). Unattributed XRPL payments are refunded on XRPL with no debt credit (`apply_verified_return`, `repay.rs:431`), and a held repayment can go back to the owner's canonical account (`ReceiveCanonicalHeld`, `repay.rs` near 1093 and 1161). I found no dedicated no-vault "return XRPL icUSD to my ICP account" operation. **Implementation missing.** BB-37 specifies it as a follow-up after #586 merges, not an edit to #586.
- Bucket mapping (`xrpl_icusd/state.rs:119-135`): `D` = PM, `O` = O, `R` = PR, `H` = H and RF, `ic_fee_reserve_e8s` = fee reserve. `D` carries `in_flight` (signed issuance that may already be on XRPL), the analogue of `M_inflight`.
- **D2 (open): trust-line issued currency (PR #586) versus Multi-Purpose Token (BB-32 recommendation).** IOU: already implemented and exact-parsed, but exact only to 15 significant digits (section 3) and holders need a trust line. MPT: exact int63 at `AssetScale` 8, mainnet amendment enabled, but flags are fixed at creation, holders must `MPTokenAuthorize`, wallet support varies, and it replaces the implemented lane. R: keep the IOU lane as the groundwork seam and record MPT as the alternative.

### 1.5 Destination addressing (a decision of this design)

Two options exist: a `destination` argument on one shared interface, or one canister per destination with the frontend choosing the actor.
**R: per-destination instances, frontend selects the actor.** The instance is found from the profile `network_id` (9.3 `instance_ref`).
No Base API change follows: the reviewed Base canister keeps its Candid (`bridge_to_base`, `notify_base_bridge_out`, `get_base_addresses`, `get_supply_audit`, `lib.rs:93, 143, 163`) and its state unchanged.
A shared `destination` argument would change Base Candid and re-key the singleton state (1.1), so it is rejected for groundwork. Decided (D1a): new EVM instances run the same generalized Wasm and therefore initially keep the legacy Base-named Candid methods and fields (one Wasm exports one interface); the naming is documented and the frontend routes by the per-network actor. Generic aliases are optional future API cleanup, not a groundwork gate. `baseBridge/bridgeActor.ts` (`getBridgeCanisterId`, one canister) gains a lookup keyed by `network_id` in later frontend work, not a change to the Base actor.

## 2. Accounting (B)

### 2.1 Disjoint from the debt books

The backend invariant is `sum(chain_supplies) == total_debt + reserve backing + pending chain burn`
(`chains/supply.rs:96-100` `chain_backing_rhs_e8s`, `107-160` `apply_supply_delta`, `167-178` `check_invariant`;
the module header's `== total_debt` is stale and the bodies govern). F
Escrow-backed destinations are never `ChainId`s in `chain_supplies`; `apply_supply_delta` returns `UnknownChain`
for an unregistered chain (`supply.rs:118`). The bridge never calls the backend (Base design 7.3), and PR #586 does not
touch `MultiChainState`. Escrowed icUSD is ordinary ICP ledger supply. No destination supply is ever added to `total_icusd`.

### 2.2 Shared vocabulary (per destination `d`, e8s)

| Symbol | Meaning | Base field | PR #586 |
| --- | --- | --- | --- |
| `E_d` | Ledger balance of destination `d`'s own escrow account | escrow balance | escrow balance |
| `O_d` | Destination outstanding: finalized delivered mints minus finalized processed returns | `outstanding_base` | `O` |
| `PM_d` | Escrowed, not yet delivered (includes deposit fee; held re-mints) | `pending_mint` | `D` |
| `RC_d` | ReturnCustodied: finalized valid returns whose destination tokens sit in the canister-owned return account, not yet burned (custodied-return families only) | n/a (Base burns in `bridgeOut`) | n/a (a payment to the issuer redeems) |
| `BU_d` | BurnUnknown: custodied returns whose burn was signed but is not yet final or reconciled | n/a | n/a |
| `PR_d` | PendingRelease, payable: finalized valid returns whose destination tokens are already destroyed (burned atomically by the return, or by a finalized burn), not yet released on ICP | `pending_release` | `R` |
| `RF_d`, `H_d` | Refunds in progress; held claims | `pending_refund`, `held` | `H` |
| `F_d` | Fees earned, not swept | `fees` | fee reserve |
| `U_d` | Outflow of escrow transfers whose outcome is unknown | `unknown_outflow` | in `ic_fee_reserve` terms |

`L_d = O_d + PM_d + RC_d + BU_d + PR_d + RF_d + H_d + F_d`; each e8 of liability is in exactly one bucket. Origin tag per claim: `imported` (existing canonical icUSD pulled
with ICRC-2) or `debt_originated` (PR #586 borrow path only). Only `imported` counts toward the "no new debt" claim.

### 2.3 Invariants (per destination, enforced inside its own instance)

Correction (Miles' review, 2026-10-30): an earlier draft added the Solana return account to C2 without taking the same
units out of `PR_d`, so a valid return awaiting its burn was counted twice. Return states are now disjoint.

- **C1:** `E_d >= L_d - U_d`. Violation halts that destination only (`accounting.rs:237-259`).
- **C2:** `supply_d(final head) <= O_d + M_inflight_d + RC_d + BU_d`, where `M_inflight_d <= PM_d`. Families whose return
  destroys the tokens in the same transaction (EVM `bridgeOut`, the Sui gate module, an XRPL payment to the issuer) always
  have `RC_d = BU_d = 0`.
- **Derived bound:** `supply_d + (PM_d - M_inflight_d) + PR_d + RF_d + H_d + F_d <= L_d <= E_d + U_d`. Custodied and
  burn-unknown return units are counted once, inside `supply_d`, and never also in `PR_d`.
- **What the bound means:** destination supply plus every other open claim never exceeds the escrow balance ADJUSTED for
  unknown outflows. `E_d + U_d` is adjusted coverage, not proof that every unit is still physically locked: an escrow
  transfer whose outcome is unknown may already have left. The bound is exact again once each `U_d` attempt is reconciled
  as the same attempt (6.2).
- **Custodied return lifecycle (Solana memo option; Sui no-module fallback):** a finalized valid transfer into the
  canister-owned return account moves the amount from `O_d` to `RC_d`; signing the burn moves it to `BU_d`; a finalized
  burn moves it to `PR_d` (supply drops by the same amount). A burn proven not to have landed returns it to `RC_d` (the
  tokens are still in custody); a different transaction consuming the same nonce halts the lane (6.2 rule 4). **No ICP
  payout from escrow while a unit is in `RC_d` or `BU_d`.**
- **C3:** when quiescent (no in-flight mints, no custodied or burn-unknown returns) and fully observed, `supply_d == O_d`.
- **C4 (cumulative mints):** needs an on-chain monotone mint counter. EVM has `totalMinted` (`BridgedIcUSD.sol` mint, line 227). Solana without a program and Sui without the Move module have none (6.3), so a foreign mint that is burned again before the next check is invisible there. Recorded as a known gap, not hidden.

### 2.4 Aggregate reporting lives outside any one canister

Index sets, so no escrow account or destination supply is counted twice:

- `D` = bridge-instance destination networks whose escrow account is on the production icUSD ledger (mainnet instances;
  a testnet instance enters `D` only if, under D9, it escrows real icUSD; test-ledger instances never do).
- `X` = the XRPL lane of PR #586 (a backend module, one escrow account). `X` is NOT a member of `D`.

```
total_icusd         = icp_ledger_total_supply + sum(debt_backed_chain_supplies)    (escrow-backed tokens add nothing)
escrow_locked       = sum over d in D of E_d  +  E_X
circulating_on_icp  = icp_ledger_total_supply - escrow_locked
circulating_dest_d  = supply_d - custody_d   for d in D
circulating_xrpl    = issued_outstanding_X   (held off the issuer; a payment to the issuer redeems, so no custody term)
circulating_total   = circulating_on_icp + sum over d in D of circulating_dest_d + circulating_xrpl
                      + sum(debt_backed_chain_supplies)
```

`custody_d` is the balance of `d`'s canister-owned return account read at the same final head as `supply_d` (zero for
families whose return burns atomically). Custodied unburned return inventory is therefore excluded from circulation
exactly once, while it stays backed by escrow through `RC_d` or `BU_d` until its burn is final. `E_X` and
`issued_outstanding_X` appear only in their own terms.
On Solana the measured return-account balance can also hold strays (transfers that failed the exact-shape check,
never credited, still part of `O_d`, awaiting refund to the verified sender). They are reported as `stray_d`, excluded from
circulation while held, and checked by `custody_measured == RC_d + BU_d + stray_d` (BB-35, `chain-solana.md`); an
unexplained excess halts credits.

Each instance exposes a per-destination audit query (the Base `get_supply_audit` shape: `E`, every bucket, `U`, surplus, last
final supply, C-status). An aggregator (analytics or a read-only script, BB-40) sums them and must re-read the escrow balance
and destination supply itself rather than trust a self-report. Open follow-up (Base design sections 2 and 13):
`rumi_analytics` still reports `icrc1_total_supply` as circulating (`rumi_analytics/src/timers.rs:188-199`) and must subtract escrow.

## 3. Eight-decimal value (C)

All amounts cross ICP as u64 e8s (`BridgedIcUSD.sol` natspec: "the ICP side accounts in u64 e8s").

| Chain | On-chain amount | 1e-8 exact? | Largest exact amount | Bridge must reject |
| --- | --- | --- | --- | --- |
| Ethereum, Base, Arbitrum, Robinhood, HyperEVM, Monad | `uint256`, contract caps at `u64::MAX` (`BridgedIcUSD.sol:202-230, 243-262`) | Yes, `decimals() = 8` | 18,446,744,073,709,551,615 e8s = 184,467,440,737.09551615 icUSD | 0; above `u64::MAX`; above `supplyCap`; non-integer e8s in any text input; mint to zero or the contract |
| Solana | `u64` base units, `decimals: u8 = 8` fixed at mint init | Yes | same as EVM | Mint with `decimals != 8`; any mint extension that changes amounts (`ScaledUiAmount`, `InterestBearingConfig`, `TransferFeeConfig`); non-`Checked` instructions; 0 |
| Sui | `u64` `Balance`, `decimals: u8` display only | Yes | 184,467,440,737.09551614 icUSD (`mint_balance` aborts at `U64_MAX`) | Mint that would make supply reach `U64_MAX`; 0; any supply cap above the bound |
| XRPL trust-line token (PR #586) | Canonical amount: 16-digit mantissa in `[1e15,1e16)`, exponent `[-96,80]` (`issued_amount.rs:1-9, 22-26`) | Representable; exact only within 15 significant digits | Per request: 999,999,999,999,999 e8s = 9,999,999.99999999 icUSD | More than 8 decimals; more than 15 significant digits; non-positive; not a whole number of e8s; `delivered_amount` replaced by nominal `Amount`; same code with another issuer |
| XRPL MPT (alternative, D2) | `int63`, `AssetScale` 8 | Yes | 92,233,720,368.54775807 icUSD | Above `MaximumAmount`; 0 |

XRPL details (F): `validate_normal_request` requires positive, at most 8 decimals, at most 15 significant digits and a
whole e8s value that fits `u64` (`issued_amount.rs:241-262`). `try_from_e8s` fails with `TooPrecise` when an amount needs
more than 16 digits (`issued_amount.rs:305-309`); it never rounds. Receipts keep any valid mantissa and exponent, and totals
are tracked in 1e-40 units, finer values being refused and held (`types.rs:117`). The docs state 15-digit precision;
the on-chain mantissa width beyond that was not verified (sources-native.md section 3 row 3).
**R:** the groundwork profile requires `outstanding_cap_e8s` and `per_operation_cap_e8s` (`types.rs:342-380`, default 0)
to be at most 999,999,999,999,999 e8s, so a balance cannot silently need more than 15 digits. Whether a holder trust-line
limit can enforce the same bound is not established in the facts files (D7). Any inexact amount is rejected or held; nothing rounds.

## 4. Repayment versus bridge burns (D)

Rule: **a repayment is never a bridge return, and a bridge return never repays debt.** Enforced by construction:

- **EVM (F):** `IcUSD.sol:64-67` `burn(amount, target_vault_id)` emits `Burn(vault_id, burner, amount)`, which the backend treats as repayment. `BridgedIcUSD.sol` has no `burn`, no `burnFrom`, and `bridgeOut` is the only burn with a different event (Base design section 6). `bind_contract` pins `VERSION()`, decimals 8, zero supply, zero bridge-outs, zero `totalMinted` and the role holders at a final block (`observer.rs:261-297`), so a debt-backed `IcUSD` address cannot be bound.
- **Monad (F/R):** both a debt-backed `IcUSD` and an escrow-backed token may exist on one chain (`sources-evm.md` section 6 row 7). Profile blocker: the two must have distinct on-chain name and symbol or distinct user-facing labels (D8). The backend decodes only its registered contract; the bridge decodes only its bound contract.
- **ICP side (F):** a bridge release is an escrow-to-ICP-account transfer and never an escrow-to-minting-account transfer. A release to the ledger minting account is a burn, so Base holds it and re-mints to the sender (Base design 11a). The bridge never calls backend repay paths.
- **XRPL (F):** only the repayment flow does `EscrowBurn` then the debt change, and only for a payment tied to an open vault intent. Any other payment to the issuer is refunded on XRPL with no debt credit (`repay.rs:431`). `CanonicalReturn` is escrow to owner, not a burn.
- **Solana and Sui (R):** the backend's Solana rail plans a debt-backed SPL icUSD (`chains/solana/tx.rs:6`, token address in runtime state `chains/solana/config.rs:3-5`). The escrow mint must be a different mint and must never be written into that config. The Sui and Solana guards (6.3) have no vault argument.
- **Future implementation acceptance test per family** (specified by BB-34 to BB-37, written when the family is implemented; not a groundwork gate): one negative decoder test showing a repay-shaped event is not accepted by the bridge, and a bridge-shaped event is not accepted by the repay path. No wider sweep.

## 5. Domain separation (E)

### 5.1 Strings and paths

Base live strings (unchanged, F): mint `rumi.icusd.base.mint.v1`, re-mint `rumi.icusd.base.remint.v1`, bridge-out
`rumi.icusd.base.bridge-out.v1` (`ids.rs:21-25`), escrow label `rumi-icusd-base-bridge:escrow:v1`, key root `rumi-icusd-base-bridge` (`ids.rs:28-31`),
contract `VERSION = "rumi-base-icusd-bridge-v1"`. Changing any of them would orphan consumed mint ids and addresses.

| Identifier | New EVM instance (R) | Solana (R) | Sui (R) | XRPL (PR #586, unchanged) |
| --- | --- | --- | --- | --- |
| Mint id domain | `rumi.icusd.<slug>.mint.v1`, slug in `eth, arb, rhood, hyper, monad` | `rumi.icusd.sol.mint.v1` | `rumi.icusd.sui.mint.v1` | operation id and 32-byte request nonce (`issuance.rs:17`) |
| Re-mint id domain | `rumi.icusd.<slug>.remint.v1` | `rumi.icusd.sol.remint.v1` | `rumi.icusd.sui.remint.v1` | n/a (refund operations) |
| Return id | Contract-computed `transferId` (`BridgedIcUSD.sol:269-270`), domain inside the contract (D5) | Dedup key `sha256(domain, genesis hash, tx signature)`, domain `rumi.icusd.sol.return.v1` | Move `transfer_id` over (domain `rumi.icusd.sui.return.v1`, chain identifier, sender, client nonce) | dedup `(network, issuer, tx hash)` plus never-reused destination tag |
| Preimage identity | chain id, contract, instance salt | cluster genesis hash, mint, instance salt | chain identifier, package, instance salt | network, issuer |
| Escrow subaccount | `sha256("rumi-icusd-<slug>-bridge:escrow:v1")` | `sha256("rumi-icusd-sol-bridge:escrow:v1")` | `sha256("rumi-icusd-sui-bridge:escrow:v1")` | `sha256("rumi:xrpl-icusd:escrow:v1")` |
| Signing path | `[rumi-icusd-<slug>-bridge, v1, lane, chain_id_be8]` | `[rumi-icusd-sol-bridge, v1, role, genesis_hash_32]` | `[rumi-icusd-sui-bridge, v1, role, chain_identifier_32]` | `[rumi, xrpl-icusd, v1, network]` |
| Ledger memo prefix | Base prefixes (`RBRG-DEP/RFD/FEE`, `ids.rs:36-38`) | `RSOL-*` | `RSUI-*` | per PR #586 |
| Sequencing token | per-instance lane nonce | durable nonce account per in-flight op | owned-object versions | issuer `Sequence`, single-flight |

Why chain-qualified strings although chain id is in the preimage: a defect that drops the chain element still cannot cross
chains, and audits read the chain off the string. A parameterized slug of `base` reproduces the live Base strings exactly (test requirement for D1).
Roles are separate keys: EVM `minter` and `admin` (`ids.rs:155`); Solana `mint-authority`, `fee-payer`, `return-owner`;
Sui `treasury`, `gas`, `upgrade-custody`. Network identity is a path element, so a testnet key cannot equal a mainnet key
even if canister separation failed. (Per-canister derivation binding is stated in Base design sections 3 and 4; I did not re-verify it against the IC specification.)

### 5.2 Sequences and nonces

`bridgeOutCount` is per contract (`BridgedIcUSD.sol:97, 262`). Any index that spans instances is keyed
`(destination network_id, contract or mint, sequence)`. Per-instance indexes (`state.rs:1262-1270`) are safe only because one instance binds one contract
once; they must never be merged into one map. An instance never rebinds a second contract; replacement means a new instance
and a new `instance_salt` (`sha256(label || canister id || install time)`, Base design 7.1).

### 5.3 Legacy Base records stay intact

Existing Base intent records, storage keys, readers and signing domains are not renamed, moved or dropped. That covers the frontend
keys under `rumi:base-bridge:*` (`evmWallet.ts:304-305`, `intents.ts:19-23` prefix `rumi:base-bridge:v1:`), the canister's stable-memory layout
(`state.rs:43-58`, append-only) and the three live domains above. New destinations use their own key namespace that includes `network_id`
and never share the `rumi:base-bridge:*` prefix. If a migration is ever needed, it binds each legacy record explicitly to `base-mainnet` or
`base-sepolia` (by chain id) and keeps the old key readable; it never relies on a missing network field meaning Base.

## 6. Finality, unknown outcomes and native guards (F)

### 6.1 What "final" means

| Chain | Final for settlement | Source |
| --- | --- | --- |
| Base | `finalized` tag, lowest agreeing quorum value, then block hash by quorum (about 20 min) | `rpc.rs:745`; Base design 7.7 |
| Ethereum | `finalized` (about 13 min) | sources-evm.md section 1 row 3 |
| Arbitrum | `finalized` = batch in an Ethereum finalized block (about 13 min after posting) | sources-evm.md section 3 row 3 |
| Robinhood | Ethereum finality of the posted batch; `finalized` tag support UNVERIFIED, probe before relying | sources-evm.md section 4 row 3, section 8 Q1 |
| HyperEVM | HyperBFT commit; tag and reorg semantics UNVERIFIED. **Blocker:** default RPC serves `eth_call` at `latest` only, but the observer needs numbered finalized state (`observer.rs:261-297, 612-648, 905-922`) | sources-evm.md addendum |
| Monad | **Verified**, not just `finalized`: head = finalized minus the execution delay, which Monad's developer summary gives as three blocks (verified 2026-10-30; the block-states page gives only the formula). Receipts, logs and supply read at `finalized` are ordering-level evidence only. **Blocker:** observer head-selection change | sources-evm.md addendum; docs.monad.xyz block-states |
| Solana | `finalized` commitment (under Alpenglow `confirmed` equals `finalized`) | sources-native.md section 1 row 5 |
| Sui | Finalized checkpoint containing the transaction digest; reads pinned to a checkpoint | sources-native.md section 2 row 5 |
| XRPL | Validated ledger with `tesSUCCESS`, `ledger_index: validated`, `delivered_amount`, read quorum 2 | `issuer_rpc.rs:62, 115`; `xrp_rpc.rs:435` (PR #586) |

Every EVM destination must serve numbered, finalized, historical contract state from the quorum of independent providers; reads never
fall back to `latest` (sources-evm.md addendum).

### 6.2 One rule for unknown outcomes

Anything unknown (reply lost, providers disagree, not yet final, ambiguous signing reply) is **held and reconciled as the same
attempt**. **Attempt identity = the unsigned transaction bytes plus a single-use sequencing token** (EVM nonce, Solana durable-nonce
value, XRPL `Sequence` with `LastLedgerSequence`, Sui owned-object versions). Signatures and hashes are evidence, and every one ever produced is recorded and checked.

1. Persist the unsigned bytes and the token before the first signing call.
2. Rebroadcast the same signed bytes; broadcast errors and "already known" lead to receipt or status checks, never to a new transaction (Base design 7.6).
3. A replacement at the **same token** is allowed only when no recorded attempt has landed and the token is unconsumed.
4. A **new token** (new nonce or `Sequence`) only after the old one is proven consumed or expired. A consumed token with no matching landed attempt halts that lane; nothing is re-signed (Base design 11a).
5. After `SYS_UNKNOWN` or `CANISTER_ERROR` a signature may already exist (sources-native.md ICP-side table), so a lost sign reply means "re-sign the same unsigned bytes", not "start over". The facts files do not say whether IC Ed25519 signing is deterministic, so Solana and XRPL key reconciliation on the message and token, not on one signature.
6. ICP ledger calls: resend the identical `(created_at_time, memo)` tuple; only `Ok` or `Duplicate` proves success; ICRC-3 search after `TooOld` (Base design 7.4).

### 6.3 Native guards: return binding and idempotent mints (points a and b)

SPL `MintToChecked`, a plain SPL `Burn`, and Sui `TreasuryCap` mint or burn bind no ICP account and keep no consumed-id record.
The EVM contract does both (`mintConsumed`, `bridgeOutUsed`, and `BridgeOut` carrying the principal bytes, subaccount and a sequence).

| Chain | (a) Return bound to an ICP account and a durable unique id | (b) Mint retry cannot mint twice | Smallest guard | Status |
| --- | --- | --- | --- | --- |
| EVM | `BridgeOut(transferId, from, amount, sequence, principal, subaccount)`; unique `transferId` | `mintConsumed[mintId]`, plus lane nonce | The deployed contract | Exists (Base) |
| Solana (R, D3) | Holder signs one transaction: `TransferChecked` (8 decimals) to a canister-owned return account plus an SPL Memo `rumi1\|<principal>\|<subaccount>\|<client_nonce>`. The observer credits a return only when the finalized transaction has exactly the allowed shape: one top-level `TransferChecked` of the escrow mint (decimals 8) into the canister-owned return account, authorized by the source token account's owner (or an authority the profile explicitly allows), plus one Memo instruction in the same transaction naming the ICP account. It verifies mint, source owner or authority, destination account and decimals from the instruction itself and cross-checks pre and post token balances; a net balance delta alone is never credited. Other shapes (unchecked `Transfer`, several transfers, inner-instruction transfers, missing or mismatched memo) are refunded to the verified sender. Credited tokens enter `RC_d` (2.3) and pay out only after their burn is final. Return id = tx signature. No valid memo: refund to the verified sender, never credited to an ICP account. | Mint tx = `[AdvanceNonce, CreateIdempotent ATA, MintToChecked(8), Memo(mint_id)]` on a per-op durable nonce account. Persist unsigned message and nonce binding, then sign, then send. The nonce advances when it lands, so the same bytes or a same-nonce replacement can land only once. Reconcile: read the nonce account at `finalized`; look up each recorded signature; else find the memo `mint_id` via signature history. A different transaction that consumed the nonce halts the lane. | No program: Memo plus durable nonce. Stronger option: a small program with a `consumed[mint_id]` PDA and a `bridge_out` that burns and emits the ICP account (adds a program upgrade authority). | **Implementation missing** (observer, nonce lane, memo parser). No on-chain consumed record, so C4 is unavailable without the program (2.3). RPC history retention bounds how late a memo lookup works (unmeasured). |
| Sui (R, D4) | A plain coin transfer carries no ICP account (the Coin and wallet docs read describe no memo). A Move module `bridge_out(coin, icp_principal, icp_subaccount, client_nonce)` burns the coin, stores `transfer_id`, increments a counter and emits `BridgeOut`. Fallback with no module: a per-intent derived deposit address (extra gas and sponsorship work). | The module wraps `TreasuryCap` and aborts `mint(mint_id, ...)` if `mint_id` was consumed, with `minted_total`. Attempt identity is the transaction digest (hash of the unsigned bytes, sources-native.md section 2 row 4). Retry the same bytes; look up by digest. The module is the on-chain backstop. | Move module `rumi_icusd_gate` (consumed table, counters, `BridgeOut` event). | **Implementation missing.** Unverified (not in facts files): signing two different transactions on the same owned-object version may lock the objects until epoch end, so rule 6.2 (same bytes) also matters here. |
| XRPL (PR #586) | Payment of the token to the issuer with a canister-prepared, never-reused destination tag; dedup `(network, issuer, tx hash)`; only `delivered_amount` credited (`repay.rs:431`) | Issuer sequencer is single-flight; unsigned bytes, signature and signed blob persisted before submit; resubmit the same blob (`MAX_SUBMITS 8`, resubmit after 20 s); a signed ambiguity blocks claim resolution (`issuance.rs:163` `plan_resolution`) | Existing lane | Exists for issuance. The no-vault return intent is **implementation missing** (1.4). |

### 6.4 Testing Ed25519 (three separate claims)

1. **dfx local replica:** has no Ed25519 key ("There is no local replica Ed25519 key", sources-native.md ICP-side table). That quote concerns this replica only.
2. **PocketIC:** provisions threshold Schnorr keys when booted with an II subnet plus an application subnet. Existing seams in this worktree: `src/rumi_protocol_backend/tests/solana_m2_pic.rs` lines 17-32 (`schnorr_public_key`, `sign_with_schnorr`, `test_key_1`), `xrp_native_e2e_pic.rs` lines 23-29 and 269 (`key_1`; degrades to a gated subset when a build cannot provision it), and `solana_m2_sign_pic.rs` (best effort; it may skip the signature assertion). The EVM analogue is the Base bridge's PocketIC threshold-ECDSA tests (Base design section 11).
3. **Execution proof:** none is claimed here. These are test seams, not evidence that signing tests passed; nobody reran them for this card.

"Mainnet only" is therefore not a local-test gate. Real-network signing is a separate activation step (section 8).

## 7. Authority and trust (G)

There are **no operator hot keys.** Signing is ICP threshold only: ECDSA secp256k1 for EVM, Schnorr Ed25519 for Solana, Sui and XRPL (`key_1`
mainnet, `test_key_1` test). Operators fund gas and fee-payer balances; funding is not key custody.

| Piece | Who can administer or upgrade | Powers and limits |
| --- | --- | --- |
| Bridge instance (ICP) | Canister controllers (set not established in the files read; BB-41 "unclear"). Config admins | Admins pause, set limits and bind once; they cannot move escrow (`admin.rs:1-6`). Controllers can upgrade code, which reaches escrow and every signature. **The controller set is the root of trust** (D6). |
| EVM `BridgedIcUSD` | `DEFAULT_ADMIN_ROLE` = canister admin address (two-step delayed transfer), `MINTER_ROLE` = canister minter address, optional pause-only guardian; deployer holds nothing | Admin can grant `MINTER_ROLE`, so it is a full mint path and must only be canister-held (`BridgedIcUSD.sol` natspec). |
| Solana mint | Mint authority = canister Ed25519 key (or a program PDA under D3). Freeze authority unset. Token-2022 metadata update authority set to none or the canister. A gate program, if any, adds an upgrade authority | Mint authority is one reassignable pubkey (`SetAuthority`). Whoever controls the canister can mint. Losing the canister strands the authority. Whether a freeze authority can be added after creation is unconfirmed (D3 input). |
| Sui | `TreasuryCap` (wrapped in the gate), `UpgradeCap` custody (separate role key; policy decision), `MetadataCap`. No `DenyCap`: never call `make_regulated` (irreversible) | All are address-owned objects; a lost key strands them with no recovery path. `make_immutable` consumes the `UpgradeCap` and removes the fix path. Foundation endpoints are rate-limited and not for production. |
| XRPL issuer (PR #586) | Threshold Ed25519 key at the issuer path. No SignerList, regular key or blackholing in the diff. Controller-only admin endpoints (`main.rs` near 9786-9792). Flags: DefaultRipple, RequireDest, par TransferRate; clawback or NoFreeze only through an explicit `IssuerPolicy` (`types.rs:273-318`) | Clawback and NoFreeze are irreversible. An issuing account cannot be blackholed. Losing the sole key makes it permanently inaccessible. |

Native-chain limits that cannot be designed away: the controller set can always reach signing; finality is only as good as the
chain (Section 6.1) and the provider quorum; chain-level freeze, clawback or upgrade powers exist wherever the standard or the
program allows them; HTTPS-outcall RPC is trust in public providers bounded by quorum; wallet support differs per chain (Xaman only for XRPL, no Phantom or Sui connector in the repo).

## 8. Distinct claims (H)

A board, profile or status line may state a claim only with its own evidence. They are separate fields (`claims` in section 9).

| Claim | Meaning | Evidence required |
| --- | --- | --- |
| `designed` | A profile and design exist | This document and the profile file |
| `deployment` | Token, contract, package or issuer exists on the network and passed the bind checks at a final block (pristine supply and counters) | Final-block reads by independent providers. Values: `none`, `deployed_unbound`, `bound` |
| `mint_authority` | Only the canister can mint (roles or authorities verified; deployer holds none) | Authority reads at a final block. Values: `none`, `canister_only`, `other` |
| `mint_enabled` | Deposits unpaused, gas funded, limits set (a fresh Base canister starts with deposits paused, `state.rs` default pauses) | Instance config |
| `finality_checked` | The chain's final-state rule was exercised against the real network | Recorded probe |
| `settlement_proven` | A full round trip with C1 to C3 holding | Test or run evidence |

Groundwork sets `deployment = none`, `mint_authority = none`, and the three booleans `false`. Base's live status is not established here (reuse-inventory.md section 7).

**Disabled versus paused are different states and must not share a label.**

| State | Meaning | Source |
| --- | --- | --- |
| DISABLED destination | Profile `status = disabled`. No signing request is made in either direction: no mint or admin transaction on the chain, no release transfer, no threshold-key call for it. Groundwork installs nothing. | Section 9 `status` |
| PAUSED releases on an active destination | Existing Base behavior: `releases_paused` leaves the excess in `PR`, nothing is lost, the delivery is held with disclosure, and reconciliation keeps running; pauses never erase liabilities or forget signed attempts. | Base design 7.5 step 7 and 7.8 |

Groundwork adds no Base fix and no test gate for the paused case; the state is only named so profiles and boards do not conflate it with DISABLED.

## 9. Destination-profile schema and validator (I)

### 9.1 Fact wrapper

`Fact<T> = { value: T, source_url: string, verified: "YYYY-MM-DD", source_kind: primary | secondary | repo | derived }`.
`Decision<T> = { value: T, rationale: string, status: "recommendation" | "planning_decision" }`; `planning_decision` records a settled planning assumption (section 12) and never activates anything. Identity fields must be `primary`; `secondary` is never accepted for them.

### 9.2 Profile (one JSON file per chain, nine files)

| Field | Type | Allowed values | Meaning | Example (source) |
| --- | --- | --- | --- | --- |
| `schema_version` | u32 | `1` | Contract version | `1` |
| `id` | string | `^[a-z][a-z0-9-]{1,23}$`, unique | Chain slug | `monad` |
| `display_name` | string | free text | Label | `Monad` |
| `family` | enum | `evm`, `solana`, `sui`, `xrpl` | Chain family | `evm` |
| `topology` | object | `kind` in `bridge_instance`, `backend_module`; `wasm_family`; `existing_work` | Where the logic lives | XRPL: `backend_module`, `pr586` (BB-41) |
| `representation.standard` | enum | `erc20`, `spl_token_2022`, `spl_token`, `sui_currency`, `xrpl_iou`, `xrpl_mpt` | Token standard | `spl_token_2022` (sources-native.md section 1) |
| `representation.backing` | enum | `escrow_locked_icusd` only | Backing | constant |
| `representation.debt_creating` | bool | `false` only | No new debt | constant |
| `representation.decimals` | u8 | `8` only | Decimals | `8` |
| `representation.amount_bounds` | object | `min_e8s >= 1`; `max_e8s`; `encoding` in `u64`, `int63`, `xrpl_decimal_15` | Exact range (section 3) | `u64`, 18446744073709551615 |
| `signing.scheme` | enum | `ecdsa_secp256k1`, `schnorr_ed25519` | IC threshold scheme | EVM: ECDSA (sources-evm.md section 0) |
| `signing.key_name` | object | `mainnet = key_1`, `testnet = test_key_1` | IC key names | `key_1` (cycle-costs page) |
| `signing.derivation_domain` | string[] | template with a `{network_identity}` element | Path (section 5.1) | `[rumi-icusd-sol-bridge, v1, {role}, {network_identity}]` |
| `signing.signed_payload` | string | free text, `Fact` | What is signed | Sui: BLAKE2b-256 of intent prefix and BCS data (sources-native.md section 2) |
| `wallet.standard` | enum | `eip1193_eip6963`, `solana_wallet_standard`, `sui_wallet_standard`, `vendor_specific` | Browser wallet interface | Solana Wallet Standard (sources-native.md section 1 row 6) |
| `wallet.named` | string[] | free text, `Fact` | Wallets with sourced support | Xaman (BB-41 Q6) |
| `prerequisites[]` | list | `{id, text, unavoidable: bool}`, `Fact` | Unavoidable external steps | Monad: MON above the 10 MON reserve (sources-evm.md section 6 row 7) |
| `networks[]` | list | at least `mainnet`, plus `testnet`; `devnet` only for solana, sui, xrpl | Per-network records below | |

### 9.3 Network record (`networks[]`)

| Field | Type | Allowed values | Meaning | Example (source) |
| --- | --- | --- | --- | --- |
| `network_id` | string | unique across all profiles | Instance name | `arbitrum-sepolia` |
| `role` | enum | `mainnet`, `testnet`, `devnet` | Network role | `testnet` |
| `identity` | `Fact` | `kind` in `evm_chain_id`, `solana_genesis_hash`, `sui_chain_identifier`, `xrpl_network_id`; unique `(kind, value)` | Public network identity | `evm_chain_id 421614` (sources-evm.md section 3 row 1) |
| `status` | enum | `disabled` only in groundwork | Activation state; DISABLED, not paused (section 8) | `disabled` |
| `instance_ref` | string or null | `null` in groundwork; later the instance canister principal | Which actor the frontend calls (1.5) | `null` |
| `activation_blockers[]` | list | `{id, text, kind: config\|code\|external\|decision, source}` | Why it is disabled | `hyperevm-historical-state` |
| `claims` | object | section 8 values | Distinct claims | all `none` or `false` |
| `finality.rule` | enum | `evm_tag`, `monad_verified`, `solana_commitment`, `sui_checkpoint`, `xrpl_validated` | Final-state rule | `monad_verified` (sources-evm.md addendum) |
| `finality.parameters` | object | `tag`, `commitment`, `execution_delay_blocks` (nullable), `typical_wait` | Rule parameters | `tag: finalized`; Monad delay `null` plus blocker |
| `rpc.path` | enum | `evm_rpc_canister`, `sol_rpc_canister`, `https_outcall_graphql`, `https_outcall_jsonrpc` | ICP read path | Robinhood: `evm_rpc_canister` with `Custom` (sources-evm.md section 4 row 4) |
| `rpc.native_variant` | string or null | EVM RPC canister variant name | Built-in provider set | `ArbitrumOne` |
| `rpc.min_quorum` | `Decision<u8>` | `>= 2` on mainnet | Identical results required | `2` of 3 (Base design 7.7) |
| `rpc.min_independent_providers` | u8 | `>= min_quorum` | Providers that must exist | Robinhood: only one keyless endpoint found |
| `rpc.historical_state_required` | bool | `true` for EVM | Numbered finalized state needed | `true` (HyperEVM blocker) |
| `rpc.endpoints[]` | string[] | host names only; no keys or query strings | Public endpoints | `rpc.hyperliquid.xyz` host (sources-evm.md section 5 row 1) |
| `rpc.outcall_notes` | `Fact` | free text | Outcall limits | XRPL non-443 port support unknown (sources-native.md Q3) |

Every factual leaf carries `source_url` and `verified`; every design choice is a `Decision` with a rationale.

### 9.4 Validator rules (named)

| Rule | Check |
| --- | --- |
| V1 | `schema_version` known; ids match the pattern and are unique; `(identity.kind, value)` unique across all files. |
| V2 | `status == disabled` for every network; `instance_ref == null`; `claims` at defaults; no `staged` or `enabled` accepted by this version. |
| V3 | `decimals == 8`; `debt_creating == false`; `backing == escrow_locked_icusd`; amount bounds match the family encoding (`u64`, `int63`, or `xrpl_decimal_15` with `max_e8s <= 999999999999999`). |
| V4 | Every `Fact` has an `https` URL (or a repo path for `repo`), a parseable date not after the supplied `today` (validator takes `today` as input, no clock), and `secondary` is rejected for identity fields. |
| V5 | Family consistency: `evm` uses `ecdsa_secp256k1`; `solana`, `sui`, `xrpl` use `schnorr_ed25519`; `finality.rule` matches family; Monad must use `monad_verified`. |
| V6 | Signing: `key_1` only on mainnet records, `test_key_1` only on testnet and devnet; `derivation_domain` unique per network and contains the identity element; no profile except `base-*` may equal a live Base string (section 5.1). |
| V7 | Required blockers present: HyperEVM `hyperevm-historical-state`; Monad `monad-verified-head`; Robinhood `robinhood-finalized-tag-unverified` and `single-keyless-endpoint`; Solana `solana-guard-missing` and `sol-rpc-canister-id-conflict`; Sui `sui-gate-module-missing` and `sui-rpc-provider-contract`; XRPL `xrpl-pr586-unmerged` and `xrpl-return-intent-missing`. |
| V8 | XRPL profile: `topology = backend_module`, `existing_work = pr586`; no second XRPL record of standard `xrpl_iou` or `xrpl_mpt` (one XRPL icUSD). |
| V9 | RPC: mainnet `min_quorum >= 2`; `min_independent_providers >= min_quorum`; if fewer providers are listed than that, a blocker must be present; `historical_state_required` for every EVM record. |
| V10 | Public-repo hygiene: reject 20-byte hex addresses, XRPL classic addresses, 64-hex hashes, absolute local paths, URLs with `key=`, `token=` or credentials. Public chain ids and genesis or chain identifiers are allowed. |
| V11 | Nine ids present exactly once: `ethereum, base, arbitrum, robinhood, hyperevm, monad, solana, sui, xrpl`. |

### 9.5 Crate recommendation

The coordinator's plan stands (R): a new workspace member `src/rumi_bridge_destinations` holding the types, the validator, the
nine JSON files under `profiles/` and unit tests, with no change to any canister. Evidence and constraints: the workspace
lists members explicitly (`Cargo.toml` members), so the only shared-file edit is one `members` line; `serde` and `serde_json`
are already used by `rumi_icusd_bridge` (`Cargo.toml:18-20`), so no new dependency family; the crate needs no `ic-cdk`, so
`cargo test -p` stays native and fast. The validator takes `today` as a parameter. Profiles are embedded with `include_str!` and
stay the single source for later TypeScript consumption (BB-39). No canister depends on the crate in groundwork.
A data-only alternative (JSON plus a script) would give up typed validation; I found no evidence against the crate.

## 10. Per-chain summary (J)

| Chain | Topology | Representation | Final-state rule | Signing | RPC path | Reuse source | Activation blockers |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Ethereum | EVM instance per network (`ethereum-*`) | ERC-20 (`BridgedIcUSD` bytecode) | `finalized` (about 13 min) | tECDSA `key_1` | EVM RPC canister: `EthMainnet`, `EthSepolia` native; Hoodi via `Custom` | `foundry/src/BridgedIcUSD.sol`; `src/rumi_icusd_bridge/**` | EVM Wasm generalization (D1): allow-list, provider variant, deploy-script allow-list; token must not be registered with a canonical L2 bridge |
| Base | Existing instances (reference) | ERC-20 | `finalized` (about 20 min) | tECDSA | `BaseMainnet` native; Sepolia via `Custom` | Reference; unchanged | Mainnet install and live status not established here |
| Arbitrum | EVM instance per network | ERC-20 | `finalized` (about 13 min after batch posting) | tECDSA | `ArbitrumOne` native; Sepolia via `Custom` | As Ethereum | D1; L1 poster fee in gas buffers |
| Robinhood | EVM instance per network | ERC-20 | Posted-batch Ethereum finality; tag UNVERIFIED | tECDSA | `Custom` 4663/46630, at least 2 independent providers needed | As Ethereum | D1; `finalized` tag probe; one keyless endpoint; PUSH0 and ArbOS unconfirmed |
| HyperEVM | EVM instance per network | ERC-20 | HyperBFT commit; tag UNVERIFIED | tECDSA | `Custom` 999/998 | As Ethereum | D1; **historical-state blocker** (archive providers or observer redesign); deploy gas near 3M small-block limit; 8-nonce queue |
| Monad | EVM instance per network | ERC-20 (name distinct from debt `IcUSD`, D8) | **Verified** head (finalized minus execution delay) | tECDSA | `Custom` 143/10143 | As Ethereum | D1; **Verified-head observer change**; explicit gas limits; reserve-aware funding; log range chunking |
| Solana | Solana-family instance per cluster | Token-2022 (8 decimals; metadata extensions only) or classic SPL | `finalized` | Schnorr Ed25519 (`sign_with_schnorr`) | SOL RPC canister (id conflict open); no `getLatestBlockhash` | `chains/solana/{ted25519,sol_rpc,settlement}.rs` patterns | D3 guard; return observer; nonce lane; ATA rent funding; Solana wallet connector; SOL RPC id |
| Sui | Sui-family instance per network | Currency Standard coin plus Move gate | Finalized checkpoint | Schnorr Ed25519, flag `0x00`, BLAKE2b-256 intent digest | HTTPS outcalls to GraphQL (JSON-RPC gone; gRPC unproven) | None in repo | Move module (D4); provider contract; `UpgradeCap` policy; Sui wallet connector |
| XRPL | Backend module (PR #586), no instance | Trust-line `icUSD` (D2 vs MPT) | Validated ledger, `tesSUCCESS`, `delivered_amount` | Schnorr Ed25519 over `STX\0` prefix | HTTPS outcalls to rippled JSON-RPC, quorum 2 | `xrpl_icusd/**`, `chains/xrp/**` on `b02584e4` | PR #586 unmerged and disabled; no-vault return intent missing; non-443 outcall port unknown; Xaman only |

## 11. Sequenced next steps (K)

1. **BB-38 (profiles and validator):** build the crate (9.5), nine disabled profiles, rules V1 to V11. First, because every later card cites a profile. No dependency.
2. **D-decisions:** settled for planning on 2026-10-30 except D6 (section 12). They shape implementation plans; none authorizes a live action.
3. **BB-34 (EVM reuse):** per `reuse-inventory.md` 2b: chain allow-list and provider variants (`rpc.rs:217-223`, `state.rs:235-247`), domain slug test reproducing Base strings exactly, per-chain head selection (Monad Verified head, Robinhood tag probe), HyperEVM held at its blocker. Order inside EVM: Ethereum, Arbitrum, then Robinhood, then Monad and HyperEVM.
4. **BB-35 (Solana):** specify the exact allowed transfer and burn instruction shapes, token owner and authority checks and memo association (6.3), and the custodied return lifecycle with no payout before a final burn (2.3); decide D3; specify the nonce lane, memo parser, return account and burn; resolve the SOL RPC canister id; choose Token-2022 or classic SPL after wallet checks; use the PocketIC Schnorr seams (6.4) as the harness.
5. **BB-36 (Sui):** specify the Move gate, the GraphQL provider and the `UpgradeCap` policy; check the equivocation hazard against Sui docs.
6. **BB-37 (XRPL):** specify the no-vault return intent as a follow-up to #586, decide D2 and D7, confirm outcall port support. No edit to #586.
7. **BB-39 (frontend wallets, done as a requirements note):** its section 6 is future implementation backlog: wallet-standard connectors (Solana, Sui), XRPL wallet beyond Xaman, chain-table rows, library policy (`baseBridge` avoids EVM libraries by design).
8. **BB-40 (integrated plan):** the aggregator and analytics subtraction (2.4), per-destination audit queries, cycle cost measurement per instance, activation order and the separate real-network signing step.

## 12. Open decisions for Rob and Miles

**Settled for planning (Miles, under Rob's implementation-decision authority, 2026-10-30).** These are planning
assumptions; they authorize no deployment, live upgrade, signing, funding, controller change or merge.

- **D1:** in-place source generalization on a working branch, Base behavior and stable state preserved; no deployed Base canister is upgraded; the EVM stack is not copied into a second artifact.
- **D1a:** new EVM instances keep the legacy Base-named Candid methods and fields at first, documented, routed by the per-network actor; generic aliases are optional later.
- **D1b:** additive native RPC-service variants where useful; the absence of a published endpoint list does not prove `Custom` impossible or force this API change; the provider and finality requirements still apply.
- **D2 and D7:** XRPL uses PR #586's issued-currency escrow and import seam; MPT is deferred, not a replacement gate; conservative 15-significant-digit caps on each operation AND on total outstanding, with exact rejection or holding of inexact amounts; the no-vault canonical return stays a named missing implementation.
- **D3:** Solana's first plan uses memo plus durable nonce with exact instruction, mint, owner and memo verification and the RC/BU/PR lifecycle; same-attempt reconciliation and archival history availability are mandatory; the missing monotone mint counter (C4) stays explicit; a program guard is a stronger later option, not a research gate.
- **D4:** plan the small Sui Move gate wrapping `TreasuryCap`, mint-id consumption, counters and recipient-bound returns; `UpgradeCap` stays a separate, disclosed authority; custody and live changes remain held.
- **D5:** reuse identical `BridgedIcUSD` bytecode, keeping `CONTRACT_VERSION` and its Base-labelled bridge-out domain; chain id and contract stay in the id preimages; only new off-chain instance domains are chain-qualified, and Base reproduces its strings exactly.
- **D8:** Monad uses explicit chain and escrow-backed labels plus exact contract binding; a new on-chain symbol is optional later; the debt token is never bound as a bridge token, nor the reverse.
- **D9:** synthetic test ledgers for future testnet demos; no real canonical icUSD is locked for a new testnet rollout under this groundwork.
- **Still open, D6:** actual controllers, production custody and authority activation are separate future decisions; no controller set is invented here. It does not block disabled groundwork.

The table below keeps the original options and tradeoffs for reference.

| Id | Decision | Recommendation and tradeoff |
| --- | --- | --- |
| D1 | Generalize the Base Wasm in place (chain allow-list, provider variants, domain slug) or build a separate EVM artifact. In place needs re-review of reviewed Base source and a Base-behavior-identical test; separate duplicates about 21k lines. | In place on a branch, Base canister not upgraded until a separate decision. |
| D2 | XRPL trust-line token (PR #586) or MPT (BB-32). IOU: implemented, 15-digit exactness limit, trust line needed. MPT: exact int63, flags fixed at creation, replaces the lane, wallet support varies. | Keep the IOU lane; record MPT as the alternative. |
| D3 | Solana guard: Memo plus durable nonce (no program, no on-chain consumed record, no C4) or a small program (consumed PDA, adds an upgrade authority). | Memo plus durable nonce first. |
| D4 | Sui: Move gate module (recommended, `UpgradeCap` becomes a mint-path authority; immutable removes fixes) or per-intent deposit addresses (no module, more gas). | Move gate; `UpgradeCap` custody separate from the mint key. |
| D5 | Non-Base EVM contracts: identical bytecode (domain string still says `base`, ids unique by chain id and contract) or a relabeled, newly reviewed contract. | Identical bytecode. |
| D6 | Controller and upgrade authority model for new instances and native authorities (BB-32 Q10). Not decided here. | Rob and Miles. |
| D7 | XRPL precision caps: 15 versus 16 digits; whether a trust-line limit enforces it. | 15-digit caps until verified. |
| D8 | Naming: escrow-backed and debt-backed icUSD on one chain (Monad). | Distinct names or labels. |
| D9 | Testnet escrow: real icUSD ledger or a test ledger. | Test ledger (a testnet token should not lock real icUSD). Not grounded in a source. |
| D1a | Raised by BB-34: one Wasm has one Candid, so an in-place EVM generalization cannot give each instance its own method names (1.5). Options: keep the Base method names on every EVM instance, or add generic methods additively while Base keeps its names. | Additive generic methods, Base names kept; decide with D1. |
| D1c | Raised by BB-34: Robinhood Chain and HyperEVM publish one keyless public endpoint each, so a quorum may need keyed providers, which conflicts with keeping no secrets in canister state. | Keyless independent providers only until a reviewed key-handling design exists; otherwise the chain stays disabled. |
| D5 x D8 | Raised by BB-34: identical bytecode hard-codes `ERC20("Rumi icUSD", "icUSD")` (`foundry/src/BridgedIcUSD.sol:157`), so on Monad, where a debt-backed IcUSD also exists, D8's distinct name needs a new reviewed contract (constructor name and symbol) or a label-only answer in wallets and the app. | Decide D5 and D8 together. |

## 13. Not grounded in source

- Whether IC Ed25519 signing is deterministic, and whether `sign_with_ecdsa` returns low-S (sources-native.md Q7); XRPL and Sui `secp256k1` are avoided for that reason.
- The controller set of any mainnet canister, and the per-canister threshold-key derivation rule beyond what the Base design states.
- Sui equivocation behavior, Solana freeze-authority mutability, XRPL trust-line limit as a balance bound, XRPL on-chain mantissa width beyond 15 digits.
- Whether the IC Ed25519 and ECDSA paths pass in the PocketIC seams in 6.4; no test was run.
- Robinhood and HyperEVM `finalized` tag support (the Monad delay is now sourced as three blocks), the SOL RPC canister id, and cycle costs of any new outcall or signing path.
- A dedicated XRPL no-vault return operation: absent from the PR #586 endpoint list I read; bodies of `observer.rs`, `retention.rs` and `settlement.rs` were not read in depth (BB-41).
