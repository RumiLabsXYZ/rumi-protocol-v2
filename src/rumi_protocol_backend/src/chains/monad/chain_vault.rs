//! Monad vault helpers - thin wrappers over the chain-agnostic state machine.
//!
//! The vault record, error types, CR math, and the open/verify/withdraw/close
//! state machine were hoisted to `crate::chains::vault` (Task F3) so Solana
//! (Task 6) can reuse them. This module now:
//!
//! - RE-EXPORTS the types (`ChainVaultStatus`, `ChainVaultV1`, `OpenVaultError`,
//!   `WithdrawError`) and the two already-chain-agnostic helpers
//!   (`collateral_ratio_e4`, `verify_deposit_and_enqueue_mint_in_state`) so every
//!   existing path `crate::chains::monad::chain_vault::ChainVaultV1` (etc.) keeps
//!   resolving unchanged. State + candid representation are therefore identical.
//! - WRAPS the three address/price-aware helpers
//!   (`open_chain_vault_in_state`, `withdraw_collateral_in_state`,
//!   `close_chain_vault_in_state`) with their ORIGINAL signatures, baking in the
//!   Monad specifics: the EVM address validator
//!   (`tecdsa::is_valid_evm_address`) and the `"MON"` manual-price key. Monad
//!   runtime behavior is byte-identical to before the hoist.
//!
//! Keeps the Monad-only `MONAD_MIN_CR_E4` constant local.

use crate::chains::config::ChainId;
use crate::chains::multi_chain_state::MultiChainStateV4;
use candid::Principal;

// Re-export the chain-agnostic types + un-parameterized helpers. Every existing
// caller and test that names `crate::chains::monad::chain_vault::{...}` or
// `super::chain_vault::{...}` resolves through these, so nothing downstream
// changes.
pub use crate::chains::vault::{
    collateral_ratio_e4, verify_deposit_and_enqueue_mint_in_state, ChainVaultStatus, ChainVaultV1,
    OpenVaultError, WithdrawError,
};

/// Minimum collateral ratio (e4: 13000 == 130.00%) required to open a Monad
/// chain vault. Checked against DECLARED collateral at open time. Per-collateral
/// configurability is a later refinement (Phase 2 unifies the foreign-chain and
/// ICP-native CDP parameter models).
pub const MONAD_MIN_CR_E4: u64 = 13_000;

/// Open a Monad chain vault (open-then-verify). Thin wrapper over
/// `crate::chains::vault::open_chain_vault_in_state` with the Monad EVM address
/// validator and the `"MON"` price key baked in. Signature, behavior, and error
/// surface are identical to the pre-hoist Monad helper.
#[allow(clippy::too_many_arguments)]
pub fn open_chain_vault_in_state(
    state: &mut MultiChainStateV4,
    chain: ChainId,
    owner: Principal,
    custody_address: String,
    collateral_e18: u128,
    debt_e8s: u128,
    mint_recipient: String,
    min_cr_e4: u64,
    now_ns: u64,
    vault_id: u64,
) -> Result<(), OpenVaultError> {
    crate::chains::vault::open_chain_vault_in_state(
        state,
        chain,
        owner,
        custody_address,
        collateral_e18,
        debt_e8s,
        mint_recipient,
        crate::chains::monad::tecdsa::is_valid_evm_address,
        "MON",
        min_cr_e4,
        now_ns,
        vault_id,
    )
}

/// Withdraw Monad collateral. Thin wrapper over
/// `crate::chains::vault::withdraw_collateral_in_state` with the Monad EVM
/// address validator and the `"MON"` price key baked in. Signature, behavior,
/// and error surface are identical to the pre-hoist Monad helper.
pub fn withdraw_collateral_in_state(
    state: &mut MultiChainStateV4,
    vault_id: u64,
    amount_e18: u128,
    dest_address: String,
    min_cr_e4: u64,
    now_ns: u64,
) -> Result<(), WithdrawError> {
    crate::chains::vault::withdraw_collateral_in_state(
        state,
        vault_id,
        amount_e18,
        dest_address,
        crate::chains::monad::tecdsa::is_valid_evm_address,
        "MON",
        min_cr_e4,
        now_ns,
    )
}

/// Close a debt-free Monad vault. Thin wrapper over
/// `crate::chains::vault::close_chain_vault_in_state` with the Monad EVM address
/// validator and the `"MON"` price key baked in. Signature, behavior, and error
/// surface are identical to the pre-hoist Monad helper.
pub fn close_chain_vault_in_state(
    state: &mut MultiChainStateV4,
    vault_id: u64,
    dest_address: String,
    min_cr_e4: u64,
    now_ns: u64,
) -> Result<(), WithdrawError> {
    crate::chains::vault::close_chain_vault_in_state(
        state,
        vault_id,
        dest_address,
        crate::chains::monad::tecdsa::is_valid_evm_address,
        "MON",
        min_cr_e4,
        now_ns,
    )
}
