//! Dormant key-name and network policy for native-XLM custody.

use super::codec::Network;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StellarKeyNetwork {
    pub key_name: String,
    pub network: Network,
}

pub const STELLAR_PRODUCTION_KEY_NAME: &str = "key_1";
pub const STELLAR_TEST_KEY_NAME: &str = "test_key_1";

/// The persisted key name is the sole selector for the matching Stellar network.
/// Unknown names deliberately have no network mapping and must fail closed.
pub fn network_for_key_name(key_name: &str) -> Option<Network> {
    match key_name {
        STELLAR_PRODUCTION_KEY_NAME => Some(Network::Public),
        STELLAR_TEST_KEY_NAME => Some(Network::Test),
        _ => None,
    }
}

pub fn default_key_name() -> String {
    STELLAR_TEST_KEY_NAME.to_string()
}

/// Resolve the single persisted XLM key selector to its coupled network. Callers
/// should use this instead of selecting a network independently from state.
pub fn selection_from_state(
    state: &crate::state::State,
) -> Result<StellarKeyNetwork, &'static str> {
    let key_name = state.xlm_stellar_key_name.clone();
    let network = network_for_key_name(&key_name)
        .ok_or("unsupported Stellar key name (expected test_key_1 or key_1)")?;
    Ok(StellarKeyNetwork { key_name, network })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_name_selects_exact_stellar_network_and_unknown_fails_closed() {
        assert_eq!(network_for_key_name("key_1"), Some(Network::Public));
        assert_eq!(network_for_key_name("test_key_1"), Some(Network::Test));
        assert_eq!(network_for_key_name("key_10"), None);
        assert_eq!(network_for_key_name(""), None);
    }

    #[test]
    fn state_selection_keeps_key_and_network_coupled() {
        let mut state = crate::state::State::default();
        assert_eq!(
            selection_from_state(&state),
            Ok(StellarKeyNetwork {
                key_name: "test_key_1".to_string(),
                network: Network::Test,
            })
        );
        state.xlm_stellar_key_name = "unknown".to_string();
        assert!(selection_from_state(&state).is_err());
    }
}
