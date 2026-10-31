//! Disabled multichain icUSD destination profiles and their validator.
//!
//! Groundwork only (card BB-38, design section 9). Nothing here activates a
//! destination: every network is `disabled`, no canister depends on this crate,
//! and no function makes a network call or reads a clock.
//!
//! Amounts in the JSON files are canonical decimal strings, never numbers, so a
//! JavaScript consumer cannot round a `u64` such as `18446744073709551615`.
//! Rust parses them into checked `u64` values (see [`parse_u64_decimal`]).

use serde::Serialize;

pub mod aggregate;
pub mod types;
pub mod validate;

#[cfg(test)]
mod tests;

pub use types::*;
pub use validate::{validate, Violation, REQUIRED_PROFILE_IDS};

/// The nine profiles, embedded at compile time, in a stable order.
pub const PROFILE_FILES: [(&str, &str); 9] = [
    ("ethereum", include_str!("../profiles/ethereum.json")),
    ("base", include_str!("../profiles/base.json")),
    ("arbitrum", include_str!("../profiles/arbitrum.json")),
    ("robinhood", include_str!("../profiles/robinhood.json")),
    ("hyperevm", include_str!("../profiles/hyperevm.json")),
    ("monad", include_str!("../profiles/monad.json")),
    ("solana", include_str!("../profiles/solana.json")),
    ("sui", include_str!("../profiles/sui.json")),
    ("xrpl", include_str!("../profiles/xrpl.json")),
];

/// Parse one profile. Unknown fields are rejected.
pub fn parse_profile(json: &str) -> Result<Profile, serde_json::Error> {
    serde_json::from_str(json)
}

/// Parse the nine embedded profiles.
pub fn load_embedded() -> Result<Vec<Profile>, String> {
    PROFILE_FILES
        .iter()
        .map(|(name, json)| parse_profile(json).map_err(|e| format!("{name}.json: {e}")))
        .collect()
}

/// The serialized (snake_case) name of a unit enum value, for messages.
pub fn label<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}
