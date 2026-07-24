//! Native-SOL address decode + validation (mirrors `chains::xrp::address`).
//!
//! A Solana address is plain base58 of a 32-byte Ed25519 public key: NO
//! version byte, NO checksum (unlike XRPL's Base58Check classic address).
//! That makes a typo far more likely to decode "successfully" than an XRPL
//! address, so validation here must be STRICTER than length-only.
//!
//! `is_valid_sol_address` requires BOTH:
//!   1. base58 decodes to exactly 32 bytes, and
//!   2. the point is on the Ed25519 curve.
//!
//! The on-curve check matters: an off-curve 32-byte value is a program-derived
//! address (PDA), which has no private key. Sending collateral to one destroys
//! it irrecoverably — nobody can ever sign a transaction moving it back out.
//! `solana-pubkey` is already a dependency with the `curve25519` feature
//! enabled (see Cargo.toml), so `Pubkey::is_on_curve()` costs nothing new.
//!
//! This closes a real gap in both reference implementations in this repo and
//! its sibling: `chains::solana::ted25519::decode_solana_address` and
//! musicalchairs' `isPlausibleSolanaAddress` both check length only. Unlike
//! XRP (which only wired its validator into claim settlement), this validator
//! is threaded through ALL THREE entry points that accept a caller-supplied
//! SOL destination: claim settlement, SP opt-in payout address, and manual-
//! liquidation payout (Phase 2 wiring; the validator itself lives here).

use solana_pubkey::Pubkey;

/// Decode a base58 Solana address to its raw 32-byte Ed25519 public key.
/// Errs on non-base58 input or a decode that is not exactly 32 bytes. Does
/// NOT check the on-curve property — see `is_valid_sol_address` for the full
/// trust-boundary validator. Delegates to the existing pure base58 decode in
/// `chains::solana::ted25519` (identical logic, no reason to duplicate a
/// plain bs58 decode-and-length-check).
pub fn decode_sol_address(s: &str) -> Result<[u8; 32], String> {
    crate::chains::solana::ted25519::decode_solana_address(s)
}

/// The single trust-boundary validator for a caller-supplied SOL destination:
/// well-formed base58 AND on the Ed25519 curve. An off-curve value (a PDA) is
/// rejected even though it decodes cleanly, because sending SOL there would
/// destroy it irrecoverably (see module doc comment).
pub fn is_valid_sol_address(s: &str) -> bool {
    match decode_sol_address(s) {
        Ok(bytes) => Pubkey::new_from_array(bytes).is_on_curve(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use solana_pubkey::Pubkey;

    /// A real Ed25519 public key is, by construction, on the curve (it is a
    /// clamped-scalar times the base point). Generating one via ed25519-dalek
    /// (a dev-dependency; host-side test only) avoids hardcoding a guessed
    /// address, matching the design doc's instruction for the off-curve case.
    fn on_curve_address() -> String {
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let pk = sk.verifying_key();
        bs58::encode(pk.to_bytes()).into_string()
    }

    /// A real off-curve PDA, derived via `Pubkey::find_program_address` (the
    /// same mechanism `chains::solana::tx::derive_ata` uses), rather than a
    /// hardcoded guess.
    fn off_curve_pda_address() -> String {
        let program_id = Pubkey::new_from_array([9u8; 32]);
        let (pda, _bump) = Pubkey::find_program_address(&[b"sol-collateral-test"], &program_id);
        bs58::encode(pda.as_ref()).into_string()
    }

    #[test]
    fn accepts_a_real_on_curve_address() {
        let addr = on_curve_address();
        assert!(is_valid_sol_address(&addr), "on-curve address must validate");
    }

    #[test]
    fn rejects_a_real_off_curve_pda() {
        let addr = off_curve_pda_address();
        // Decodes fine (it is 32 valid base58 bytes)...
        assert!(decode_sol_address(&addr).is_ok());
        // ...but must NOT validate as a payable destination.
        assert!(!is_valid_sol_address(&addr), "off-curve PDA must be rejected");
    }

    #[test]
    fn rejects_wrong_length() {
        let short = bs58::encode([1u8; 31]).into_string();
        let long = bs58::encode([1u8; 33]).into_string();
        assert!(!is_valid_sol_address(&short));
        assert!(!is_valid_sol_address(&long));
        assert!(decode_sol_address(&short).is_err());
        assert!(decode_sol_address(&long).is_err());
    }

    #[test]
    fn rejects_non_base58() {
        // '0', 'O', 'I', 'l' are absent from the Bitcoin base58 alphabet Solana
        // addresses use.
        assert!(!is_valid_sol_address("0OIl0OIl0OIl0OIl0OIl0OIl0OIl0OIl"));
        assert!(decode_sol_address("0OIl0OIl0OIl0OIl0OIl0OIl0OIl0OIl").is_err());
    }

    #[test]
    fn rejects_empty() {
        assert!(!is_valid_sol_address(""));
    }
}
