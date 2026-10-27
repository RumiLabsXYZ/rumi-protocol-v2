//! Dormant, caller-configured Horizon provider set for read-only XLM evidence.
//!
//! Nothing in the canister API invokes this module. Provider identity and
//! origin configuration must be supplied explicitly; no vendor URLs or
//! credentials are embedded here.

use super::{
    codec::Network,
    horizon_parse::{self, AccountSnapshot, FinalizedDeposit, LatestLedgerReserve},
    horizon_quorum, horizon_rpc,
};
use ic_cdk::api::management_canister::http_request::{http_request, CanisterHttpRequestArgument};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HorizonProvider {
    pub identity: String,
    pub base_url: String,
}

/// Provider whose Horizon root response has been checked against the selected
/// Stellar network. Fields are private so callers cannot assert validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedHorizonProvider {
    provider: HorizonProvider,
    network: Network,
}

/// HTTP-status evidence for one transaction lookup. `NotFound` is explicitly
/// separate from a found transaction and must never be interpreted as success.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TransactionStatusHttpEvidence {
    Http200(Vec<u8>),
    Http404NotFound(Vec<u8>),
}

impl TransactionStatusHttpEvidence {
    /// Return the HTTP code and normalized body expected by the pure status
    /// classifier. The tuple preserves 404 as a not-found observation.
    pub fn status_and_body(&self) -> (u16, &[u8]) {
        match self {
            Self::Http200(body) => (200, body),
            Self::Http404NotFound(body) => (404, body),
        }
    }
}

impl ValidatedHorizonProvider {
    pub fn identity(&self) -> &str {
        &self.provider.identity
    }

    pub fn network(&self) -> Network {
        self.network
    }
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
    NetworkVerificationFailed(usize),
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

    /// Validate all configured providers against their root resource before
    /// permitting any account, transaction, or ledger evidence request.
    pub async fn validate_for_network(
        &self,
        network: Network,
    ) -> Result<[ValidatedHorizonProvider; 3], ProviderConfigError> {
        let providers = self.validate()?;
        let mut validated: [Option<ValidatedHorizonProvider>; 3] = [None, None, None];
        for (index, provider) in providers.into_iter().enumerate() {
            let verified = async {
                let request = horizon_rpc::root_request(&provider.base_url)?;
                let body = read_request(request).await.ok()?;
                horizon_rpc::validate_root_network(&body, network).ok()?;
                Some(ValidatedHorizonProvider { provider, network })
            }
            .await;
            validated[index] =
                Some(verified.ok_or(ProviderConfigError::NetworkVerificationFailed(index))?);
        }
        Ok(validated.map(|provider| provider.expect("verified provider slot")))
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

async fn read_latest(provider: &ValidatedHorizonProvider) -> Result<LatestLedgerReserve, String> {
    let provider = &provider.provider;
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

async fn read_transaction_status(
    request: CanisterHttpRequestArgument,
) -> Result<TransactionStatusHttpEvidence, String> {
    let (response,) = http_request(request, super::horizon_reads::READ_CYCLES)
        .await
        .map_err(|(code, message)| {
            format!("Horizon HTTPS outcall rejected ({code:?}): {message}")
        })?;
    classify_transaction_status_response(response.status, response.body)
}

fn classify_transaction_status_response(
    status: candid::Nat,
    body: Vec<u8>,
) -> Result<TransactionStatusHttpEvidence, String> {
    if status == candid::Nat::from(200u16) {
        Ok(TransactionStatusHttpEvidence::Http200(body))
    } else if status == candid::Nat::from(404u16) {
        Ok(TransactionStatusHttpEvidence::Http404NotFound(body))
    } else {
        Err(format!(
            "Horizon transaction status request returned unsupported status {status}"
        ))
    }
}

/// Read one latest-ledger reserve observation from each configured provider.
/// The quorum result is an observed reserve, not proof of freshness. Callers
/// must independently enforce their minimum sequence and maximum age policy.
pub async fn fetch_latest_reserve_observations(
    config: &HorizonProviderConfig,
    network: Network,
) -> Result<([Option<LatestLedgerReserve>; 3], u64), String> {
    let providers = config
        .validate_for_network(network)
        .await
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
    network: Network,
    account_id: &str,
) -> Result<AccountSnapshot, String> {
    let providers = config
        .validate_for_network(network)
        .await
        .map_err(|error| format!("invalid Horizon provider configuration: {error:?}"))?;
    let mut observations = [None, None, None];
    for (index, provider) in providers.iter().enumerate() {
        let Some(request) = account_request_for_provider(&provider.provider, account_id) else {
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
    network: Network,
    transaction_hash: &str,
) -> Result<u32, String> {
    let providers = config
        .validate_for_network(network)
        .await
        .map_err(|error| format!("invalid Horizon provider configuration: {error:?}"))?;
    let mut observations = [None, None, None];
    for (index, provider) in providers.iter().enumerate() {
        let Some(request) = transaction_request_for_provider(&provider.provider, transaction_hash)
        else {
            continue;
        };
        if let Ok(body) = read_request(request).await {
            observations[index] = horizon_parse::parse_transaction(&body, transaction_hash).ok();
        }
    }
    horizon_quorum::transaction_ledger_quorum(observations)
        .map_err(|error| format!("Horizon transaction quorum failed: {error:?}"))
}

/// Fetch a transaction-status response from each independently configured
/// provider after binding all three roots to `network`. A slot is `None` when
/// its request/outcall/status is unavailable; only HTTP 200 and 404 enter as
/// typed status evidence. This does not classify success or claim settlement.
pub async fn fetch_transaction_status_evidence(
    config: &HorizonProviderConfig,
    network: Network,
    transaction_hash: &str,
) -> Result<[Option<TransactionStatusHttpEvidence>; 3], String> {
    if transaction_hash.len() != 64
        || !transaction_hash
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("invalid Stellar transaction hash".to_string());
    }
    let providers = config
        .validate_for_network(network)
        .await
        .map_err(|error| format!("invalid Horizon provider configuration: {error:?}"))?;
    let mut observations = [None, None, None];
    for (index, provider) in providers.iter().enumerate() {
        let request = horizon_rpc::transaction_status_request_at(
            &provider.provider.base_url,
            transaction_hash,
        )
        .ok_or_else(|| "invalid configured Horizon transaction request".to_string())?;
        observations[index] = read_transaction_status(request).await.ok();
    }
    Ok(observations)
}

/// Fetch one bounded transaction-operations page from each provider after
/// binding all provider roots to the selected network. Each slot contains that
/// provider's parsed page or `None` when its request is unavailable/invalid.
/// This returns raw slot evidence only; it does not claim quorum or credit.
pub async fn fetch_custody_deposit_observations(
    config: &HorizonProviderConfig,
    network: Network,
    transaction_hash: &str,
    expected_ledger: u32,
    expected_destination: &str,
) -> Result<[Option<Vec<FinalizedDeposit>>; 3], String> {
    if transaction_hash.len() != 64
        || !transaction_hash
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("invalid Stellar transaction hash".to_string());
    }
    let providers = config
        .validate_for_network(network)
        .await
        .map_err(|error| format!("invalid Horizon provider configuration: {error:?}"))?;
    let mut observations = [None, None, None];
    for (index, provider) in providers.iter().enumerate() {
        let Some(request) = horizon_rpc::transaction_operations_request_at(
            &provider.provider.base_url,
            transaction_hash,
            horizon_rpc::OPERATIONS_PAGE_LIMIT,
        ) else {
            continue;
        };
        if let Ok(body) = read_request(request).await {
            observations[index] = horizon_parse::parse_custody_deposits(
                &body,
                transaction_hash,
                expected_ledger,
                expected_destination,
                horizon_rpc::OPERATIONS_PAGE_LIMIT,
            )
            .ok();
        }
    }
    Ok(observations)
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
    fn network_maps_to_the_canonical_stellar_passphrase() {
        assert_eq!(
            Network::Public.passphrase(),
            "Public Global Stellar Network ; September 2015"
        );
        assert_eq!(
            Network::Test.passphrase(),
            "Test SDF Network ; September 2015"
        );
    }

    #[test]
    fn transaction_status_evidence_preserves_404_as_not_found() {
        let found = TransactionStatusHttpEvidence::Http200(b"{}".to_vec());
        assert_eq!(found.status_and_body(), (200, b"{}".as_slice()));
        let not_found = TransactionStatusHttpEvidence::Http404NotFound(
            br#"{"type":"https://stellar.org/horizon-errors/not_found","status":404}"#.to_vec(),
        );
        let (status, body) = not_found.status_and_body();
        assert_eq!(status, 404);
        assert!(String::from_utf8_lossy(body).contains("not_found"));
        assert!(matches!(
            classify_transaction_status_response(candid::Nat::from(200u16), b"{}".to_vec()),
            Ok(TransactionStatusHttpEvidence::Http200(_))
        ));
        assert!(matches!(
            classify_transaction_status_response(candid::Nat::from(404u16), b"{}".to_vec()),
            Ok(TransactionStatusHttpEvidence::Http404NotFound(_))
        ));
        assert!(
            classify_transaction_status_response(candid::Nat::from(500u16), b"{}".to_vec())
                .is_err()
        );
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
        let status = horizon_rpc::transaction_status_request_at(&provider.base_url, hash).unwrap();
        assert_eq!(status.url, transaction.url);
        assert!(status.transform.is_some());
        let latest = horizon_rpc::latest_ledger_reserve_request(&provider.base_url).unwrap();
        assert_eq!(
            latest.url,
            "https://horizon.example/ledgers?order=desc&limit=1"
        );
    }
}
