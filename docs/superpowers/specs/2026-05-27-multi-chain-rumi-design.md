# Multi-Chain Rumi: Architecture & Phasing

**Date**: 2026-05-27
**Status**: Draft (pending user review)
**Supersedes**: The unfinished Monad-prototype spec from the 2026-05-25 "Prototype Rumi-on-Monad hybrid" brainstorm. That work folds into Phase 1 of this document.
**Prior research**: 2026-05-24 "Chainfusion EVM support exploration" session.

## Context

Rumi today is an ICP-native CDP stablecoin. Users deposit ICP (and other Internet Computer collateral) into a vault on the `rumi_protocol_backend` canister, mint icUSD against it, and either hold or use icUSD on ICP. The protocol math, ledger, stability pool, AMM, and treasury all live on Internet Computer.

The goal of this design is to extend Rumi to support collateral and icUSD circulation across multiple chains supported by Chainfusion (Monad first as a sandbox, then Solana, then Ethereum and L2s as demand grows). The long-term user experience target is: "Deposit collateral on chain X, mint icUSD on chain Y, never touch ICP UI."

This document is the umbrella architecture. Per-phase implementation plans are produced separately when each phase is ready to start.

## Locks from brainstorm

These decisions are settled and inputs to the rest of the document.

1. **One mainline backend supports multi-chain collateral.** No per-chain canister forever. The existing `rumi_protocol_backend` extends to carry multi-chain machinery; the Monad-only sandbox canister from the prior brainstorm is reframed as a staging deploy of the same mainline wasm.
2. **One canonical icUSD accounting source on ICP.** The backend canister always knows total icUSD = sum across all chains. The ICP `icusd_ledger` shows ICP-side circulation only; the canonical "all chains" view is the backend's `chain_supplies` table.
3. **icUSD wrapping model: burn-and-mint.** icUSD circulates natively on each chain. No lock-and-mint bridge custody subaccount on ICP. Total supply across chains equals total protocol debt. Backend signs every authorized mint and observes every burn.
4. **Liquidation routing: hybrid (keeper market primary, SP backstop with DEX swap).** Keepers on the origin chain handle the vast majority of liquidations. SP fires only as backstop. When SP backstops, it triggers a canister-signed DEX swap on the origin chain and re-mints icUSD back into itself, never holding a persistent foreign-asset position.
5. **Vault chain affinity: one collateral chain per vault.** Vault IDs stay globally unique u64s. Each vault carries a `collateral_chain` field. Multi-chain users open multiple vaults.
6. **Cross-chain mint flow.** Vaults specify a `mint_destination = (chain_id, recipient_address)` at open/borrow time. Backend signs the icUSD mint on the chosen destination chain. Same primitive whether destination matches the collateral chain (v1) or differs (v2 dream).

## Section 1: Long-Term Architecture

### Topology

Hub-and-spoke. ICP backend is the canonical brain. Each foreign chain is a spoke holding:

- N per-user collateral custody addresses, each derived via tECDSA (EVM) or tEd25519 (Solana). Canister is the sole signer.
- 1 canister-controlled icUSD token contract (ERC-20 on EVM, SPL Token on Solana). Canister holds the sole minter/burner role.
- 1 liquidation router contract (can be folded into the icUSD contract for chain #1, separate contract from chain #2 onward for cleaner ergonomics).
- N keeper-facing entrypoints exposed by the router.

### On ICP (mainline canisters)

| Canister | Change |
|---|---|
| `rumi_protocol_backend` | Extend. New `chains/` module tree, per-chain configs, per-chain settlement queues, `chain_supplies` accounting table, multi-provider RPC configs. The existing single-chain code path becomes "chain_id = ICP, native collateral." |
| `icusd_ledger` | Unchanged. ICP-side ICRC-1/2 ledger. `icrc1_total_supply()` represents ICP-side circulation only; `get_global_icusd_supply()` on the backend is the canonical multi-chain view. |
| `rumi_stability_pool` | Extend. Stays icUSD-only per liquidation routing decision. Gains a `cross_chain_backstop` path that can sign DEX swaps on foreign chains when keepers do not bite. |
| `rumi_treasury`, `rumi_3pool`, `rumi_amm`, `liquidation_bot` | Unchanged. |
| `ic_siwe_provider` | New. EVM Sign-In-With-Ethereum, deterministic ICP principal derivation. |
| `ic_siws_provider` | New (Phase 3). Solana Sign-In-With-Solana equivalent. |

### Per foreign chain (minimal footprint)

- 1 × `IcUSD.sol` or `icusd_program` (ERC-20 / SPL Token, ~80-100 LOC) with canister as sole minter/burner
- 1 × `LiquidationRouter` (per-chain; can be omitted for chain #1 if the icUSD contract handles the keeper deposit/burn flow inline)
- N × user-derived collateral custody addresses (no contract, just owned addresses)

### Per-chain integration cost target

New chain after the abstractions are proven: ~1-2 weeks of work. Achieved via:

- `chains/<chain>/adapter.rs` implements a common `ChainAdapter` trait: `verify_deposit`, `sign_withdrawal`, `sign_mint`, `sign_burn`, `fetch_finality`, `observe_event`
- `chains/<chain>/contracts/` holds the Solidity/Rust source for that chain's icUSD + router
- `chains/<chain>/config.rs` carries RPC endpoints, finality depth, gas/fee strategy, chain ID
- Backend init registers the chain via `register_chain(ChainConfig)`. No schema changes per chain.

### Frontend

One app, wallet-first. Connectors: MetaMask (all EVM chains), Phantom (Solana), Internet Identity (ICP-native). Frontend detects the connected wallet, looks up which collateral chains it is compatible with, presents the right vault UI.

SIWE/SIWS handshake on first foreign-chain login derives a deterministic ICP principal. Foreign-chain users never see ICP UI; ICP calls are made by the frontend on their behalf using the derived principal.

Per-chain frontend modules at `vault_frontend/src/lib/chains/<chain>/` mirror the Rust adapter pattern: `{wallet, siwe, icusd, liquidate}.ts`.

## Section 2: Cross-Chain Data Flows

Every cross-chain write follows the same pattern: enqueue, Timer D picks up, tECDSA/tEd25519 sign, submit, confirm finality, update state. Every cross-chain read is: observe event at finality, update state. All cross-chain async funnels through one settlement queue abstraction per chain with shared retry, error, and idempotency logic.

### 1. SIWE/SIWS login

User signs a challenge with their EVM (MetaMask) or Solana (Phantom) wallet. The `ic_siwe_provider` or `ic_siws_provider` returns a deterministic ICP principal derived from the foreign-chain address. The frontend uses that principal for all backend calls. The foreign-chain user never sees ICP UI.

### 2. Get collateral deposit address

User clicks "deposit collateral X." Frontend calls `get_user_deposit_address(chain, user_principal)`. Backend derives a per-user custody address via tECDSA/tEd25519 (deterministic from principal + chain + nonce), returns it. User sends collateral to that address with their own wallet.

### 3. Deposit watch

Backend's `chains/<chain>/deposit_watch.rs` polls the chain via the configured RPC provider(s) for incoming transactions at known custody addresses. On finality (per-chain confirmation depth), credits the user's collateral balance in backend state. The UI surfaces a "pending → confirmed" status during the wait.

### 4. Open vault with mint destination

User specifies collateral amount, debt amount, and `mint_destination = (chain_id, recipient_address)`. Backend:
- Creates the vault entry with the specified `collateral_chain`
- Increments `total_debt` by the debt amount
- Increments `chain_supplies[mint_destination.chain]` by the same amount
- Enqueues a mint op on `settlement_queue[mint_destination.chain]`

Timer D signs and submits the foreign-chain mint transaction. User receives icUSD on the chosen chain. This is the same primitive whether the destination matches the collateral chain (v1) or differs (v2 dream).

### 5. Borrow more on existing vault

Same as flow 4 but on an existing vault. Backend re-checks CR before queuing the mint.

### 6. Repay

User burns icUSD on any registered chain by calling `IcUSD.burn(amount, target_vault_id)`. The contract emits a `Burn` event with vault_id metadata. Backend's observer watches the event, waits for finality, decrements `chain_supplies[source_chain]` by the amount, decrements the vault's debt by the same amount.

### 7. Withdraw collateral

User requests withdrawal. Backend verifies the vault is healthy (or fully repaid). Enqueues a transfer-out tx on `settlement_queue[collateral_chain]`. Timer D signs and submits.

### 8. Close vault

Repay all debt plus withdraw all collateral. Vault marked closed.

### 9. Cross-chain icUSD bridge (no vault interaction)

User on chain X wants icUSD on chain Y. Frontend has them sign a burn tx on chain X with metadata `{destination_chain: Y, destination_address: addr}`. Backend observes the burn at finality, decrements `chain_supplies[X]`, increments `chain_supplies[Y]`, enqueues mint on `settlement_queue[Y]`. Timer D signs the mint on Y. User receives icUSD on Y.

Net effect: `sum(chain_supplies)` unchanged, `total_debt` unchanged. Same primitive as flow 6 plus flow 4, with no vault involved. This is what enables keepers to move icUSD across chains for liquidations.

### 10. Liquidation, primary path (keeper market)

A keeper on chain X observes that vault V is liquidatable (oracle-derived CR below threshold). Keeper calls `LiquidationRouter.liquidate(V, debt_amount)` with icUSD they hold on X. The router burns the icUSD on X and emits a `LiquidationCompleted` event. Backend observes the event, signs a collateral transfer from the vault's custody address to the keeper's specified address on chain X, decrements `chain_supplies[X]` by the debt amount, decrements `total_debt`, marks the vault liquidated.

### 11. Liquidation, SP backstop

No keeper bites within the configured window (per-chain, e.g., 5 minutes). Backend triggers the SP fallback:

1. SP burns ICP-side icUSD equal to vault debt
2. Backend marks vault liquidated, transfers custody-address ownership of the collateral to a canister-controlled SP-collateral address on chain X
3. Backend signs a swap tx on a chain X DEX (Uniswap V3, Jupiter, etc.) to convert the collateral to a stable asset (chain-native USDC or USDT)
4. Backend signs a bridge-back tx to convert that stable asset to ICP-side icUSD (via ckUSDC or equivalent)
5. Backend mints the resulting icUSD back into the SP

SP's foreign-chain position lasts only the brief swap window. Never a persistent multi-asset book.

## Section 3: Operational Hardening

### Error categories and handling

| Category | Handling |
|---|---|
| RPC down (single provider) | Multi-provider consensus where available. For obscure chains, fall through to next provider after timeout. If all fail, queue retries with exponential backoff. UI surfaces a "chain X experiencing issues" banner. |
| tECDSA/tEd25519 signing failure | Retry next tick. After N failures, log + emit alert event, op stays in queue. No silent drop. |
| Tx submitted but stuck | Per-chain stuck-tx detector watches for txs that do not confirm within `finality_depth × 2`. On detection, bump gas (EVM) or resubmit with higher priority (Solana). |
| Foreign-chain tx reverted | Compensating action: reverse the in-flight state change. Vault op stays in `pending`, debt and supply not modified until tx confirms. UI shows op as failed, lets user retry. |
| Gas-out on canister hot wallet | Canister tracks hot wallet balance per chain. Below threshold, refuses new outbound ops on that chain (read ops still work). Admin top-up endpoint. No automatic top-up (avoids runaway drain). |
| Reorg on foreign chain | Wait for `finality_depth[chain]` confirmations before treating any observed event as committed. Reorg shorter than finality depth is invisible to backend. Reorg deeper than finality requires manual intervention (alerts, halt new ops on that chain). |
| State wipe on upgrade | Versioned snapshot pattern for ALL multi-chain state: `MultiChainStateV1`, `MultiChainStateV2`, etc. Add fields only via new version plus migration function. Never modify `Encode!/Decode!` structs in place. (Per the 2026-05-18 AMM state-wipe incident.) |
| Nonce mismatch / double-submit | Per-derivation-path serialization in settlement queue. One outbound tx in flight per custody address at a time. Idempotency keys on every queued op. |

### Supply invariant enforcement

The rule: `sum(chain_supplies) == total_debt` at all times.

Enforced two ways:

1. **In-process assertion.** Every state mutation that touches debt or supply runs through a single `apply_supply_delta(chain, delta)` function that maintains the invariant. If a caller violates it, the function traps. No mutation paths bypass it.
2. **Periodic self-check.** Runs on Timer B (the existing 60s interest/treasury timer). Computes `sum(chain_supplies)` and compares to `total_debt`. On mismatch: halts new debt issuance and minting across all chains, logs an emergency event, requires admin intervention.

External auditors can verify by querying `get_supply_audit()`, which returns the per-chain breakdown along with the canonical `totalSupply()` query URL for each chain's icUSD contract. Auditors check each chain independently and compare against the internal table. Full audit completes in ~5 minutes.

### Oracle strategy per chain

| Asset class | Primary source | Fallback |
|---|---|---|
| Major assets (ICP, ETH, BTC) | XRC | Pyth via EVM RPC |
| Chain-native (MON, SOL) | Pyth via EVM RPC (where Pyth feeds exist) | Manual admin override (gated) |
| Stables (USDC, USDT) | XRC | Hardcoded $1 with circuit breaker if depeg detected |

Per-collateral `min_xrc_sources` already exists (CDP-14 / XAUT precedent). Per-chain oracle staleness threshold is configurable. If an oracle is down for X minutes on chain Y, new debt issuance freezes on Y; repays and liquidations continue using the last-known price with a widened safety margin.

### Testing tiers

1. **Rust unit tests** for adapters, supply invariant logic, settlement queue, error paths.
2. **Foundry tests** (and Anchor tests for Solana) for the canister-controlled token and router contracts. Property tests for mint/burn invariants.
3. **Property tests** for the supply invariant specifically. Randomized cross-chain op sequences (mint X, burn Y, vault open, liquidate, etc.) asserting `sum(chain_supplies) == total_debt` after every op.
4. **Manual integration on staging canister plus chain testnets.** End-to-end vault flows, liquidation flow, bridge flow.
5. **PocketIC** for chain-agnostic backend logic only. Chain-specific async paths (tECDSA, HTTPS outcalls, foreign-chain txs) are explicitly out of PocketIC scope. They get manual integration coverage on staging. Accepted risk consistent with the prior Monad session decision.

## Section 4: Phasing

### Phase 0: icp-cli migration (~1 week)

- Identity import (`icp identity import rumi_identity --from-pem ...`)
- Canister IDs migrate to `.icp/data/mappings/<environment>.ids.json`
- Replace `dfx generate` with a `didc bind --target ts` script that mirrors the existing `declarations/` output format
- Author `icp.yaml` with `mainnet-live` and `mainnet-staging` environments, recipes for each quirky canister (3pool, SP, bot, backend init/upgrade-arg patterns)
- Update `.claude/hooks/pre-deploy-test.sh` to fire on `icp deploy`
- Migrate one canister at a time, lowest-risk first: `flaky_ledger` → `icusd_index` → `liquidation_bot` → `rumi_3pool` → `rumi_stability_pool` → `rumi_treasury` → `rumi_amm` → `rumi_analytics` → `rumi_protocol_backend` (last)

**Pre-flight spikes** (before committing the migration plan, two ~30-minute investigations):
1. Verify icp.yaml can express the "dummy init args on upgrade" pattern needed for 3pool, SP, bot
2. Verify icp-cli has a `deps pull` equivalent for `icp_ledger_canister`, `internet_identity`, and `xrc`, and that it preserves local-replica integration (XRC on system subnet specifically)

**Risks**:
- icp-cli is newer than dfx and may hit bugs. Mitigation: keep dfx installed during the migration as fallback for 2 weeks after completion.
- Upgrade-arg handling for quirky canisters requires verification (the pre-flight spike).
- Declarations regeneration is non-trivial (frontend depends on a specific `declarations/` output format).

### Phase 1: Multi-chain primitives + Monad (6-9 weeks)

Combined into one phase because the abstractions cannot be validated without a real first customer.

**Sub-phase 1a: Backend scaffolding (1-2 weeks).**
`chains/` module tree, `ChainAdapter` trait, `chain_supplies: HashMap<ChainId, u128>`, `get_global_icusd_supply()` query, per-chain settlement queue, admin endpoints (`register_chain`, `disable_chain`, `set_chain_config`). No real chain configured yet. Staging canister deploys but with chains map empty.

**Sub-phase 1b: Monad adapter and happy path (3-4 weeks).**
- Implement Monad adapter (deposit watch, settlement, admin)
- Deploy `IcUSD.sol` to Monad testnet via Foundry
- Deploy fresh `multichain_rumi_test` canister on ICP mainnet with Monad enabled via config
- Wire up deposit → borrow → repay → close end-to-end on staging
- Backend's production canister is untouched throughout (same wasm, Monad not registered there)

**Sub-phase 1c: Liquidations and bridge primitive (2-3 weeks).**
- SP backstop path with canister-signed Uniswap V3 swap on Monad
- Keeper market entry: deploy `LiquidationRouter.sol` (or fold into IcUSD.sol for Monad's simplicity)
- Cross-chain icUSD bridge primitive (burn on X, mint on Y) implemented and tested
- Supply invariant property tests covering the bridge flow

**Sub-phase 1d: Frontend (2 weeks, parallel with 1c).**
- New `/monad-vaults` route
- MetaMask connector via viem
- SIWE login via `ic-siwe-js`
- Vault UI for Monad-collateral vaults
- icUSD bridge UI (move icUSD between chains)
- Route hidden from main nav initially (only accessible by direct URL)

### Phase 2: Monad production rollout (1-2 weeks)

Flip Monad on the production `rumi_protocol_backend` via admin call. Frontend `/monad-vaults` route becomes user-visible (linked from main nav). Monitor closely for the first week.

### Phase 3: Second chain, Solana (2-3 weeks)

The cycle-time test: how fast can chain #2 be added once the pattern is proven? Solana adapter using tEd25519, SPL Token instead of ERC-20, SIWS provider, Anchor program for the icUSD token, Jupiter for SP backstop swaps. If Phase 1 abstractions are right, this should be ~1/3 the effort of Phase 1.

### Phase 4: Cross-chain UX dream (2-3 weeks)

"Deposit on Monad, mint icUSD on Solana." Vault open UI lets the user select a mint destination chain that differs from the collateral chain. Backend signs the mint on the chosen chain. Same primitive for repay (burn icUSD on any chain, decrement debt regardless of where the vault lives).

### Phase 5 and beyond: Additional chains on demand

Ethereum mainnet, Base, Arbitrum, ckBTC integration, etc. Each ~1-2 weeks given the template is solid. Prioritization driven by demand.

### Spec detail level per phase

| Phase | Detail in this document |
|---|---|
| Phase 0 | Full spec above. Implementation plan produced separately when work starts. |
| Phase 1 | Full spec above (architecture, data flows, error handling, sub-phasing). Implementation plan produced before Phase 1 starts. |
| Phase 2 | Outline above. Detailed plan locked when Phase 1 nears completion. |
| Phase 3 | Outline above. Detailed plan locked when Phase 2 ships. |
| Phase 4 | Acknowledged direction. Detailed design locked after Phase 3. |
| Phase 5+ | Mentioned. No detailed planning. |

## Section 5: Open Questions and Non-Decisions

These are explicitly NOT decided in this document and are deferred to implementation plans:

- **Per-chain finality depth.** Configured per chain at registration. Monad: TBD based on testnet observations (single-slot finality likely means depth = 1, verify on testnet first). Solana: depth = "finalized" commitment. EVM mainnet: depth = 12 blocks.
- **Per-chain keeper liquidation discount %.** Likely 5-10% but exact value tuned per chain based on observed keeper participation.
- **SP backstop timeout window per chain.** Likely 5 minutes for fast chains, longer for chains with worse keeper coverage.
- **Specific oracle feed addresses per asset per chain.** Locked in chain registration config, not architectural.
- **Specific DEX router contract addresses** for SP backstop swaps. Locked in chain config.
- **Frontend UI design and mockups for `/monad-vaults` and beyond.** Separate visual design work, not architectural.
- **Cycle budget per chain.** How many cycles per second the backend allocates to each chain's deposit_watch + settlement_queue. Tunable per chain.
- **Whether to expose a public `bridge_icusd` flow in v1 or gate it behind Phase 4.** Lean toward exposing in Phase 1c (keepers need it anyway, and exposing it to users is free).
- **Cycle wallet pattern for canister-funded foreign-chain gas.** TBD how the canister tops up its Monad/Solana hot wallets. Manual admin top-up for v1; automated top-up via ICP→cycles→bridge later if needed.

## Acceptance criteria for this design

This spec is considered complete and ready to hand off to writing-plans when:

1. User has reviewed and approved (or requested changes integrated)
2. The four foundational locks (icUSD wrapping, liquidation routing, vault chain affinity, cross-chain mint flow) are clearly documented and not ambiguous
3. Phase 0 has enough detail that an implementation plan can be written from it without further architectural decisions
4. Phase 1 has enough detail at the sub-phase level that work can begin once Phase 0 ships

Next step after approval: invoke the `writing-plans` skill to produce the Phase 0 implementation plan.
