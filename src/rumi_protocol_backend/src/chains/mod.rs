//! STATUS (2026-08-20): the Conflux (CFX/eSpace) chains-liquidation rail is
//! code-complete and dormant, not experimental scaffolding. Increments 0-13
//! (PRs #261-#286, June 2026) built the full engine: EIP-712 vault auth,
//! observer/settlement workers, the liquidation bot path, and the SP
//! escalation (`stability_pool_liquidate_chain_vault`). It is dormant purely
//! because prod carries no per-chain config row.
//!
//! DEPLOY NOTE: this PR also fixes a live stale-cache bug in the
//! settlement/interest-treasury/reserve address derivation
//! (chains/evm/tecdsa.rs). Do NOT trust any chain address read from prod
//! before this upgrade lands: a pre-rotation cache entry may still be warm.
//!
//! ── The actual public-open gate (verified by reading `verify_intent_ctx` +
//! `open_chain_vault_in_state` end to end, not assumed) ──────────────────────
//! `open_chain_vault_evm`'s checks run in this order:
//!  1. `verify_intent_ctx` (main.rs) resolves the chain's bound
//!     `chain_contracts` entry BEFORE it ever verifies the EIP-712 signature
//!     (the contract address is the domain separator). An unbound chain
//!     rejects with "no contract set", on ANY signature, regardless of
//!     registration or price state. This is why binding the IcUSD contract
//!     (`set_chain_contract`) is the step that actually makes a chain
//!     publicly open, not registration.
//!  2. `open_chain_vault_in_state` (chains/vault.rs) then requires
//!     `MultiChainState::chain_is_registered` (`ChainStatus::Registered`).
//!     This check runs in the SYNCHRONOUS post-`.await` half of the open
//!     call (after the tECDSA custody-address derive resolves), so a
//!     `disable_chain` landing while that derive is in flight is still
//!     caught on resume. `borrow_chain_vault_evm` (fully synchronous, no
//!     `.await`) reads the same predicate. Withdraw/close/repay do NOT read
//!     it and their pure state helpers still ACCEPT the call on a Disabled
//!     chain, but see the `disable_chain` section below: accepting the call
//!     is not the same as completing it, since the worker that would
//!     actually broadcast the resulting withdrawal is also gated off.
//!  3. A native-asset price must be present (`manual_prices`); it is only
//!     STALENESS-CHECKED once a `chain_liquidation_configs` row exists with
//!     `max_price_age_ns > 0` (`gated_chain_price_e8`, chains/vault.rs),
//!     independent of that row's `enabled` flag, which gates the
//!     liquidation-swap WORKER only, never the open/borrow price check.
//!
//! ── XRC-managed pricing invariant (`xrc::chain_is_xrc_managed`) ────────────
//! A chain that is `Registered` AND carries a `chain_liquidation_configs` row
//! (regardless of `enabled`) is "XRC-managed": the XRC price timer is the
//! SOLE writer of that chain's native-symbol price FOR THE PRICE-PUSHER.
//! `set_manual_collateral_price` REJECTS a pusher write to that pair, to
//! avoid a last-writer-wins race between the pusher and the timer; the
//! developer principal retains a manual override (a single trusted operator
//! cannot race itself, and needs override for dry-run/staging price control
//! and deterministic liquidation-bot testing). The (superseded) off-chain
//! CFX price monitor is a decommissioned/emergency-only fallback now if it
//! runs as the pusher: it can only push a price again after `disable_chain`
//! (see below) makes the pair manual for the pusher too; run as the
//! developer principal it is unaffected by this invariant.
//!
//! CYCLE COST NOTE: staging a `chain_liquidation_configs` row starts the XRC
//! meter immediately (~1B cycles/call at 300s = roughly 288B cycles/day per
//! symbol), the moment the row is INSERTED, not when `enabled` is later
//! flipped true. Budget for that at staging time, not at liquidation go-live.
//!
//! ── `disable_chain`: a HARD FREEZE, not a graceful risk-only stop ──────────
//! Flips `ChainStatus` to `Disabled`. Verified effects, reading every gate
//! that reads `ChainStatus`/`chain_is_registered`, not assumed:
//!  - Blocks new opens/borrows (item 2 above) and the XRC price timer
//!    (`chains_needing_price_feed` reads the same predicate), which also
//!    re-opens manual pricing for that pair (see the XRC-managed invariant).
//!  - ALSO stops the observer and settlement workers for that chain:
//!    `run_all_observers`/`run_all_settlements` (main.rs) and their legacy
//!    per-chain equivalents `observer_tick`/`settlement_tick`
//!    (deposit_watch.rs, settlement.rs) all filter their chain list to
//!    `ChainStatus::Registered`. A Disabled chain gets NO deposit
//!    verification, NO liquidation detection (that runs inside the
//!    observer), and NO settlement-queue draining (mints, withdrawals,
//!    liquidation swaps) at all.
//!  - Composite consequence for an ALREADY-OPEN vault: `withdraw_collateral_in_state`/
//!    `close_chain_vault_in_state` do not read `ChainStatus` and still
//!    ACCEPT the call, enqueueing a settlement op, but with the settlement
//!    worker gated off that op is never broadcast: the vault sticks
//!    mid-exit (e.g. `Closing`) indefinitely. The one flow that genuinely
//!    completes on a Disabled chain is repay via `submit_burn_proof`
//!    (chains/evm/burn_proof.rs), which reads only the bound contract and
//!    finality depth, no `ChainStatus` check at all.
//!  - RECOVERY: there is currently NO `Disabled` -> `Registered` transition.
//!    `UpdateChainConfigArg` has no status field and there is no
//!    `enable_chain` endpoint. `register_chain` refuses an already-present
//!    `chain_id` (`ChainAlreadyRegistered`), and `delete_chain` refuses while
//!    the chain carries any vault or nonzero supply. A pending exit enqueued
//!    before or during `disable_chain` therefore STRANDS: it cannot complete
//!    (worker gated off) and cannot be cleared (`delete_chain` blocked by the
//!    very vault it would need to remove) until chain-recovery tooling exists.
//!  - Treat `disable_chain` as a genuine last resort, not a routine pause: it
//!    is closer to "brick this chain until further tooling ships" than
//!    "pause new risk, let existing positions wind down." Whether workers
//!    should keep draining exits on a Disabled chain is an open design
//!    question, tracked separately; this file states current behavior, not
//!    a recommendation.
//!
//! ── Go-live checklist for the first prod chain (Conflux, chain id 1030) ────
//!  1. `set_chains_ecdsa_key_name("key_1")` BEFORE the first prod chain
//!     vault opens (the key locks in at first vault use). VERIFY via a
//!     POST-UPGRADE read-back of `get_chains_ecdsa_key_name`. Do not assume
//!     a value from before this upgrade; the address-cache bug above means a
//!     pre-upgrade key report could have been stale.
//!  2. AFTER that read-back, re-derive and verify the settlement and reserve
//!     addresses (`get_chain_settlement_address`, `get_chain_reserve_address`)
//!     BEFORE any immutable on-chain contract deployment bakes an address in.
//!  3. Register chain 1030 with MAINNET-shaped args
//!     (`conflux_mainnet_register_arg`): finality_depth 400,
//!     min_quorum_providers 2, operator-vetted RPC URLs.
//!  4. Deploy IcUSD.sol on eSpace mainnet, then `set_chain_contract`. This
//!     is the actual "make public" step (see the public-open gate above).
//!  5. Stage a liquidation config row (real Swappi router, fee/divergence/
//!     deadline; `enabled` can start false). This activates the XRC price
//!     timer for the chain AND claims the pair from manual pricing (see the
//!     XRC-managed invariant above); budget cycles from this moment.
//!  6. Verify `get_evm_rpc_principal` reports the official EVM-RPC canister
//!     (no override left pointing at a mock).
//!  7. Fund nothing: the reserve address is a sink; the custody address pays
//!     gas from deposited collateral.
//!  8. Flip the liquidation config's `enabled` to true only after its own
//!     on-chain validation (`set_chain_liquidation_config` re-derives the
//!     factory pair on enable).
//!
//! Monad and Solana remain genuinely experimental: testnet/devnet only,
//! observer/settlement timers off by default (Solana also gated behind
//! `solana_workers_enabled`), and their write paths are still developer-
//! gated pending the same kind of review pass Conflux already had.

pub mod adapter;
pub mod admin;
pub mod collateral_config;
pub mod config;
pub mod interest;
pub mod liquidation;
pub mod liquidation_config;
pub mod multi_chain_state;
pub mod recovery;
pub mod settlement_queue;
pub mod supply;
pub mod vault;
pub mod evm;
pub mod monad;
pub mod solana;
pub mod xrp;

pub use adapter::ChainAdapter;
pub use config::{ChainConfig, ChainId, ChainStatus};
pub use liquidation_config::{ChainLiquidationConfigV1, DexKind, LiquidationConfigError};
pub use multi_chain_state::{
    MultiChainState, MultiChainStateV1, MultiChainStateV2, MultiChainStateV3, MultiChainStateV4,
    MultiChainStateV5, MultiChainStateV6,
};
pub use settlement_queue::{SettlementOp, SettlementQueueV1};
pub use supply::{apply_supply_delta, SupplyDelta, SupplyInvariantError};

#[cfg(test)]
mod tests_adapter;

#[cfg(test)]
mod tests_config;

#[cfg(test)]
mod tests_settlement_queue;

#[cfg(test)]
mod tests_multi_chain_state;

#[cfg(test)]
mod tests_multi_chain_state_v2;

#[cfg(test)]
mod tests_supply;

#[cfg(test)]
mod tests_admin;

#[cfg(test)]
mod tests_recovery;

#[cfg(test)]
mod tests_self_check;

#[cfg(test)]
mod tests_vault;
