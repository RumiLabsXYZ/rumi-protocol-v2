//! `SolanaAdapter`: M2 implementation of `ChainAdapter` for Solana devnet.
//!
//! Wires the tested threshold-Ed25519, SOL RPC, and durable-nonce primitives
//! from this module into the six `ChainAdapter` trait methods. Mirrors
//! `chains::monad::adapter` (same struct shape, same error-mapping discipline,
//! same "broadcaster sets the real hash later" contract).
//!
//! ## Signing model
//! One settlement key per chain (derived via `settlement_derivation_path`) is
//! the SPL mint authority AND the native-SOL withdrawal source AND the
//! durable-nonce authority. A separate `nonce_derivation_path` key owns the
//! durable nonce account. Mint and transfer messages are nonce-led
//! (`advance_nonce_account` first), which on Solana needs only ONE signature
//! (the settlement key); the nonce account is a non-signer in advance-nonce.
//! So we sign with the single-signature path and assemble a single-sig wire tx.
//! (The two-signer path is only for the create-nonce bootstrap in `tx.rs`.)
//!
//! ## Phase scope
//! - `sign_mint`: builds + signs a durable-nonce `create-ATA-idempotent + MintTo`
//!   message on the icUSD SPL mint.
//! - `sign_withdrawal`: builds + signs a durable-nonce native SOL transfer.
//! - `sign_burn`: reserved; Solana burns are user-initiated on-chain (M3).
//! - `observe_event`: delegated to the dedicated observer (Task 7).
//! - `verify_deposit` / `fetch_finality`: use SOL RPC reads.

use async_trait::async_trait;
use candid::Principal;
use solana_pubkey::Pubkey;

use crate::chains::adapter::{
    ChainAdapter, ChainAdapterError, DepositRecord, FinalitySnapshot, MintInstruction,
    SignedBurn, SignedMint, SignedWithdrawal, WithdrawalRequest,
};
use crate::chains::config::ChainId;
use crate::state::read_state;

use super::{sol_rpc, ted25519, tx};

// ─── SolanaAdapter ───────────────────────────────────────────────────────────

/// Adapter binding the Solana chain to the Rumi protocol.
pub struct SolanaAdapter {
    chain_id: ChainId,
}

impl SolanaAdapter {
    /// Create a new `SolanaAdapter` for the given chain id.
    ///
    /// In production this is always `SOLANA_CHAIN_ID` (501); tests may pass an
    /// arbitrary id for isolation.
    pub fn new(chain_id: ChainId) -> Self {
        SolanaAdapter { chain_id }
    }

    // ─── private helpers ─────────────────────────────────────────────────────

    /// Return the deployed icUSD SPL mint address (base58) for this chain, or an
    /// error if it has not yet been registered via `set_chain_contract`.
    fn icusd_mint_b58(&self) -> Result<String, ChainAdapterError> {
        read_state(|s| s.multi_chain.chain_contracts.get(&self.chain_id).cloned())
            .ok_or_else(|| ChainAdapterError::InvalidPayload("icUSD mint not set".to_string()))
    }
}

// ─── pure helpers (synchronous; unit-tested) ─────────────────────────────────

/// Decode a base58 Solana address into a `Pubkey` at the trust boundary,
/// mapping any malformed input to `InvalidPayload` so a bad address NEVER
/// panics deep in tx-building. `what` names the field for the error message.
/// `pub(crate)` so the synchronous validation is unit-tested directly (the
/// async signing/broadcast path is covered by the Task 9 PocketIC test).
pub(crate) fn decode_pubkey(b58: &str, what: &str) -> Result<Pubkey, ChainAdapterError> {
    let arr = ted25519::decode_solana_address(b58)
        .map_err(|e| ChainAdapterError::InvalidPayload(format!("invalid {what} address: {e}")))?;
    Ok(Pubkey::new_from_array(arr))
}

/// Convert a u128 e8s/lamports amount to the on-chain `u64` with a CHECKED
/// conversion (never `as u64`), mapping overflow to `InvalidPayload`. Both SPL
/// `MintTo` amounts and System `Transfer` lamports are u64 on Solana.
/// `pub(crate)` for the same unit-test reason as `decode_pubkey`.
pub(crate) fn checked_amount_u64(amount: u128) -> Result<u64, ChainAdapterError> {
    u64::try_from(amount)
        .map_err(|_| ChainAdapterError::InvalidPayload("amount exceeds u64".to_string()))
}

// ─── ChainAdapter impl ─────────────────────────────────────────────────────────

#[async_trait(?Send)]
impl ChainAdapter for SolanaAdapter {
    fn chain_id(&self) -> ChainId {
        self.chain_id
    }

    /// Confirm `tx_sig` landed and succeeded on-chain at `finalized`.
    ///
    /// Returns a `DepositRecord` with `block_number` set to the confirmation
    /// slot when the tx is finalized and successful. `amount_e8s` and
    /// `depositor` are left at defaults (0 / empty string), mirroring the Monad
    /// adapter: `verify_deposit` only confirms finality, and the observer
    /// (Task 7) decodes amounts/depositor from the instruction with full
    /// context. A not-found signature => `InvalidPayload("deposit not
    /// finalized")`; a landed-but-reverted tx => `InvalidPayload("deposit
    /// failed")`; an RPC failure => `RpcError`.
    async fn verify_deposit(&self, tx_sig: &str) -> Result<DepositRecord, ChainAdapterError> {
        match sol_rpc::get_transaction(tx_sig).await {
            Ok(sol_rpc::TxStatus::Confirmed { slot }) => Ok(DepositRecord {
                depositor: String::new(),
                amount_e8s: 0,
                block_number: slot,
                tx_hash: tx_sig.to_string(),
            }),
            Ok(sol_rpc::TxStatus::NotFound) => Err(ChainAdapterError::InvalidPayload(
                "deposit not finalized".to_string(),
            )),
            Ok(sol_rpc::TxStatus::Failed) => Err(ChainAdapterError::InvalidPayload(
                "deposit failed".to_string(),
            )),
            Err(message) => Err(ChainAdapterError::RpcError {
                provider: "sol_rpc".to_string(),
                message,
            }),
        }
    }

    /// Build and sign a durable-nonce native SOL transfer to `req.recipient`.
    ///
    /// # Amount note
    /// `WithdrawalRequest.amount_e8s` carries the SOL amount in its native
    /// denomination (lamports, e9) here, mirroring the Monad adapter's wei
    /// wart in the same field. The value is passed through unchanged as the
    /// System `Transfer` lamports (after a checked u128->u64 conversion).
    async fn sign_withdrawal(
        &self,
        req: WithdrawalRequest,
    ) -> Result<SignedWithdrawal, ChainAdapterError> {
        // 1. Validate + decode the recipient base58 at the boundary, and do the
        //    checked amount conversion, BEFORE any await (mirror Monad's
        //    discipline of failing fast on bad payloads).
        let recipient = decode_pubkey(&req.recipient, "recipient")?;
        let lamports = checked_amount_u64(req.amount_e8s)?; // wart: see doc comment

        // 2. Derive settlement (fee payer + nonce authority + transfer source)
        //    and nonce-account addresses. No state borrow is held across these
        //    awaits.
        let settlement_path = ted25519::settlement_derivation_path(self.chain_id);
        let (settlement_pk, _settlement_addr) =
            ted25519::derive_solana_address(settlement_path.clone())
                .await
                .map_err(ChainAdapterError::SignatureFailed)?;
        let nonce_path = ted25519::nonce_derivation_path(self.chain_id);
        let (nonce_pk, nonce_addr) = ted25519::derive_solana_address(nonce_path)
            .await
            .map_err(ChainAdapterError::SignatureFailed)?;

        let settlement = decode_pubkey_bytes(&settlement_pk, "settlement")?;
        let nonce = decode_pubkey_bytes(&nonce_pk, "nonce")?;

        // 3. Read the current durable nonce (the message's recent_blockhash).
        let durable_nonce =
            sol_rpc::get_durable_nonce(&nonce_addr)
                .await
                .map_err(|message| ChainAdapterError::RpcError {
                    provider: "sol_rpc".to_string(),
                    message,
                })?;

        // 4. Build the nonce-led transfer message (advance_nonce first), serialize,
        //    sign once with the settlement key (single required signer), assemble
        //    the single-signature wire tx.
        let message = tx::build_transfer_message_with_nonce(
            &settlement,
            &recipient,
            lamports,
            &nonce,
            durable_nonce,
        );
        let raw_tx = sign_single(&message, settlement_path).await?;

        Ok(SignedWithdrawal {
            raw_tx,
            tx_hash: String::new(), // populated by the broadcaster (Task 8)
        })
    }

    /// Build and sign a durable-nonce `create-ATA-idempotent + MintTo` message
    /// on the icUSD SPL mint, with the settlement key as both fee payer and mint
    /// authority. The recipient's associated token account is created if absent
    /// (idempotent) then minted into.
    async fn sign_mint(&self, instr: MintInstruction) -> Result<SignedMint, ChainAdapterError> {
        // 1. Resolve + decode the SPL mint, decode the recipient, and do the
        //    checked amount conversion. All synchronous, all fail-fast, all
        //    before any await.
        let mint_b58 = self.icusd_mint_b58()?;
        let mint = decode_pubkey(&mint_b58, "mint")?;
        let recipient = decode_pubkey(&instr.recipient, "recipient")?;
        let amount = checked_amount_u64(instr.amount_e8s)?;

        // 2. Derive settlement (fee payer + mint authority + nonce authority)
        //    and nonce-account addresses.
        let settlement_path = ted25519::settlement_derivation_path(self.chain_id);
        let (settlement_pk, _settlement_addr) =
            ted25519::derive_solana_address(settlement_path.clone())
                .await
                .map_err(ChainAdapterError::SignatureFailed)?;
        let nonce_path = ted25519::nonce_derivation_path(self.chain_id);
        let (nonce_pk, nonce_addr) = ted25519::derive_solana_address(nonce_path)
            .await
            .map_err(ChainAdapterError::SignatureFailed)?;

        let settlement = decode_pubkey_bytes(&settlement_pk, "settlement")?;
        let nonce = decode_pubkey_bytes(&nonce_pk, "nonce")?;

        // 3. Read the durable nonce.
        let durable_nonce =
            sol_rpc::get_durable_nonce(&nonce_addr)
                .await
                .map_err(|message| ChainAdapterError::RpcError {
                    provider: "sol_rpc".to_string(),
                    message,
                })?;

        // 4. Build the nonce-led mint message (authority = settlement is both the
        //    mint authority and the nonce authority), serialize, sign once with
        //    the settlement key, assemble the single-signature wire tx.
        let message = tx::build_mint_message_with_nonce(
            &settlement,
            &mint,
            &recipient,
            amount,
            &nonce,
            durable_nonce,
        );
        let raw_tx = sign_single(&message, settlement_path).await?;

        Ok(SignedMint {
            raw_tx,
            tx_hash: String::new(), // populated by the broadcaster (Task 8)
        })
    }

    /// M2: burns are user-initiated on-chain (the user burns their SPL icUSD
    /// directly). The canister never signs a burn in M2. Reserved for an M3
    /// SP-backstop burn where the canister might burn icUSD held by the
    /// settlement address.
    async fn sign_burn(
        &self,
        _amount_e8s: u128,
        _burner: Principal,
    ) -> Result<SignedBurn, ChainAdapterError> {
        Err(ChainAdapterError::NotImplemented)
    }

    /// Return the latest and finalized slots for the chain. On Solana the
    /// commitment level replaces EVM block depth: `confirmed` is the latest
    /// (super-majority) slot, `finalized` is the rooted/finalized slot.
    async fn fetch_finality(&self) -> Result<FinalitySnapshot, ChainAdapterError> {
        let finalized_block = sol_rpc::get_slot("finalized")
            .await
            .map_err(|message| ChainAdapterError::RpcError {
                provider: "sol_rpc".to_string(),
                message,
            })?;
        let latest_block = sol_rpc::get_slot("confirmed")
            .await
            .map_err(|message| ChainAdapterError::RpcError {
                provider: "sol_rpc".to_string(),
                message,
            })?;
        Ok(FinalitySnapshot {
            latest_block,
            finalized_block,
        })
    }

    /// Deposit observation is handled by the dedicated observer (Task 7), which
    /// holds typed state for cursor tracking and decodes amounts/depositor from
    /// the instruction. This trait method satisfies the interface contract;
    /// callers that need events should use the observer directly. Mirrors the
    /// Monad adapter.
    async fn observe_event(
        &self,
        from_block: u64,
    ) -> Result<Vec<DepositRecord>, ChainAdapterError> {
        let _ = from_block; // cursor is managed by the observer, not this method
        Ok(vec![])
    }
}

// ─── async signing helper (settlement single-sig) ────────────────────────────

/// Decode a derived 32-byte Ed25519 pubkey into a `Pubkey`, mapping a wrong
/// length to `SignatureFailed` (a derivation bug, not a payload bug). `what`
/// names the key for the error.
fn decode_pubkey_bytes(bytes: &[u8], what: &str) -> Result<Pubkey, ChainAdapterError> {
    let arr: [u8; 32] = bytes.try_into().map_err(|_| {
        ChainAdapterError::SignatureFailed(format!(
            "{what} pubkey must be 32 bytes, got {}",
            bytes.len()
        ))
    })?;
    Ok(Pubkey::new_from_array(arr))
}

/// Serialize a legacy message, threshold-Ed25519 sign its bytes at
/// `settlement_path` (the single required signer for a nonce-led mint/transfer),
/// and assemble the single-signature wire tx. Maps a signing failure or a
/// wrong-length signature to `SignatureFailed`.
async fn sign_single(
    message: &solana_message::Message,
    settlement_path: Vec<Vec<u8>>,
) -> Result<Vec<u8>, ChainAdapterError> {
    let message_bytes = tx::serialize_legacy_message(message);
    let signature = ted25519::sign_message(message_bytes.clone(), settlement_path)
        .await
        .map_err(ChainAdapterError::SignatureFailed)?;
    let sig_arr: [u8; 64] = signature.as_slice().try_into().map_err(|_| {
        ChainAdapterError::SignatureFailed(format!(
            "expected 64-byte Ed25519 signature, got {}",
            signature.len()
        ))
    })?;
    Ok(tx::assemble_wire_tx(sig_arr, &message_bytes))
}
