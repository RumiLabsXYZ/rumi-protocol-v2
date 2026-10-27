//! Stellar's network-independent address codecs.
//!
//! This module only handles classic Ed25519 account `G…` StrKeys. It does not
//! validate account existence, muxed accounts, contracts, or network state.

pub mod address;
pub mod amount;

pub use address::{
    classic_address_from_ed25519_pubkey, ed25519_pubkey_from_classic_address,
    is_valid_classic_address,
};
pub use amount::{
    format_stroops, parse_xlm_to_stroops, spendable_stroops, AmountError, STROOPS_PER_XLM,
    XLM_DECIMALS,
};
