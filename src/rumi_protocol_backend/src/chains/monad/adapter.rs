//! `MonadAdapter`: Phase 1b implementation of `ChainAdapter` for Monad testnet.
//!
//! Wires the tested tECDSA, EVM RPC, and EIP-1559 primitives from this module
//! into the six `ChainAdapter` trait methods required by the protocol.
//!
//! ## Signing model
//! The settlement key (one per chain, derived via `settlement_derivation_path`)
//! acts as the on-chain minter/relayer.  Phase 1b only needs to *build and sign*
//! transactions; broadcasting is handled by the settlement worker (Task 13).
//!
//! ## Phase scope
//! - `sign_mint`: builds + signs the `mint(address,uint256,uint64)` call on the
//!   icUSD EVM contract.
//! - `sign_withdrawal`: builds + signs a native MON transfer to the recipient.
//! - `sign_burn`: reserved; Phase 1b burns are user-initiated on-chain.
//! - `observe_event`: delegated to the dedicated observer (Task 9).
//! - `verify_deposit` / `fetch_finality`: use EVM RPC reads.

use async_trait::async_trait;
use candid::Principal;

use crate::chains::adapter::{
    ChainAdapter, ChainAdapterError, DepositRecord, FinalitySnapshot, MintInstruction,
    SignedBurn, SignedMint, SignedWithdrawal, WithdrawalRequest,
};
use crate::chains::config::ChainId;
use crate::state::read_state;

use super::{evm_rpc, tecdsa, tx};

// ─── MonadAdapter ────────────────────────────────────────────────────────────

/// Adapter binding the Monad chain to the Rumi protocol.
pub struct MonadAdapter {
    chain_id: ChainId,
}

impl MonadAdapter {
    /// Create a new `MonadAdapter` for the given chain id.
    ///
    /// In production this is always `MONAD_CHAIN_ID` (10143); tests may pass an
    /// arbitrary id for isolation.
    pub fn new(chain_id: ChainId) -> Self {
        MonadAdapter { chain_id }
    }

    // ─── private helpers ─────────────────────────────────────────────────────

    /// Return the deployed icUSD contract address for this chain, or an error
    /// if it has not yet been registered via `set_chain_contract`.
    fn icusd_contract(&self) -> Result<String, ChainAdapterError> {
        read_state(|s| s.multi_chain.chain_contracts.get(&self.chain_id).cloned())
            .ok_or_else(|| ChainAdapterError::InvalidPayload("icUSD contract not set".to_string()))
    }

    /// Fetch (base_fee_wei, priority_fee_wei) from the EVM RPC canister.
    async fn build_fees(&self) -> Result<(u128, u128), ChainAdapterError> {
        evm_rpc::fetch_fees(self.chain_id)
            .await
            .map_err(|message| ChainAdapterError::RpcError {
                provider: "evm_rpc".to_string(),
                message,
            })
    }
}

// ─── ChainAdapter impl ────────────────────────────────────────────────────────

#[async_trait(?Send)]
impl ChainAdapter for MonadAdapter {
    fn chain_id(&self) -> ChainId {
        self.chain_id
    }

    /// Check whether `tx_hash` has been mined and succeeded on-chain.
    ///
    /// Returns a `DepositRecord` with `block_number` populated when mined.
    /// `amount_e8s` and `depositor` are left at defaults (0 / empty string) —
    /// the observer (Task 9) decodes those from the event log with full ABI
    /// context; `verify_deposit` is used by the vault path only to confirm
    /// finality, not to parse amounts.
    async fn verify_deposit(&self, tx_hash: &str) -> Result<DepositRecord, ChainAdapterError> {
        match evm_rpc::get_transaction_receipt(self.chain_id, tx_hash).await {
            Ok(Some((true, block_number))) => Ok(DepositRecord {
                depositor: String::new(),
                amount_e8s: 0,
                block_number,
                tx_hash: tx_hash.to_string(),
            }),
            Ok(Some((false, _))) => Err(ChainAdapterError::InvalidPayload(
                "deposit transaction reverted".to_string(),
            )),
            Ok(None) => Err(ChainAdapterError::InvalidPayload(
                "deposit not mined".to_string(),
            )),
            Err(message) => Err(ChainAdapterError::RpcError {
                provider: "evm_rpc".to_string(),
                message,
            }),
        }
    }

    /// Build and sign a native MON transfer to `req.recipient`.
    ///
    /// # Amount note
    /// `WithdrawalRequest.amount_e8s` carries the MON amount in its native
    /// denomination (e18 wei for MON) as a known field-name wart.  Task 13
    /// refines this with a `NativeWithdrawal` variant that makes the unit
    /// explicit.  For now the value is passed through unchanged as the EIP-1559
    /// `value` field, which the EVM interprets as wei.
    async fn sign_withdrawal(
        &self,
        req: WithdrawalRequest,
    ) -> Result<SignedWithdrawal, ChainAdapterError> {
        // 1. Derive the settlement (minter) address.
        let path = tecdsa::settlement_derivation_path(self.chain_id);
        let (_, settlement_addr) = tecdsa::derive_evm_address(path.clone())
            .await
            .map_err(ChainAdapterError::SignatureFailed)?;

        // 2. Fetch nonce for the settlement account.
        let nonce = evm_rpc::get_transaction_count(self.chain_id, &settlement_addr)
            .await
            .map_err(|message| ChainAdapterError::RpcError {
                provider: "evm_rpc".to_string(),
                message,
            })?;

        // 3. Fetch fee estimates.
        let (base_fee, prio) = self.build_fees().await?;

        // 4. Build EIP-1559 fields for a plain native transfer via the shared
        //    builder (tx::build_eip1559_fields) — the single source of truth for
        //    the per-op-kind field shape, so the settlement worker's submit and a
        //    replace-by-fee resubmit build byte-identical transactions.
        //    gas_limit = 21_000 (standard ETH/EVM native transfer).
        //    max_fee = 2 * base_fee + priority_fee (headroom for base-fee spikes).
        //
        //    NOTE: `value` carries req.amount_e8s, which is the MON amount in
        //    wei (e18).  The field name is a known wart; see doc comment above.
        let max_fee = base_fee.saturating_mul(2).saturating_add(prio);
        let fields = tx::build_eip1559_fields(
            self.chain_id.0 as u64,
            tx::MonadTxKind::NativeWithdrawal {
                recipient: &req.recipient,
                amount_wei: req.amount_e8s, // wart: see doc comment above
            },
            nonce,
            prio,
            max_fee,
        );

        // 5. Sign.
        let raw = tx::sign_eip1559(&fields, path, &settlement_addr)
            .await
            .map_err(ChainAdapterError::SignatureFailed)?;

        Ok(SignedWithdrawal {
            raw_tx: raw.into_bytes(),
            tx_hash: String::new(), // populated by the broadcaster (Task 13)
        })
    }

    /// Build and sign a `mint(address,uint256,uint64)` call on the icUSD contract.
    ///
    /// gas_limit = 120_000 — generous for a typical ERC-20-style mint call,
    /// covers the vault-id event emission and any SSTORE on first-mint paths.
    async fn sign_mint(&self, instr: MintInstruction) -> Result<SignedMint, ChainAdapterError> {
        // 1. Resolve the icUSD contract.
        let contract = self.icusd_contract()?;

        // 2. Derive the settlement (minter) address.
        let path = tecdsa::settlement_derivation_path(self.chain_id);
        let (_, settlement_addr) = tecdsa::derive_evm_address(path.clone())
            .await
            .map_err(ChainAdapterError::SignatureFailed)?;

        // 3. Fetch nonce.
        let nonce = evm_rpc::get_transaction_count(self.chain_id, &settlement_addr)
            .await
            .map_err(|message| ChainAdapterError::RpcError {
                provider: "evm_rpc".to_string(),
                message,
            })?;

        // 4. Fetch fee estimates.
        let (base_fee, prio) = self.build_fees().await?;

        // 5. Build EIP-1559 fields (calldata + gas + to + value) via the shared
        //    builder (tx::build_eip1559_fields) — the single source of truth for
        //    the per-op-kind field shape, shared with the settlement worker so a
        //    submit and a replace-by-fee resubmit are byte-identical.
        //    gas_limit = 120_000; max_fee = 2 * base_fee + priority_fee.
        let max_fee = base_fee.saturating_mul(2).saturating_add(prio);
        let fields = tx::build_eip1559_fields(
            self.chain_id.0 as u64,
            tx::MonadTxKind::Mint {
                contract: &contract,
                recipient: &instr.recipient,
                amount_e8s: instr.amount_e8s,
                vault_id: instr.vault_id,
            },
            nonce,
            prio,
            max_fee,
        );

        // 6. Sign.
        let raw = tx::sign_eip1559(&fields, path, &settlement_addr)
            .await
            .map_err(ChainAdapterError::SignatureFailed)?;

        Ok(SignedMint {
            raw_tx: raw.into_bytes(),
            tx_hash: String::new(), // populated by the broadcaster (Task 13)
        })
    }

    /// Phase 1b: burns are user-initiated on-chain (the user calls `burn()` on
    /// the icUSD EVM contract directly).  The canister never signs a burn in
    /// Phase 1b.  This slot is reserved for a Phase 1c SP-backstop burn where
    /// the canister might need to burn icUSD held by the settlement address.
    async fn sign_burn(
        &self,
        _amount_e8s: u128,
        _burner: Principal,
    ) -> Result<SignedBurn, ChainAdapterError> {
        Err(ChainAdapterError::NotImplemented)
    }

    /// Return the latest and finalized block numbers for the chain.
    async fn fetch_finality(&self) -> Result<FinalitySnapshot, ChainAdapterError> {
        let (latest_block, finalized_block) = evm_rpc::fetch_block_numbers(self.chain_id)
            .await
            .map_err(|message| ChainAdapterError::RpcError {
                provider: "evm_rpc".to_string(),
                message,
            })?;
        Ok(FinalitySnapshot {
            latest_block,
            finalized_block,
        })
    }

    /// Deposit / burn log observation is handled by the dedicated observer
    /// (Task 9) which holds typed state for cursor tracking and does typed log
    /// decoding via `decode_burn_log` / `decode_mint_log`.  This trait method
    /// satisfies the interface contract; callers that need events should use
    /// the observer directly.
    async fn observe_event(
        &self,
        from_block: u64,
    ) -> Result<Vec<DepositRecord>, ChainAdapterError> {
        let _ = from_block; // cursor is managed by the observer, not this method
        Ok(vec![])
    }
}
