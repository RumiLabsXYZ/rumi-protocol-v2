//! Native Stellar XLM primitives: classic addresses, amounts, and Payment XDR.
//!
//! These pure helpers do not validate account existence, custody state,
//! transaction inclusion, signatures, or network state.

pub mod address;
pub mod amount;
pub mod codec;
pub mod ted25519;

pub use address::{
    classic_address_from_ed25519_pubkey, ed25519_pubkey_from_classic_address,
    is_valid_classic_address,
};
pub use amount::{
    format_stroops, parse_xlm_to_stroops, spendable_stroops, AmountError, STROOPS_PER_XLM,
    XLM_DECIMALS,
};
pub use codec::{
    encode_signed_envelope, signing_payload_hash, CodecError, Network, Payment, SigningMaterial,
};
pub use ted25519::{derive_stellar_address, sign_payload_hash, StellarKeyRole, STELLAR_CHAIN_ID};
