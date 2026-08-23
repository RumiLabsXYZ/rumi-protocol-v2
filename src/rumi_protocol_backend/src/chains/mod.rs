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
//! ── Authoritative Conflux-mainnet public risk gate ─────────────────────────
//! `open_chain_vault_evm`, `borrow_chain_vault_evm`, debt-bearing signed
//! withdrawals, deposit verification, and chain-1030 Mint/InterestMint submit
//! all reuse the same bounded
//! predicate that backs `get_chain_public_launch_status`. For chain 1030 it
//! fails closed unless the reviewed IcUSD contract, exact finality, adequate
//! RPC agreement, enabled liquidation, fresh price, clear breakers, seeded
//! burn cursor and fresh proven hot-wallet gas balance are all present. It also
//! pins the 0.10/500 icUSD debt limits and the exact reviewed Swappi route/risk
//! row. Open checks
//! before nonce consumption and again after its tECDSA await before insertion;
//! Borrow and debt-bearing withdrawal check inside their synchronous state
//! mutations. Debt-free withdrawal/Close/repay remain risk-reducing paths.
//! Other chains retain their existing gates.
//!
//! ── ONE PRICE WRITER (`xrc::pair_is_xrc_managed`) ──────────────────────────
//! A chain that is `Registered` AND carries a `chain_liquidation_configs` row
//! (regardless of `enabled`) is "XRC-managed". While it is, the automatic XRC
//! price timer is AUTHORITATIVE and the SOLE writer of that chain's
//! native-symbol price: `set_manual_collateral_price` REJECTS a write to that
//! pair for EVERY caller, the narrowly-scoped price pusher and the
//! developer alike. The race this closes is writer-vs-writer on one
//! `MultiChainState::set_manual_price` cell, not operator-vs-operator: the
//! timer fires from its own message on its own schedule, with no ordering
//! relationship to an ingress call, so a manual write can be silently
//! overwritten seconds later or can overwrite a fresher automatic sample.
//! The (superseded) off-chain CFX price monitor is a decommissioned,
//! emergency-only fallback now.
//!
//! The automatic writer is itself constrained (`xrc::chains_price_sample_is_acceptable`):
//! it stamps the sample's SOURCE timestamp rather than arrival time, requires
//! each candidate to be strictly newer at the source than the stored sample,
//! and holds it inside the accepted audit LIQ-007 `PRICE_SANITY_BAND_RATIO`
//! band relative to the last accepted price. A rejected sample writes NOTHING
//! (not even the freshness timestamp), and there is deliberately no
//! consecutive-confirmation escalation, so a sustained out-of-band move stays
//! FAIL-CLOSED until an operator intervenes.
//!
//! CYCLE COST NOTE: staging a `chain_liquidation_configs` row starts the XRC
//! meter immediately (~1B cycles/call at 300s = roughly 288B cycles/day per
//! symbol), the moment the row is INSERTED, not when `enabled` is later
//! flipped true. Budget for that at staging time, not at liquidation go-live.
//!
//! ── `disable_chain` / `enable_chain`: a REVERSIBLE HARD FREEZE ────────────
//! Flips `ChainStatus` to `Disabled`. Verified effects, reading every gate
//! that reads `ChainStatus`/`chain_is_registered`, not assumed:
//!  - Blocks new opens/borrows (item 2 above) and the XRC price timer
//!    (`chains_needing_price_feed` reads the same predicate), which also
//!    re-opens manual pricing for that pair (see the XRC-managed invariant).
//!  - ALSO stops the observer and settlement workers for that chain:
//!    `run_all_observers`/`run_all_settlements` (main.rs) and their legacy
//!    per-chain equivalents `observer_tick`/`settlement_tick`
//!    (deposit_watch.rs, settlement.rs) all filter their chain list to
//!    `ChainStatus::Registered`. Each worker also rechecks current status after
//!    RPC awaits before a deposit transition or immediately before signing and
//!    broadcast, so an already-suspended continuation cannot escape a Disable.
//!    A Disabled chain gets NO deposit
//!    verification, NO liquidation detection (that runs inside the
//!    observer), and NO settlement-queue draining (mints, withdrawals,
//!    liquidation swaps) at all.
//!  - Composite consequence for an ALREADY-OPEN vault: debt-bearing public
//!    withdrawal is refused by the risk gate. Debt-free signed withdrawal/Close
//!    (and policy-neutral admin helpers) can still enqueue an exit, but with the
//!    settlement worker frozen that op is not broadcast until re-enable. The one flow that genuinely
//!    completes on a Disabled chain is repay via `submit_burn_proof`
//!    (chains/evm/burn_proof.rs), which reads only the bound contract and
//!    finality depth, no `ChainStatus` check at all.
//!  - RECOVERY (`enable_chain`, added by this PR): the freeze is REVERSIBLE.
//!    `enable_chain` is developer-gated, flips ONLY `Disabled -> Registered`,
//!    refuses an unknown chain and an already-`Registered` one, adds no
//!    persisted field, and preserves every per-chain state entry (vaults,
//!    supply, contract binding, observer cursor, prices and their timestamps,
//!    liquidation config). Re-enabling puts the chain back in
//!    `registered_chains_and_solana_flag`'s list, so the observer and
//!    settlement workers resume and any exit enqueued while Disabled drains
//!    normally instead of stranding. Before it existed there was no
//!    `Disabled -> Registered` transition at all: `UpdateChainConfigArg` has
//!    no status field, `register_chain` refuses an already-present `chain_id`
//!    (`ChainAlreadyRegistered`), and `delete_chain` refuses while the chain
//!    carries any vault or nonzero supply, so a pending exit could neither
//!    complete (worker gated off) nor be cleared (`delete_chain` blocked by
//!    the very vault it would need to remove).
//!  - It is still a FREEZE while it lasts, not a graceful wind-down: exits
//!    enqueued on a Disabled chain sit until `enable_chain` runs. Keep the
//!    window short and deliberate. Whether workers should keep draining exits
//!    on a Disabled chain is a separate open design question; this file
//!    states current behavior, not a recommendation.
//!
//! MANUAL REBASELINE IS ONLY AVAILABLE WHILE DISABLED. The whole operator
//! loop is: `disable_chain` (risk stop, XRC stop, worker freeze),
//! `set_manual_collateral_price` (rebaseline), read it back and VERIFY,
//! `enable_chain` (XRC and the workers resume). Enable reopens the GATE only;
//! it does not vouch for price freshness, so an open after enable still has to
//! satisfy the price presence/staleness checks, which is exactly why the
//! verify step is part of the loop.
//!
//! ── Go-live checklist for the first prod chain (Conflux, chain id 1030) ────
//!  1. `set_chains_ecdsa_key_name("key_1")` BEFORE the first prod chain
//!     vault opens (the key locks in at first vault use). VERIFY via a
//!     POST-UPGRADE read-back of `get_chains_ecdsa_key_name`. Do not assume
//!     a value from before this upgrade; the address-cache bug above means a
//!     pre-upgrade key report could have been stale.
//!  2. AFTER that read-back, re-derive and verify the settlement and reserve
//!     addresses (`get_chain_settlement_address`, `get_chain_reserve_address`)
//!     and confirm `get_evm_rpc_principal` reports the official EVM-RPC
//!     canister (no override left pointing at a mock), ALL of it BEFORE any
//!     immutable on-chain contract deployment bakes an address in.
//!  3. Register chain 1030 with MAINNET-shaped args
//!     (`conflux_mainnet_register_arg`): finality_depth 400,
//!     min_quorum_providers 2, operator-vetted RPC URLs.
//!  4. Set the native price and seed the observer cursor while the chain is
//!     still unbound (and therefore not publicly openable).
//!  5. Deploy IcUSD.sol on eSpace mainnet, then `set_chain_contract`. Binding
//!     is necessary but not sufficient: public Open/Borrow stays closed until
//!     every condition in the authoritative risk gate above is clear.
//!  6. Stage a liquidation config row (real Swappi router, fee/divergence/
//!     deadline; `enabled` starts false). This hands pricing to the automatic
//!     XRC feed AND claims the pair from manual pricing (see ONE PRICE WRITER
//!     above); budget cycles from this moment.
//!  7. Fund nothing: the reserve address is a sink; the custody address pays
//!     gas from deposited collateral.
//!  8. Liquidation is a SEPARATE switch: flip the liquidation config's
//!     `enabled` to true only after its own on-chain validation
//!     (`set_chain_liquidation_config` re-derives the factory pair on
//!     enable). Staging the row (step 6) does not enable liquidation.
//!
//! Monad and Solana remain genuinely experimental: testnet/devnet only,
//! observer/settlement timers off by default (Solana also gated behind
//! `solana_workers_enabled`), and their write paths are still developer-
//! gated pending the same kind of review pass Conflux already had.

pub mod adapter;
pub mod admin;
pub mod collateral_config;
pub mod config;
pub mod evm;
pub mod interest;
pub mod liquidation;
pub mod liquidation_config;
pub mod monad;
pub mod multi_chain_state;
pub mod recovery;
pub mod settlement_queue;
pub mod solana;
pub mod supply;
pub mod vault;
pub mod xrp;

pub use adapter::ChainAdapter;
pub use config::{ChainConfig, ChainId, ChainStatus};
pub use liquidation_config::{ChainLiquidationConfigV1, DexKind, LiquidationConfigError};
pub use multi_chain_state::{
    MultiChainState, MultiChainStateV1, MultiChainStateV2, MultiChainStateV3, MultiChainStateV4,
    MultiChainStateV5, MultiChainStateV6, MultiChainStateV7,
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
