//! Dormant, caller-configured Horizon provider set for read-only XLM evidence.
//!
//! Nothing in the canister API invokes this module. Provider identity and
//! origin configuration must be supplied explicitly; no vendor URLs or
//! credentials are embedded here.

use super::{
    horizon_parse::{self, AccountSnapshot, LatestLedgerReserve},
    horizon_quorum, horizon_rpc,
};
use ic_cdk::api::management_canister::http_request::{http_request, CanisterHttpRequestArgument};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HorizonProvider {
    pub identity: String,
    pub base_url: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HorizonProviderConfig {
    /// Slot order is stable and is preserved in all returned observations.
    pub slots: [Option<HorizonProvider>; 3],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderConfigError {
    MissingProvider(usize),
    InvalidIdentity(usize),
    DuplicateIdentity(usize),
    InvalidHttpsOrigin(usize),
    DuplicateOrigin(usize),
}

impl HorizonProviderConfig {
    pub fn validate(&self) -> Result<[HorizonProvider; 3], ProviderConfigError> {
        let mut providers: [Option<HorizonProvider>; 3] = [None, None, None];
        let mut identities: [Option<String>; 3] = [None, None, None];
        let mut origins: [Option<String>; 3] = [None, None, None];
        for index in 0..3 {
            let Some(provider) = self.slots[index].as_ref() else {
                return Err(ProviderConfigError::MissingProvider(index));
            };
            if provider.identity.trim().is_empty()
                || provider.identity.trim() != provider.identity
                || provider.identity.bytes().any(|b| b.is_ascii_whitespace())
            {
                return Err(ProviderConfigError::InvalidIdentity(index));
            }
            if identities[..index]
                .iter()
                .flatten()
                .any(|id| id == &provider.identity)
            {
                return Err(ProviderConfigError::DuplicateIdentity(index));
            }
            let Some(origin) = canonical_https_origin(&provider.base_url) else {
                return Err(ProviderConfigError::InvalidHttpsOrigin(index));
            };
            if origins[..index]
                .iter()
                .flatten()
                .any(|seen| seen == &origin)
            {
                return Err(ProviderConfigError::DuplicateOrigin(index));
            }
            identities[index] = Some(provider.identity.clone());
            origins[index] = Some(origin);
            providers[index] = Some(provider.clone());
        }
        Ok(providers.map(|provider| provider.expect("validated provider slot")))
    }
}

fn canonical_https_origin(value: &str) -> Option<String> {
    let authority = value.strip_prefix("https://")?;
    if authority.is_empty()
        || authority.contains('/')
        || authority.contains('?')
        || authority.contains('#')
        || authority.contains('@')
        || authority.contains('\\')
    {
        return None;
    }
    if authority.bytes().any(|byte| byte.is_ascii_whitespace()) {
        return None;
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => {
            if port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            let port: u16 = port.parse().ok()?;
            if port == 0 {
                return None;
            }
            (host, Some(port))
        }
        Some(_) => return None,
        None => (authority, None),
    };
    if host.is_empty()
        || !host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
        || host.starts_with('.')
        || host.ends_with('.')
        || host.contains("..")
    {
        return None;
    }
    let canonical_host = host.to_ascii_lowercase();
    match port {
        None | Some(443) => Some(format!("https://{canonical_host}")),
        Some(port) => Some(format!("https://{canonical_host}:{port}")),
    }
}

async fn read_latest(provider: &HorizonProvider) -> Result<LatestLedgerReserve, String> {
    let request = horizon_rpc::latest_ledger_reserve_request(&provider.base_url)
        .ok_or_else(|| "invalid configured Horizon origin".to_string())?;
    let (response,) = http_request(request, super::horizon_reads::READ_CYCLES)
        .await
        .map_err(|(code, message)| {
            format!("Horizon HTTPS outcall rejected ({code:?}): {message}")
        })?;
    if response.status != candid::Nat::from(200u16) {
        return Err(format!(
            "Horizon HTTP request returned status {}",
            response.status
        ));
    }
    horizon_parse::parse_latest_ledger_reserve(&response.body)
        .map_err(|error| format!("Horizon latest-ledger response failed validation: {error:?}"))
}

async fn read_request(request: CanisterHttpRequestArgument) -> Result<Vec<u8>, String> {
    let (response,) = http_request(request, super::horizon_reads::READ_CYCLES)
        .await
        .map_err(|(code, message)| {
            format!("Horizon HTTPS outcall rejected ({code:?}): {message}")
        })?;
    if response.status != candid::Nat::from(200u16) {
        return Err(format!(
            "Horizon HTTP request returned status {}",
            response.status
        ));
    }
    Ok(response.body)
}

/// Read one latest-ledger reserve observation from each configured provider.
/// The quorum result is an observed reserve, not proof of freshness. Callers
/// must independently enforce their minimum sequence and maximum age policy.
pub async fn fetch_latest_reserve_observations(
    config: &HorizonProviderConfig,
) -> Result<([Option<LatestLedgerReserve>; 3], u64), String> {
    let providers = config
        .validate()
        .map_err(|error| format!("invalid Horizon provider configuration: {error:?}"))?;
    let mut observations: [Option<LatestLedgerReserve>; 3] = [None, None, None];
    for (index, provider) in providers.iter().enumerate() {
        observations[index] = read_latest(provider).await.ok();
    }
    let reserve_inputs = observations
        .clone()
        .map(|item| item.map(|latest| latest.reserve));
    let reserve = horizon_quorum::ledger_reserve_quorum(reserve_inputs)
        .map_err(|error| format!("Horizon reserve quorum failed: {error:?}"))?;
    Ok((observations, reserve))
}

/// Fetch an account snapshot from every configured provider and apply the
/// existing complete-snapshot quorum. No result is returned without quorum.
pub async fn fetch_account_snapshot_quorum(
    config: &HorizonProviderConfig,
    account_id: &str,
) -> Result<AccountSnapshot, String> {
    let providers = config
        .validate()
        .map_err(|error| format!("invalid Horizon provider configuration: {error:?}"))?;
    let mut observations = [None, None, None];
    for (index, provider) in providers.iter().enumerate() {
        let Some(request) = account_request_for_provider(provider, account_id) else {
            continue;
        };
        if let Ok(body) = read_request(request).await {
            observations[index] = horizon_parse::parse_account(&body, account_id).ok();
        }
    }
    horizon_quorum::account_snapshot_quorum(observations)
        .map_err(|error| format!("Horizon account quorum failed: {error:?}"))
}

/// Fetch a transaction ledger from all three providers and apply the existing
/// strict affirmative-transaction quorum.
pub async fn fetch_transaction_ledger_quorum(
    config: &HorizonProviderConfig,
    transaction_hash: &str,
) -> Result<u32, String> {
    let providers = config
        .validate()
        .map_err(|error| format!("invalid Horizon provider configuration: {error:?}"))?;
    let mut observations = [None, None, None];
    for (index, provider) in providers.iter().enumerate() {
        let Some(request) = transaction_request_for_provider(provider, transaction_hash) else {
            continue;
        };
        if let Ok(body) = read_request(request).await {
            observations[index] = horizon_parse::parse_transaction(&body, transaction_hash).ok();
        }
    }
    horizon_quorum::transaction_ledger_quorum(observations)
        .map_err(|error| format!("Horizon transaction quorum failed: {error:?}"))
}

/// Build an account request for the configured endpoint without changing the
/// existing public/test-network request builders.
pub fn account_request_for_provider(
    provider: &HorizonProvider,
    account_id: &str,
) -> Option<CanisterHttpRequestArgument> {
    if canonical_https_origin(&provider.base_url).is_none()
        || !super::address::is_valid_classic_address(account_id)
    {
        return None;
    }
    let url = format!(
        "{}/accounts/{account_id}",
        provider.base_url.trim_end_matches('/')
    );
    Some(horizon_rpc::request_at(
        url,
        horizon_rpc::MAX_ACCOUNT_RESPONSE_BYTES,
        "stellar_transform_account",
    ))
}

/// Construct a transaction URL only after validating the configured origin.
pub fn transaction_request_for_provider(
    provider: &HorizonProvider,
    transaction_hash: &str,
) -> Option<CanisterHttpRequestArgument> {
    if canonical_https_origin(&provider.base_url).is_none()
        || transaction_hash.len() != 64
        || !transaction_hash.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }
    let url = format!(
        "{}/transactions/{transaction_hash}",
        provider.base_url.trim_end_matches('/')
    );
    Some(horizon_rpc::request_at(
        url,
        horizon_rpc::MAX_TRANSACTION_RESPONSE_BYTES,
        "stellar_transform_transaction",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(identity: &str, base_url: &str) -> Option<HorizonProvider> {
        Some(HorizonProvider {
            identity: identity.into(),
            base_url: base_url.into(),
        })
    }

    fn config(slots: [Option<HorizonProvider>; 3]) -> HorizonProviderConfig {
        HorizonProviderConfig { slots }
    }

    fn valid_config() -> HorizonProviderConfig {
        config([
            provider("alpha", "https://one.example"),
            provider("beta", "https://two.example"),
            provider("gamma", "https://three.example"),
        ])
    }

    #[test]
    fn rejects_missing_provider_slot() {
        let mut config = valid_config();
        config.slots[1] = None;
        assert_eq!(
            config.validate(),
            Err(ProviderConfigError::MissingProvider(1))
        );
    }

    #[test]
    fn rejects_duplicate_provider_identity() {
        let config = config([
            provider("same", "https://one.example"),
            provider("same", "https://two.example"),
            provider("third", "https://three.example"),
        ]);
        assert_eq!(
            config.validate(),
            Err(ProviderConfigError::DuplicateIdentity(1))
        );
    }

    #[test]
    fn rejects_duplicate_canonical_origin_for_case_and_default_port() {
        let config = config([
            provider("one", "https://Horizon.Example"),
            provider("two", "https://horizon.example:443"),
            provider("three", "https://other.example"),
        ]);
        assert_eq!(
            config.validate(),
            Err(ProviderConfigError::DuplicateOrigin(1))
        );
    }

    #[test]
    fn rejects_non_https_paths_and_credentials() {
        for invalid in [
            "http://one.example",
            "https://one.example/path",
            "https://user@one.example",
        ] {
            let config = config([
                provider("one", invalid),
                provider("two", "https://two.example"),
                provider("three", "https://three.example"),
            ]);
            assert_eq!(
                config.validate(),
                Err(ProviderConfigError::InvalidHttpsOrigin(0))
            );
        }
    }

    #[test]
    fn accepts_three_distinct_explicit_provider_slots() {
        let providers = valid_config().validate().unwrap();
        assert_eq!(providers[0].identity, "alpha");
        assert_eq!(providers[1].identity, "beta");
        assert_eq!(providers[2].identity, "gamma");
    }

    #[test]
    fn provider_requests_use_configured_origin_and_fixed_paths() {
        let provider = HorizonProvider {
            identity: "provider".into(),
            base_url: "https://horizon.example".into(),
        };
        let account_id = "GAYOLLLUIZE4DZMBB2ZBKGBUBZLIOYU6XFLW37GBP2VZD3ABNXCW4BVA";
        let account = account_request_for_provider(&provider, account_id).unwrap();
        assert_eq!(
            account.url,
            format!("https://horizon.example/accounts/{account_id}")
        );
        let hash = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let transaction = transaction_request_for_provider(&provider, hash).unwrap();
        assert_eq!(
            transaction.url,
            format!("https://horizon.example/transactions/{hash}")
        );
        let latest = horizon_rpc::latest_ledger_reserve_request(&provider.base_url).unwrap();
        assert_eq!(
            latest.url,
            "https://horizon.example/ledgers?order=desc&limit=1"
        );
    }
}
