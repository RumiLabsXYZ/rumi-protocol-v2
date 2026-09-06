//! Pure/validating helpers plus the claim-settlement signer for native-SOL
//! collateral (mirrors the relevant pieces of `vault::xrp_credit_amount` /
//! `chains::xrp::adapter`). Nothing here mutates `State` — the deposit-credit
//! and claim-settlement flows that call these live in `vault.rs` (Phase 2).
//!
//! ## Durable-nonce claim settlement (design doc §5)
//! Every SOL out-flow (owner withdraw, close, liquidator reward, protocol
//! fee, SP payout) becomes a `SolClaim`, settled later by a transfer signed
//! with a durable nonce rather than `getLatestBlockhash` (which changes every
//! slot and is chronically `Inconsistent` under multi-provider consensus —
//! see `chains::solana::sol_rpc::get_latest_blockhash`'s doc comment). Every
//! settlement transaction is:
//! ```text
//! [ advance_nonce_account(nonce_account, authority = settlement_key),
//!   system_transfer(custody_address -> destination, lamports) ]
//! ```
//! signed by TWO keys: the settlement key (fee payer, first signer, nonce
//! authority) and the per-vault custody key (the account the lamports
//! actually move out of).
//!
//! `sign_sol_payment_from` does NOT call
//! `chains::solana::tx::build_transfer_message_with_nonce` directly, even
//! though that builder is otherwise reused as-is throughout this rail: that
//! builder takes a SINGLE `from` key and uses it as the fee payer, the nonce
//! authority, AND the transfer source all at once (correct for the dormant
//! rail's SPL-mint settlement wallet, which really is one key doing all three
//! jobs). Here those roles are deliberately split across two different keys
//! — the transfer source is the per-vault CUSTODY key, not the settlement
//! key, so the user's collateral is never shaved by network fees and the
//! custody account needs no fee buffer beyond rent exemption (design doc
//! §5.1, point 2). So this module composes the same PURE, already-`pub`
//! instruction builders (`advance_nonce_instruction`, `system_transfer_instruction`)
//! directly with `solana_message::Message::new_with_blockhash` (the same
//! non-feature-gated compiler `build_transfer_message_with_nonce` itself
//! delegates to), which yields the identical canonical wire layout for a
//! two-signer message. `serialize_legacy_message`, `order_signatures_by_signer`,
//! `assemble_wire_tx_multi`, and `first_signature_base58` ARE reused verbatim.

use solana_message::{Hash, Message};
use solana_pubkey::Pubkey;

use crate::chains::solana::tx::{
    advance_nonce_instruction, assemble_wire_tx_multi, first_signature_base58,
    order_signatures_by_signer, serialize_legacy_message, system_transfer_instruction,
};
use crate::ProtocolError;

use super::ted25519;

/// Pure: lamports to credit from a verified SOL custody balance, net of the
/// rent-exempt minimum the user's deposit also funds (the custody account
/// stays permanently rent-exempt rather than being swept to zero — see the
/// design doc §4.1). Mirrors `vault::xrp_credit_amount`'s shape exactly, but
/// both inputs are already `u64` lamports (no u128 balance type on this
/// rail), so a plain `saturating_sub` replaces XRP's u128->u64 downcast.
/// Errors if nothing is creditable (balance <= rent_exempt) or the net is
/// below the per-collateral minimum.
pub fn sol_credit_amount(
    balance_lamports: u64,
    rent_exempt: u64,
    min_deposit: u64,
) -> Result<u64, ProtocolError> {
    let credited = balance_lamports.saturating_sub(rent_exempt);
    if credited == 0 || credited < min_deposit {
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: min_deposit.max(1),
        });
    }
    Ok(credited)
}

/// Pure: a withdrawal of `amount_lamports` from a custody account leaves it
/// funded down to (but never below) the rent-exempt minimum. The Solana
/// NETWORK FEE is paid by the settlement wallet (the fee payer on every
/// settlement transaction — see the module doc comment), NOT the custody
/// account, so it is deliberately NOT subtracted here; only the rent-exempt
/// reserve is protected.
pub fn validate_withdrawal(
    balance_lamports: u64,
    amount_lamports: u64,
    rent_exempt: u64,
) -> Result<(), String> {
    let need = amount_lamports
        .checked_add(rent_exempt)
        .ok_or_else(|| "amount + rent-exempt reserve overflows u64 lamports".to_string())?;
    if balance_lamports < need {
        return Err(format!(
            "insufficient SOL: balance {balance_lamports} lamports < amount {amount_lamports} + rent-exempt reserve {rent_exempt}"
        ));
    }
    Ok(())
}

/// Sign a 64-byte-fixed threshold-Ed25519 signature over `message` at `path`.
/// Thin wrapper over `ted25519::sign_message` that converts the returned
/// `Vec<u8>` to `[u8; 64]`, mirroring `chains::solana::tx`'s private `sign_64`
/// (not reused directly: it is not `pub`, and duplicating a five-line
/// length-check is cheaper than widening that module's visibility).
async fn sign_64(message: Vec<u8>, path: Vec<Vec<u8>>) -> Result<[u8; 64], String> {
    let sig = ted25519::sign_message(message, path).await?;
    sig.as_slice()
        .try_into()
        .map_err(|_| format!("expected 64-byte Ed25519 signature, got {}", sig.len()))
}

/// Build, multi-sign, and wire-assemble a durable-nonce SOL transfer from a
/// vault's custody address to `destination`, for claim settlement.
///
/// Returns `(wire_tx, signature)` where `signature` is the LOCALLY computed
/// (never a single RPC provider's reported) base58 transaction signature —
/// deterministic from the signed bytes, exactly like the XRP tx-hash and the
/// dormant Solana rail's settlement worker both compute locally so an
/// in-flight op can be tracked by its own signature regardless of whether the
/// `sendTransaction` outcall itself returns `Ok` or a "maybe-sent" `Err`.
///
/// `custody_pubkey` / `settlement_pubkey` MUST correspond to `custody_path` /
/// `settlement_path` respectively (each pubkey is derived from its own path
/// via `ted25519::derive_sol_address`) — a mismatch makes the on-chain
/// signature check fail. The fee payer / first required signer is always the
/// SETTLEMENT key (see the module doc comment for why the message is
/// composed directly rather than via the single-key
/// `build_transfer_message_with_nonce`).
pub async fn sign_sol_payment_from(
    custody_path: Vec<Vec<u8>>,
    custody_pubkey: &Pubkey,
    destination: &Pubkey,
    lamports: u64,
    nonce_account_pubkey: &Pubkey,
    settlement_path: Vec<Vec<u8>>,
    settlement_pubkey: &Pubkey,
    durable_nonce: Hash,
) -> Result<(Vec<u8>, String), String> {
    let advance = advance_nonce_instruction(nonce_account_pubkey, settlement_pubkey);
    let transfer = system_transfer_instruction(custody_pubkey, destination, lamports);
    let message = Message::new_with_blockhash(
        &[advance, transfer],
        Some(settlement_pubkey),
        &durable_nonce,
    );
    let message_bytes = serialize_legacy_message(&message);

    // Sign the SAME serialized bytes with each required signer's own path.
    let settlement_sig = sign_64(message_bytes.clone(), settlement_path).await?;
    let custody_sig = sign_64(message_bytes.clone(), custody_path).await?;

    let signers = [
        (*settlement_pubkey, settlement_sig),
        (*custody_pubkey, custody_sig),
    ];
    let ordered = order_signatures_by_signer(&message, &signers)?;
    let wire = assemble_wire_tx_multi(&ordered, &message_bytes);
    let signature = first_signature_base58(&wire)?;
    Ok((wire, signature))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── sol_credit_amount boundaries ─────────────────────────────────────

    #[test]
    fn credit_amount_rejects_below_rent_exempt() {
        // balance <= rent_exempt => nothing creditable.
        assert!(matches!(
            sol_credit_amount(890_880, 890_880, 0),
            Err(ProtocolError::AmountTooLow { .. })
        ));
        assert!(matches!(
            sol_credit_amount(500_000, 890_880, 0),
            Err(ProtocolError::AmountTooLow { .. })
        ));
    }

    #[test]
    fn credit_amount_accepts_exactly_min_deposit() {
        let min_deposit = 20_000_000u64;
        let rent_exempt = 890_880u64;
        let balance = rent_exempt + min_deposit;
        assert_eq!(
            sol_credit_amount(balance, rent_exempt, min_deposit).unwrap(),
            min_deposit
        );
    }

    #[test]
    fn credit_amount_rejects_below_min_deposit() {
        let min_deposit = 20_000_000u64;
        let rent_exempt = 890_880u64;
        let balance = rent_exempt + min_deposit - 1;
        assert!(matches!(
            sol_credit_amount(balance, rent_exempt, min_deposit),
            Err(ProtocolError::AmountTooLow { .. })
        ));
    }

    #[test]
    fn credit_amount_handles_saturation_without_panicking() {
        // rent_exempt > balance must not underflow-panic.
        assert!(sol_credit_amount(0, u64::MAX, 0).is_err());
    }

    // ─── validate_withdrawal boundaries ────────────────────────────────────

    #[test]
    fn validate_withdrawal_accepts_exact_boundary() {
        let rent_exempt = 890_880u64;
        let amount = 1_000_000u64;
        let balance = amount + rent_exempt;
        assert!(validate_withdrawal(balance, amount, rent_exempt).is_ok());
    }

    #[test]
    fn validate_withdrawal_rejects_one_lamport_short() {
        let rent_exempt = 890_880u64;
        let amount = 1_000_000u64;
        let balance = amount + rent_exempt - 1;
        assert!(validate_withdrawal(balance, amount, rent_exempt).is_err());
    }

    #[test]
    fn validate_withdrawal_rejects_overflowing_request() {
        assert!(validate_withdrawal(u64::MAX, u64::MAX, 1).is_err());
    }

    #[test]
    fn validate_withdrawal_does_not_subtract_a_network_fee() {
        // Balance exactly covers amount + rent-exempt reserve with NOTHING left
        // over for a network fee — must still succeed, because the fee is paid
        // by the settlement wallet, not this account.
        let rent_exempt = 890_880u64;
        let amount = 5_000_000u64;
        assert!(validate_withdrawal(amount + rent_exempt, amount, rent_exempt).is_ok());
    }
}
