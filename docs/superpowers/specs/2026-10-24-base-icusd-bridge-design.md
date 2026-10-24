# Canonical ICP icUSD <-> Base icUSD bridge: design

Status: design for implementation on branch `codex/base-icusd-bridge`. No deployment, live signing,
role assignment, or funding is authorized by this document.
Date: 2026-10-24. Source baseline: `origin/main` at `32358d23d98b6430cab0236a3245519c9322fe90`.

## 1. Intent

Canonical icUSD lives on the ICP icUSD ledger (`t6bor-paaaa-aaaap-qrd5q-cai`). This bridge lets a
holder move existing icUSD to Base and back:

- **ICP -> Base.** The holder authorizes an ICRC-2 pull of `A` icUSD into a dedicated escrow account.
  After the pull is proven, the bridge canister mints `A - deposit_fee` Base icUSD to the chosen
  Base address, signed by its threshold-ECDSA minter address.
- **Base -> ICP.** The holder calls `bridgeOut` on the Base contract. It burns `B`, and the event binds
  `B`, the destination ICP account and a unique transfer id. Once that event is at Base L1-batch
  finality, the canister releases `B - release_fee - ledger_fee` canonical icUSD from escrow to that
  account.

Bridging moves existing value. It never creates borrowing debt, never touches vaults, and never
mints ICP icUSD. Ignoring the disclosed fees, a verified 100 icUSD escrow deposit authorizes exactly
one 100 icUSD Base mint, and a verified, finalized 100 icUSD bridge burn authorizes exactly one
100 icUSD ICP release.

Out of scope (deferred, see section 13): Base collateral, borrow-to-Base, other destination chains,
AMMs, liquidity incentives, and general cross-chain messaging.

## 2. Evidence from current source

Citations are against the baseline. Comments in older designs and module banners describe some
earlier behavior; the function bodies below were checked.

| Area | Finding | Source |
| --- | --- | --- |
| Existing EVM token | `IcUSD` (OZ 5.0.2 `ERC20` + `AccessControl`), 8 decimals, `mint(address,uint256,uint64 vault_id,uint64 op_id)` with `mintedOps[op_id]` dedup, no pause or cap. `burn(amount, target_vault_id)` emits `Burn(vault_id, burner, amount)`, which the backend treats as loan repayment. | `foundry/src/IcUSD.sol:13-67` |
| Backend decoding | Topic hashes are hard-coded: `BURN_EVENT_TOPIC0`, `MINT_EVENT_TOPIC0`. The mint selector is computed at runtime. `apply_burn_to_state` decrements vault debt; unknown vault burns are skipped as `InvalidBurn` and the value is lost. | `src/rumi_protocol_backend/src/chains/evm/evm_rpc.rs:134-159`, `chains/evm/tx.rs:150-164`, `chains/evm/deposit_watch.rs:281-345` |
| Registered EVM chains | `evm_chain_config` is a compile-time match on 10143, 71 and 1030 only. Base (8453 / 84532) is unknown to the backend, so the backend never watches a Base contract. | `chains/evm/mod.rs:78-100` |
| Debt-backed supply invariant | `sum(chain_supplies) == total_chain_vault_debt + reserve_backing + pending_chain_burn`, global across all chains. Timer B halts the protocol to `ReadOnly` on mismatch. | `chains/supply.rs:96-176`, `src/rumi_protocol_backend/src/xrc.rs:1253-1281` |
| Backend mint pipeline | No pre-broadcast claim for mints. An ambiguous broadcast re-reads the `latest` nonce and re-signs. Fee bumping overwrites `last_tx_hash`. Stuck detection counts timer ticks. Finality is a depth probe, not the `finalized` tag, and block hashes are not compared. | `chains/evm/settlement.rs:2120-2134, 3636-3641`, `chains/evm/hardening.rs:9-11`, `chains/evm/evm_rpc.rs:1464-1500` |
| Backend EVM RPC usage | Raw JSON-RPC through the EVM RPC canister `request` with `RpcService::Custom`, 2B cycles per provider call, 8192-byte response cap, whole-`result` vote with `max(floor, n/2+1)` agreement. `BaseMainnet` is mirrored as decode-only. | `chains/evm/evm_rpc.rs:73-78, 196, 800-818, 964-995, 1139-1140` |
| Backend threshold ECDSA | One EVM key per rail; paths `[chain_id LE, purpose]`; addresses bound to the backend canister id. `sign_with_ecdsa` attaches 26,153,846,153 cycles (ic-cdk 0.12.2 constant). | `chains/evm/tecdsa.rs:21-90`, `chains/evm/tx.rs:256-291` |
| Wasm budget | Deployed backend code section 12,253,978 B against a recorded 12,582,912 B limit, about 66 KB beyond the project's 256 KiB reserve, before PR #587 added more code. | release manifest for ckSOL (2026-10-18), local evidence; see section 4 |
| Ledger idempotency | Persist `created_at_time` + memo before awaiting and reuse on retry; classify outcomes as no-effect, ambiguous call, or ambiguous response; after any ambiguous attempt even a typed no-effect reply stays unknown; `TooOld` expiry is proven with a zero-value probe. | `src/rumi_protocol_backend/src/management.rs:456-556, 2410-2460, 2689-2737` |
| ICRC-3 proofs | `decode_block` / exact tuple validators; archive callbacks followed only for one exactly-covering range; bounded scans. Coupled to backend `State` types, so not linkable from another crate. | `src/rumi_protocol_backend/src/icrc3_proof.rs:164-241, 917-937, 1146-1337` |
| Supply reporting | Backend `get_global_icusd_supply` returns only the sum of debt-backed foreign `chain_supplies`. `rumi_analytics` reports ICP ledger `icrc1_total_supply` as circulating. Nothing subtracts locked balances. | `src/rumi_protocol_backend/src/main.rs:1472-1481`, `src/rumi_analytics/src/timers.rs:188-199` |
| Stable-state template | The cycle sentinel writes directly into `ic-stable-structures` with a unique `MemoryId` layout test and no `pre_upgrade` hook. | `src/rumi_cycle_sentinel/src/state.rs:13-160` |
| Frontend | Hand-rolled EIP-1193 flow with persisted pending-tx locks in `routes/ckusdc/+page.svelte`; no viem in vault_frontend; CSP `connect-src` excludes Base RPC hosts; Oisy needs the first signer call inside the click handler and ICRC-21 on any canister it calls; Plug delegation targets are listed in `pnp.ts`. | `src/vault_frontend/src/routes/ckusdc/+page.svelte:25-500`, `src/vault_frontend/static/.ic-assets.json:8-12`, `src/vault_frontend/src/lib/services/pnp.ts:54-79`, `src/vault_frontend/src/lib/services/oisySigner.ts:1-23` |
| Declarations | `scripts/regenerate-declarations.sh` (didc, hard-coded canister arrays). | `scripts/regenerate-declarations.sh:30-79` |
| CI | Only `.github/workflows/sp-deposit-intent-proof.yml`; backend, frontend and Foundry checks are run locally. | `.github/workflows/` |

The XRPL design (`docs/superpowers/specs/2026-10-17-xrpl-icusd-design.md`) proposes the same escrow
model for XRPL and is an unmerged proposal (PR #586). This design does not depend on it and does not
import its code.

## 3. Platform facts

- **Base finality** ([Base docs](https://docs.base.org/base-chain/network-information/transaction-finality)):
  Flashblock inclusion (~200 ms), L2 block inclusion (~2 s), L1 batch inclusion (~2 min), L1 batch
  finality (~20 min, batch older than 2 epochs / 64 L1 blocks, "in practice impossible to reverse").
- **OP Stack block tags** ([OP Stack docs](https://docs.optimism.io/stack/transactions/transaction-finality)):
  `latest` = unsafe head (sequencer only), `safe` = batch data included in an Ethereum block,
  `finalized` = that Ethereum block finalized. `finalized` therefore corresponds to Base's L1 batch
  finality stage.
- **EVM RPC canister** (`7hfb6-caaaa-aaaar-qadga-cai`, [ICP docs](https://docs.internetcomputer.org/guides/chain-fusion/ethereum.md)):
  `request : (RpcService, json : text, maxResponseBytes : nat64) -> (RequestResult)`. `RpcService`
  includes `BaseMainnet : L2MainnetService` (Alchemy, Ankr, BlockPi, PublicNode, Llama) and
  `Custom : RpcApi`. Base Sepolia needs `Custom` URLs.
- **Threshold ECDSA**: `key_1` for production, `test_key_1` for test subnets; derivation is bound to
  the calling canister's principal plus the derivation path.

## 4. Canister boundary: a separate `rumi_icusd_bridge` canister

Two options were compared.

| Concern | Isolated module in `rumi_protocol_backend` | Separate `rumi_icusd_bridge` canister (chosen) |
| --- | --- | --- |
| Wasm budget | Not available: about 66 KB of headroom beyond reserve, and a bridge needs an EVM tx engine, ledger saga, observer and ICRC-21. | Its own 12 MiB budget; the skeleton is 0.5 MB. |
| Blast radius | Runs inside the icUSD minting account. A bridge bug in-process could mint unbacked ICP icUSD. | Not a minting account. It can only move icUSD it holds; worst case is bounded by escrow balance plus the Base supply cap. |
| Reuse | Could call the existing EVM settlement queue, but its mint semantics are vault-bound and its reconciliation gaps (section 2) would need changes to live Conflux code paths. | Reuses the same libraries and protocol patterns, with a stricter tx engine; the live backend is untouched. |
| Upgrades | Every bridge fix needs a backend upgrade (whole-`State` CBOR snapshot, highest-risk canister). | Independent upgrades; stable structures, no `pre_upgrade` snapshot. |
| Signing domain | Shares the backend's single EVM key-rail configuration and address caches. | Distinct principal, so all derived addresses are distinct from every live backend address by construction, plus explicit purpose paths. |
| Supply accounting | Tempting to add Base to `chain_supplies`, which would break the debt-backed invariant. | Separate escrow ledger with its own invariant; the backend invariant is unchanged. |

**Decision: a new canister `rumi_icusd_bridge`.** It is the smallest sound implementation because
it needs no backend change, no backend upgrade, and no change to existing chain semantics.

Reuse is at the level that fits: the same pinned libraries (`ic-cdk 0.12`, `candid 0.10`,
`ic-stable-structures 0.6`, `icrc-ledger-types` from the IC fork rev `fc278709`, `k256`, `sha3`,
`alloy-rlp`), the same EVM RPC canister `request` transport and quorum voting rule, the same
EIP-1559 / y-parity recovery approach, the same ledger idempotency and outcome taxonomy, the same
ICRC-3 decoding rules, the sentinel's stable-memory layout pattern, OpenZeppelin primitives on the
contract side, the existing icUSD ledger Wasm and PocketIC harness patterns for tests, and the
existing frontend auth, actor, fee-cache, Oisy and EIP-1193 code. The backend modules themselves
(`tx.rs`, `tecdsa.rs`, `icrc3_proof.rs`) import backend global `State` and cannot be linked without
pulling in the backend; extracting a shared crate would change the live backend's Wasm. A shared
primitives crate is a follow-up, not a prerequisite.

## 5. Authority

### 5.1 ICP side

| Authority | Holder | What it can do | Who changes it |
| --- | --- | --- | --- |
| Escrow account `(bridge, ESCROW_SUBACCOUNT)` | Bridge canister code only | Release to the account named in a finalized `BridgeOut`; refund a deposit to its own source account after non-delivery is proven; sweep accrued fees (bounded by the fee bucket) to the configured fee recipient. There is no general withdraw endpoint. | Canister code, i.e. controllers via upgrade. |
| ICRC-2 pull | The depositor | Pull exactly `A` from `Account{caller, from_subaccount}` once, under the user's own `icrc2_approve` to the bridge canister. The canister never pulls from any account except the authenticated caller's. | User. |
| Canister admins | Principals in config (init arg; changed only by controllers) | Pause/unpause; set limits, fees, gas and observer settings within hard bounds; bind the Base contract once (only with zero liabilities); sweep fees; submit Base contract admin actions; retry or recover held operations. Admins cannot move escrow except via the paths above. | Controllers (`set_admins`, controller-only). |
| Controllers | Same trust root as other Rumi canisters | Upgrade code (which could do anything with escrow and keys); replace RPC providers (chain-id verified); set admins. | NNS/controller set. |

The bridge canister is **not** the icUSD minting account, so it cannot create ICP icUSD.

### 5.2 Base side (`BridgedIcUSD`)

| Role | Holder | Powers |
| --- | --- | --- |
| `MINTER_ROLE` | Bridge minter address: tECDSA path `["rumi-icusd-base-bridge", "v1", "minter", chain_id_be]` | `mint(to, amount, mintId)` only; subject to `mintPaused`, `supplyCap`, per-window limit and `mintConsumed`. |
| `DEFAULT_ADMIN_ROLE` | Bridge admin address: path `[..., "admin", chain_id_be]` | Set cap, window limit, minimum bridge-out; unpause; grant and revoke roles. Granting `MINTER_ROLE` is a mint authority path, so the admin must only be held by the canister. Admin transfer is OZ `AccessControlDefaultAdminRules` two-step with a delay. |
| `PAUSER_ROLE` | Admin, minter, optional guardian (default none) | Pause minting or bridge-out. Cannot unpause. |
| Deployer | Ephemeral EOA | Nothing after the constructor; the deploy script asserts it holds no role. |

There is no operator hot key. Contract admin actions are canister-signed transactions created
through the admin endpoint and submitted on a separate nonce lane.

Mint authority paths, exhaustively: (1) a deposit's delivery mint, (2) a held bridge-out's re-mint to
its Base sender. Both require a consumed, escrow-backed claim in canister state. A third path exists
only if the admin grants `MINTER_ROLE` to another address; the canister exposes no action that does
that.

## 6. Base contract: `BridgedIcUSD` (new, versioned)

An explicitly versioned Base-specific contract (`VERSION = "rumi-base-icusd-bridge-v1"`) built from
OZ 5.0.2 `ERC20` and `AccessControlDefaultAdminRules`. `IcUSD.sol` and its consumers are unchanged.

```solidity
function mint(address to, uint256 amount, bytes32 mintId) external;          // MINTER_ROLE
function bridgeOut(uint256 amount, bytes icpPrincipal, bytes32 icpSubaccount, bytes32 clientNonce)
    external returns (bytes32 transferId);
function computeTransferId(address from, bytes32 clientNonce) view returns (bytes32);
event BridgeMint(bytes32 indexed mintId, address indexed to, uint256 amount);
event BridgeOut(bytes32 indexed transferId, address indexed from, uint256 amount, uint64 sequence,
                bytes icpPrincipal, bytes32 icpSubaccount);
```

- No repayment burn, no `burn`, no `burnFrom`. `bridgeOut` is the only burn path and its event has a
  different signature from `Burn(uint256,address,uint256)`, so it can never be decoded as a repayment.
- `transferId = keccak256(abi.encode(keccak256("rumi.icusd.base.bridge-out.v1"), chainid, this,
  from, clientNonce))`, stored in `bridgeOutUsed`. The same sender and client nonce cannot be
  submitted twice, which is the frontend's duplicate guard.
- `bridgeOutCount` increments per bridge-out and is emitted as `sequence`. The canister uses it to
  prove completeness of its log scan.
- Validation: amount in `[max(1, minBridgeOut), u64::MAX]`; principal 1..29 bytes and not the
  anonymous principal `0x04`.
- `mint` checks: not paused; recipient not zero or the contract; amount in `1..=u64::MAX`; `mintId`
  unused; `totalSupply + amount <= supplyCap`; fixed-window rate limit. Every mint adds to the
  cumulative `totalMinted` counter (never decreases).

## 7. Canister design

### 7.1 Identifiers and domain separation

| Id | Derivation |
| --- | --- |
| Deposit id | `u64`, monotonically assigned from a stable counter; never reused. |
| Mint id | `keccak256(abi.encode(keccak256("rumi.icusd.base.mint.v1"), chain_id, contract, instance_salt, deposit_id))` |
| Re-mint id | `keccak256(abi.encode(keccak256("rumi.icusd.base.remint.v1"), chain_id, contract, instance_salt, transfer_id))` |
| Transfer id | Contract-computed (section 6). |
| Ledger memo (pull) | `b"RBRG-DEP" ++ deposit_id BE` (16 bytes) |
| Ledger memo (release) | the 32-byte transfer id |
| Ledger memo (refund) | `b"RBRG-RFD" ++ deposit_id BE`; fee sweep `b"RBRG-FEE" ++ sweep_id BE` |

`instance_salt` is `sha256("rumi-icusd-base-bridge:instance-salt:v1" || canister id || install time)`,
fixed at init, so a hypothetical reinstall cannot reuse mint ids already consumed on the contract.

### 7.2 Accounting buckets

All amounts are e8s. `E` is the actual icUSD ledger balance of the escrow account. Liability buckets
are disjoint:

| Bucket | Meaning |
| --- | --- |
| `O` | Base outstanding: finalized delivered mints minus finalized processed bridge-outs. |
| `PM` | Pending mints: full pulled amount `A` of deposits escrowed but not yet delivered (includes the unearned deposit fee); also held re-mints in progress. |
| `PR` | Pending releases: finalized valid bridge-outs not yet released. |
| `RF` | Pending refunds: deposits being returned to their source account. |
| `H` | Held claims: finalized bridge-outs that cannot be released as requested (invalid destination, below fees), awaiting re-mint to their Base sender. |
| `F` | Fees earned and not yet swept. |

`L = O + PM + PR + RF + H + F`. `U` = total outflow (`amount + ledger_fee`) of escrow transfers whose
outcome is unknown.

**Invariant C1 (escrow coverage):** `E >= L - U`. Surplus `S = E - (L - U)` is reported, never spent.
Any violation sets `halted` and stops mints, releases, refunds and sweeps.

**Invariant C2 (Base supply bound):** at a finalized Base block, `totalSupply <= O + M_inflight`, where
`M_inflight` sums mint amounts of operations with any signed attempt not yet finalized. A violation
means unauthorized Base supply; it halts the bridge.

**Invariant C4 (cumulative mints):** the contract's cumulative `totalMinted` at the finalized head
never exceeds the canister's own cumulative finalized mints plus mints in flight. It is checked
before any burn is recorded and at every reconciliation. Unlike C2 it cannot be hidden by burning
the excess again, so a mint this canister did not make (a canister bug, or a compromised
`MINTER_ROLE` grant) halts the bridge before its burn can be paid from escrow. Residual exposure:
a foreign mint no larger than the mints in flight at that moment passes once; it is caught when
those mints finalize.

**Invariant C3 (exact quiescent match):** when no mint attempt is outstanding and the observer has
consumed every bridge-out through finalized block `F` (`processed_count == bridgeOutCount(F)`),
`totalSupply(F) == O`. Reported as a reconciliation status.

Transitions (all in one state update unless noted):

| Event | Effect |
| --- | --- |
| Deposit admitted | capacity reservation `+= A` (not a liability yet) |
| Pull proven (`Ok` or `Duplicate`) | `E += A`; `PM += A`; reservation `-= A` |
| Pull proven failed | reservation `-= A` |
| Delivery mint finalized and verified | `PM -= A`; `O += M` where `M = A - deposit_fee`; `F += deposit_fee` |
| Delivery mint finalized reverted | deposit held; no bucket change |
| Refund requested (non-delivery proven) | `PM -= A`; `RF += A` |
| Refund transfer proven | `RF -= A`; `E -= A` (sends `A - ledger_fee`, ledger fee from `A`) |
| Bridge-out finalized, valid | `O -= B`; `PR += B` |
| Bridge-out finalized, not releasable | `O -= B`; `H += B` |
| Release transfer proven | `PR -= B`; `F += release_fee`; `E -= B - release_fee` |
| Held re-mint started | `H -= B`; `PM += B` |
| Re-mint finalized and verified | `PM -= B`; `O += B` |
| Fee sweep proven | `F -= S`; `E -= S` (sends `S - ledger_fee`) |

An ambiguous mint keeps its `A` in `PM` and creates no other claim. Nothing moves to `RF` until
non-execution is proven, so one deposit can never be both delivered and refunded.

### 7.3 Coexistence with debt-backed supply

The backend's debt-backed `chain_supplies` (Monad, Conflux) and this escrow bridge are separate books:

- Base is not registered in the backend and the backend never observes `BridgedIcUSD`.
- The bridge never calls the backend and never touches vault debt.
- Escrowed icUSD is ordinary ICP ledger supply, already counted in `icrc1_total_supply`.

Aggregate reporting must therefore be:

```
total_icusd           = icp_ledger_total_supply + sum(debt_backed_chain_supplies)
circulating_on_icp    = icp_ledger_total_supply - E
circulating_on_base   = base_total_supply (<= O + M_inflight)
circulating_total     = circulating_on_icp + circulating_on_base + sum(debt_backed_chain_supplies)
```

Base supply must never be added to `total_icusd`: it is a representation of escrowed ledger tokens.
`get_supply_audit` on the bridge returns `E`, every bucket, `U`, `S`, last finalized `totalSupply`,
and the C1/C2/C3 status, so analytics can compute the lines above without double counting. The
backend's self-check is unaffected because neither its inputs nor its state change. Updating
`rumi_analytics` to subtract `E` is a reporting follow-up, not a gate.

### 7.4 ICP -> Base

States: `PullPending -> {Escrowed | PullUnknown | PullFailed}`, `PullUnknown -> {Escrowed | PullFailed}`,
`Escrowed -> MintQueued -> MintInFlight(tx_id) -> Delivered`, `MintInFlight -> MintHeld(reverted)`,
`Escrowed | MintQueued -> MintHeld(paused | cap | gas | config)`, `MintHeld -> MintQueued` (retry) or
`-> Refunding -> {Refunded | RefundUnknown -> Refunded}`.

1. `bridge_to_base(args)` rejects anonymous callers; requires deposits unpaused, a bound contract,
   valid recipient (20-byte, non-zero, not the contract), amount bounds, the expected fee equal to
   the current fee, capacity (`O + PM + reservations + A <= max_outstanding`), the daily window, and
   per-caller and global active-operation limits. `client_request_id` (16-32 bytes) is unique per
   caller: a repeated id with identical arguments returns the existing operation; with different
   arguments it is rejected.
2. The deposit is persisted as `PullPending` with `created_at_time`, memo, fee snapshot and a ledger
   tip lower bound, before the await.
3. `icrc2_transfer_from(from = {caller, from_subaccount}, to = escrow, amount = A, fee = None,
   memo, created_at_time)`. Typed `Ok`/`Duplicate` proves the pull. A typed no-effect reply on the
   first attempt proves failure. A reject, trap after send, or a typed reply after an earlier
   ambiguous attempt is `PullUnknown`.
4. `PullUnknown` is reconciled by the timer by resending the identical tuple. Only `Ok`/`Duplicate`
   prove success. `TooOld` (or a deposit older than the dedup window) switches to a bounded ICRC-3
   search for the exact block (`2xfer`, spender = bridge, from, to, amount, memo, created_at_time),
   following archive callbacks only to archives listed by `icrc3_get_archives`. A complete search
   window with no match proves failure.
5. The mint is queued. Before signing, the tx lane checks pause/cap/window locally and simulates the
   mint with `eth_call` from the minter address. A simulated revert holds the deposit with a reason
   and is retried later; it is never treated as a revert of a sent tx.
6. Delivery is accepted only when the receipt of one of the operation's recorded attempt hashes is at
   or below the quorum `finalized` head, the receipt's block hash equals the canonical block hash at
   that height, status is success, and the receipt contains exactly one `BridgeMint(mintId, to, M)`
   log from the bound contract.

### 7.5 Base -> ICP

States: `Observed -> {ReleasePending | Held}`, `ReleasePending -> {Released | ReleaseUnknown}`,
`ReleaseUnknown -> Released`, `Held -> Reminting(deposit-like op) -> Reminted`.

1. The user's Base transaction is the authorization: it binds amount, destination principal and
   subaccount, sender, and transfer id. Wallet callbacks and tx hashes are hints only.
2. **Single settlement path:** only the finalized-log observer creates release records.
   `notify_base_bridge_out(tx_hash)` (non-anonymous, bounded) records a hint shown in
   `get_bridge_info`. It never settles and never changes polling cadence (section 11a).
3. Observer pass: quorum `finalized` head `(F, hash_F)`; quorum `bridgeOutCount()` at `F`; if it
   exceeds `processed_count`, quorum `eth_getLogs(address = contract, topics = [BridgeOut],
   from = cursor + 1, to = F)` in bounded chunks. Each log must carry `removed = false`, the bound
   contract address, the exact topic layout and a sequence in `(processed_count, count_F]`. The
   cursor and count advance only when every expected sequence is found. Missing or inconsistent
   results leave the cursor in place and are reported.
4. Release record keyed by transfer id (and checked against `(tx_hash, log_index)`). A known
   transfer id is never processed twice.
5. Destination check: valid principal bytes, not anonymous, not the bridge canister itself, and
   `B > release_fee + ledger_fee`. Otherwise the claim is held and re-minted to the Base sender
   under a re-mint id. No arbitrary claimant can redirect it.
6. Release transfer `icrc1_transfer(from_subaccount = ESCROW, to, amount = B - release_fee -
   ledger_fee, fee = ledger_fee, memo = transfer_id, created_at_time)`, persisted before the await,
   with the same outcome taxonomy as the pull. `BadFee` on a first attempt is a no-effect reply that
   allows a new tuple with the new fee; after any ambiguous attempt it is unknown and goes through
   search.
7. Releases are subject to `releases_paused`, `halted`, and a daily release window. Excess waits in
   `PR`; nothing is lost.

### 7.6 Base transaction lanes

Two lanes (minter, admin), each with a persisted `next_nonce`, initialized once from quorum
`eth_getTransactionCount(address, "latest")` and `"pending"` (must agree).

- A tx record holds lane, nonce, purpose, gas limit and every attempt (fees, signed raw bytes, hash).
  Each attempt is persisted before broadcast. Its hash is computed locally from the signed bytes.
- At most one unincluded tx per lane; at most `max_unfinalized` (default 8) included-but-not-final.
- Broadcast responses are hints. Errors and "already known" / "nonce too low" lead to receipt checks.
- Receipts are checked for **every** attempt hash. A replacement (`+12.5%` on both fee fields, capped)
  is created only when no attempt has a receipt, the quorum `latest` nonce still equals the tx nonce,
  and the attempt is older than `replace_after_secs` (wall clock, default 90 s).
- If the quorum finalized nonce is past the tx nonce and no attempt receipt can be found, the lane
  halts. That would mean a key or record anomaly; nothing is re-signed.
- A receipt that disappears before finality (reorg) returns the tx to unincluded; the same signed
  bytes are rebroadcast.
- Admin recovery `admin_cancel_stuck_tx` adds a zero-value self-transfer attempt at the same nonce.
  Whichever attempt finalizes decides the outcome; a deposit can be refunded only when the finalized
  winner is not its mint and `mintConsumed(mintId)` is `false` at the finalized head.
- The minter's ETH balance must cover `gas_limit * max_fee + min_gas_reserve_wei` before signing
  (Base also charges an L1 data fee). Otherwise the lane waits in `AwaitingGas`, which the bridge
  page shows. Gas is operator funding, never paid from escrow.
- Gas limits: mint 300,000 by default (floor 250,000), admin 100,000 (floor 80,000), both at most
  1,000,000. Measured on anvil, the most expensive mint (first mint ever, after the first mint window
  ended, to a recipient with 20 non-zero bytes) uses 152,390 gas and the most expensive admin call (a
  first pause) 46,905. The mint preflight `eth_call` carries the exact gas limit of the transaction,
  so a limit that is too low becomes a preflight hold (retried automatically, no nonce used, nothing
  signed) rather than a paid on-chain revert; the transaction is created only if the configured
  limit still equals the simulated one.
- Mint order: user deposits first; a re-mint starts only when no user deposit is waiting and fewer
  than two re-mints are in flight, so recycled held burns cannot delay user deposits.

### 7.7 RPC and finality policy

- Transport: EVM RPC canister `request` per configured service (`BaseMainnet(provider)` or
  `Custom{url}`), `min_quorum` identical results required (default 2 of 3), sequential calls,
  configurable response cap (logs default 64 KiB).
- Settlement finality: `finalized` tag, taken as the lowest agreeing quorum value, then the block
  hash at that height by quorum. `latest` and `safe` are shown to users as progress only.
- Inconsistent providers never advance state; the condition is visible in `get_bridge_info`.

### 7.8 Limits, pauses and halts

Canister: per-operation min/max, `max_outstanding`, daily mint window, daily release window, per-caller
active operations (5), global active operations (500), fee hard caps, gas caps. Pauses:
`deposits_paused`, `mints_paused`, `releases_paused`, `observer_paused`. The global `halted` flag is set
automatically on a C1, C2 or C4 violation, an unexplained burn excess, an already-consumed mint id, or
an unexpected ledger `InsufficientFunds`; only an admin can clear it, and only after the checks pass
again (a C4 violation can never be cleared this way). A lane anomaly (nonce consumed with no matching
receipt) halts only that lane and parks its transaction (see 11a, lane-halt escape policy); the rest
of the bridge keeps running. Pauses never erase liabilities or forget signed attempts;
reconciliation keeps running.

Contract (defense in depth): `supplyCap`, per-window mint limit, `mintPaused`, `bridgeOutPaused`.
At contract bind time the canister reads `VERSION`, `decimals`, roles, `defaultAdmin`, `totalSupply`
and limits at the finalized head and refuses to bind unless they match; canister limits must not
exceed contract limits.

### 7.9 Persistence and upgrades

`ic-stable-structures` with a fixed `MemoryId` layout and a uniqueness test: a CBOR `StableCell` for
config and counters, `StableBTreeMap`s for deposits, releases, txs, the per-owner index, the
`(owner, client_request_id)` index, and an append-only audit log (`StableLog`; it grows with operations, not with time). Records are CBOR with
`#[serde(default)]` on added fields (CBOR is self-describing, unlike Candid `Decode!`). There is no
`pre_upgrade` hook; `post_upgrade` restarts the timer. Operation ids, attempts and buckets survive
upgrades; tests upgrade the canister mid-flight. Upgrades should follow stop -> upgrade -> start so
no call is in flight.

### 7.10 Interface

New `rumi_icusd_bridge.did`, generated from Rust with `export_candid!` and checked by a test that the
checked-in file is equal to the generated service. User methods: `bridge_to_base`, `get_deposit`,
`get_deposit_by_request`, `list_my_deposits`, `list_deposits_for_owner`, `get_release`,
`list_releases_for_owner`, `list_releases_for_base_sender`, `notify_base_bridge_out`,
`request_deposit_refund`, `get_bridge_info`, `get_supply_audit`, `get_base_addresses`,
`get_audit_log`, `bridge_version`.
Admin methods: `admin_set_pauses`, `admin_set_limits`, `admin_set_fees`, `admin_set_gas`,
`admin_set_observer_config`, `admin_set_rpc_services`, `admin_bind_contract`, `admin_sweep_fees`,
`admin_submit_contract_action`, `admin_cancel_stuck_tx`, `admin_retry_deposit`, `admin_reconcile`,
`admin_clear_halt`, `admin_clear_lane_halt`, `admin_run_tick`, and `set_admins` (controllers). Also
`icrc21_canister_call_consent_message`, `icrc28_trusted_origins` and `icrc10_supported_standards`
for Oisy. An `inspect_message` filter rejects anonymous ingress to update methods (except the ICRC
standard methods) and non-admin or non-controller ingress to admin and controller methods before
execution, and refuses arguments over 4 KiB (open methods) or 16 KiB (admin and controller
methods); it is a cost filter only, and every method keeps its own checks. Read methods are
queries and must be called as queries: a query name sent as an update call is refused, so
replicated reads never cost the bridge cycles (the runbook passes `--query`, checked by a test). Frontend bindings
come from `scripts/regenerate-declarations.sh`.

## 8. Frontend

A Bridge page at `/bridge` in vault_frontend, linked from the footer and desktop nav behind
`BRIDGE_ENABLED = CANISTER_IDS.ICUSD_BRIDGE !== ""` (the Points pattern). Base reads use the
connected wallet's EIP-1193 provider (`eth_call`, `eth_getTransactionReceipt`), so no CSP change is
needed. The canister is the settlement authority; the browser only shows hints.

- **ICP -> Base:** amount, Base recipient (defaults to the connected Base account, editable),
  disclosed fees (approve fee if an approval is needed, transfer fee, deposit fee), and expected
  delivery (~20 min after the pull). An approval is requested only when the anonymous allowance read
  is below `A + transfer_fee`. Approve and `bridge_to_base` run sequentially inside one click (Oisy
  rule: for Oisy and other popup signers, approve and bridge are separate clicks). A
  `client_request_id` is persisted before the first signer call.
- **Base -> ICP:** connect Base wallet; switch or add the Base network; show the ICP destination
  (the logged-in principal, explicit and visible in the confirmation step); amount and fees. A random
  `clientNonce` and the contract-computed `transferId` are persisted before the wallet prompt. Before
  any resend, `bridgeOutUsed(transferId)` is checked; the contract rejects duplicates anyway.
- **States shown:** pull pending, escrowed, mint pending, included on Base, finalized/delivered,
  held (with reason and refund action), refunding/refunded; wallet pending, rejected, submitted,
  included, awaiting finality, release pending, released, held/re-minted.
- **Recovery:** history comes from `list_my_deposits` / `list_releases_for_owner` (and
`list_releases_for_base_sender` for a Base wallet); localStorage
  (`rumi:base-bridge:v1:<principal>`) stores only intents and hints. Uncertain outcomes are shown as
  uncertain; a second transfer is never started automatically. Wrong network, rejected approval,
  account changes (Base or ICP session), allowance shortfall, insufficient ETH for gas, and reloads
  each have an explicit state. When the minter lane reports `awaiting_gas`, the ICP -> Base tab
  shows that deliveries wait for bridge gas and waiting deposits read "Waiting for bridge gas".

## 9. Fees and gas

- ICP -> Base: user pays the ICP approve fee (only if approving) and the `transfer_from` fee on top of
  `A`; the deposit fee (`deposit_fee_e8s`, default 0) is deducted from the minted amount.
- Base -> ICP: user pays Base gas for `bridgeOut`; the ICP ledger fee and `release_fee_e8s` (default
  0) are deducted from the released amount.
- Mint gas (L2 execution plus L1 data fee) is paid by the minter address, funded by operators. It is
  never paid from escrow and never charged silently.

## 10. Failure policy summary

| Situation | Policy |
| --- | --- |
| Ledger reply lost after a successful pull/release/refund | Same tuple resent; `Duplicate` proves it; after the dedup window, ICRC-3 search. |
| Signature reply lost | No tx exists without the bytes; re-sign the same unsigned tx. |
| Broadcast timeout / RPC error | Not failure. Check receipts for all attempts; rebroadcast the same bytes. |
| Mint landed but reply lost | Receipt of a recorded attempt proves it; `mintConsumed(mintId)` is the destination truth. |
| Mint reverted (finalized) | Deposit held; retry with a new tx and the same mint id, or refund after proof. |
| Burn observed then reorged | Only finalized logs settle, so nothing to undo. |
| Inconsistent RPC responses | No quorum, no state change; visible status. |
| Unknown nonce consumption | Lane halts; manual investigation. |
| Coverage or supply mismatch | Bridge halts. |

## 11. Testing

Contract: Foundry unit, fuzz and invariant tests, plus ABI pins shared with Rust and TypeScript.
Canister: pure unit tests for accounting transitions (exact e8s boundaries, fees, insufficient
balances, overflow), ABI and RLP encoding (vectors cross-checked with Foundry), lane state machine
(lost replies, replacements, reorgs, anomalies), observer completeness, ICRC-3 search, Candid
equality, stable-layout uniqueness and upgrade decode. Integration: PocketIC with the real icUSD
ledger Wasm, threshold ECDSA from the II subnet, and a relay canister that serves the EVM RPC
canister `request` method by HTTPS outcall; the test harness forwards those outcalls to a local
anvil running `BridgedIcUSD`, so canister-signed transactions execute on a real local EVM.
Frontend: vitest specs for the flow state machine, ABI encoding, persistence and wallet/network
changes.

Evidence must distinguish mocked external-chain behavior (injected provider faults) from actual
local-EVM execution (anvil).

## 11a. Implementation notes (decisions made while building)

- **Hints never speed up polling.** Any signed-in caller can submit a hint, so hints are bounded
  (256 total, 5 per caller, 2 h TTL) and are excluded from the "work pending" test that selects the
  fast timer interval. Otherwise a caller could force paid RPC polling.
- **Releases to the ledger minting account are held.** An `icrc1_transfer` to the minting account is
  a burn; such a bridge-out is held and re-minted to its Base sender. Releases are not classified
  until the ledger fee and minting account are known.
- **Cheap idle observer.** Block responses on Base exceed 8 KiB, so the finalized-head read needs a
  64 KiB response cap. Between full passes (at least hourly) an idle observer only probes
  `bridgeOutCount()` at the `finalized` tag; any change, disagreement or error triggers a full pass.
  Automatic supply reconciliation runs every 30 minutes and on `admin_reconcile`.
- **Escrow outflow is reserved before the first send.** `unknown_outflow` includes every escrow
  transfer that has been sent at least once and is not yet proven, so a trap after send can never
  produce a false coverage deficit.
- **Lane-halt escape policy.** If a lane's nonce is consumed and none of its recorded attempts has
  a receipt, the lane halts and its transaction is parked. Nothing is re-signed and the deposit's
  backing stays in `pending_mint`. This indicates a key or record anomaly, so recovery is a
  controller decision: investigate, then ship an explicit fix by upgrade. `admin_clear_lane_halt`
  only clears halts with no outstanding transaction whose chain nonce matches local records.
- **Review round 1 changes.**
  - `bridge_to_base` validates the request, then pre-checks the caller's allowance and balance
    before persisting anything, so a caller without funds creates no records and no paid work
    (`InsufficientAllowance` / `InsufficientFunds`). The background tick is scheduled only for work
    still in progress, and a request can never push a pending tick later.
  - Each tick arms its fallback successor before its first await, so a trap after an await cannot
    end the timer chain; admin endpoints also re-arm it.
  - Lanes settle mints before the observer records burns. A burn that exceeds settled outstanding
    supply but is covered by in-flight mints waits and is retried; only an unexplained excess halts.
  - The observer adapts its `eth_getLogs` span and response cap, proves completeness by sequence
    contiguity plus the head counter (rewinding on a mismatch) without requiring historical state
    at every chunk end, backs off after repeated failures, and checks `eth_chainId` at bind.
  - Ledger replies are classified against the op as stored after the await, and a non-success reply
    from an older attempt never resolves an op whose newer attempt is outstanding.
  - Coverage (C1) compares the balance with the buckets before and after the balance read, so a
    concurrent transition cannot fake a deficit; the audit reports the stored verdict.
  - The Base supply check is skipped when recorded burns are above the finalized head, and C3
    quiescence is count-based.
  - Capacity counts held claims; a release larger than the release window goes out alone at the
    start of a fresh window; replacing RPC providers is controller-only.
  - `SupplyAudit` now carries the ICP ledger total supply and the derived
    `circulating_on_icp = total - escrow` and `circulating_on_base` figures.
  - Contract `totalMinted` and invariant C4 (above); bind requires `totalMinted() == 0`.
  - Oisy uses two clicks when an approval is needed (each signer call is the first signer call of
    its own click); Internet Identity and Plug keep one click.
- **Review round 2 changes.**
  - Releases are re-read per iteration and a prepared release never replaces an existing ledger
    tuple, so overlapping ticks cannot pay one burn twice; the tick guard and both lane guards carry
    an owner token so a slow run never clears a newer run's claim.
  - A tick more than 60 s overdue is treated as lost and re-armed; held states no longer keep the
    timer at the fast cadence; a burn waiting on in-flight mints is a typed deferral (no backoff),
    and a long deferral is recorded and audited.
  - Mint gas sizing, preflight simulation at the exact limit, and mint ordering (7.6).
  - An ICRC-3 absence proof is applied only if the op is unchanged since the search started.
  - Recipient addresses with mixed case must match their EIP-55 checksum (canister and page).
  - `admin_set_rpc_services` checks the controller before its await and passes it through.
- **No `icp.yaml` entry.** The bridge is deliberately absent from `icp.yaml` so a broad deploy can
  never create or install it by accident. `foundry/DEPLOY-BASE.md` uses explicit commands.

## 12. Assumptions

1. The icUSD ledger supports ICRC-2 and ICRC-3 (`icrc3_get_blocks`, `icrc3_get_archives`) with a 24 h
   dedup window and 32-byte memos, as the checked-in ledger Wasm does.
2. Base `finalized` from at least two independent providers reflects L1 batch finality.
3. The EVM RPC canister forwards `request` faithfully; provider-specific JSON fields are normalized
   before voting (we vote on the parsed fields we use, not raw bodies).
4. tECDSA `key_1` is available on the deployment subnet; `test_key_1` on testnet fixtures.
5. Operators fund minter and admin gas on Base; this is a live action outside this work.

## 13. Deferred

Base collateral; direct borrow-to-Base; other destination chains (the canister is Base-only, chain id
fixed at init); AMMs and liquidity incentives; general cross-chain messaging; a shared Rust chain
primitives crate; analytics subtraction of escrow; CI jobs for the bridge; batching multiple mints in
one transaction; automated gas top-up; an independent guardian key. None of these block the first
end-to-end flow.
