//! IC side of the live transport (feature `live-transport`).
//!
//! A tiny trait ([`IcQuery`]: query a canister method with a Candid argument,
//! get the Candid reply bytes back) plus the three reads the collector needs:
//!
//! * the icUSD ledger's `icrc1_total_supply` and `icrc1_balance_of(Account)`;
//! * the backend's per-chain recorded supply, read from
//!   `get_chain_public_launch_status(chain_id)` (an anonymous-safe query whose
//!   `chain_supply_e8s` is `multi_chain.chain_supplies[chain]`). The backend has
//!   no single "all chain supplies" query, so the caller lists the debt-backed
//!   chain ids and the reads are summed with checked arithmetic. A chain the
//!   backend does not know is an error (the query itself would report 0 for an
//!   unknown id, which must never become a silent zero).
//!
//! [`AgentQuery`] is the real implementation over `ic-agent`: anonymous
//! identity, query calls only, response (query signature) verification left on,
//! which is the agent's default. This module never runs by itself; nothing in
//! the crate calls [`AgentQuery`] unless an explicit endpoint config is given.

use candid::{CandidType, Deserialize, Nat, Principal};

use crate::collect::ReadError;
use crate::escrow::EscrowBinding;

/// Why a query produced no reply bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IcQueryError {
    /// The canister has no such query method.
    MethodNotFound(String),
    /// Any other failure (reject, transport, verification). Never contains the
    /// endpoint URL.
    Failed(String),
}

/// Query a canister method. `arg` and the reply are Candid-encoded bytes.
pub trait IcQuery {
    fn query(
        &self,
        canister: &Principal,
        method: &str,
        arg: &[u8],
    ) -> Result<Vec<u8>, IcQueryError>;
}

/// ICRC-1 account on the wire: `record { owner : principal; subaccount : opt blob }`.
#[derive(CandidType, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Icrc1Account {
    pub owner: Principal,
    pub subaccount: Option<Vec<u8>>,
}

/// The part of the backend's `ChainStatus` we read.
#[derive(CandidType, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainStatusSlice {
    Disabled,
    Registered,
}

/// The two fields of the backend's `ChainPublicLaunchStatus` we read. Candid
/// decoding skips the record's other fields.
#[derive(CandidType, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct LaunchStatusSlice {
    pub status: Option<ChainStatusSlice>,
    pub chain_supply_e8s: Nat,
}

fn nat_to_u128(n: Nat, what: &str) -> Result<u128, ReadError> {
    u128::try_from(n.0).map_err(|_| ReadError::Failed(format!("{what} does not fit in u128")))
}

fn map_query_error(e: IcQueryError, method: &str) -> ReadError {
    match e {
        IcQueryError::MethodNotFound(_) => {
            ReadError::UnavailableGap(format!("the canister has no query method {method}"))
        }
        IcQueryError::Failed(why) => ReadError::Failed(format!("{method}: {why}")),
    }
}

fn run(
    q: &dyn IcQuery,
    canister: &Principal,
    method: &str,
    arg: Vec<u8>,
) -> Result<Vec<u8>, ReadError> {
    q.query(canister, method, &arg)
        .map_err(|e| map_query_error(e, method))
}

fn decode_nat(bytes: &[u8], method: &str) -> Result<Nat, ReadError> {
    candid::decode_one::<Nat>(bytes)
        .map_err(|e| ReadError::Failed(format!("{method}: cannot decode the reply as nat: {e}")))
}

/// `icrc1_total_supply` of the ledger.
pub fn ledger_total_supply(q: &dyn IcQuery, ledger: &Principal) -> Result<u128, ReadError> {
    let arg = candid::encode_args(())
        .map_err(|e| ReadError::Failed(format!("cannot encode the empty argument: {e}")))?;
    let reply = run(q, ledger, "icrc1_total_supply", arg)?;
    nat_to_u128(
        decode_nat(&reply, "icrc1_total_supply")?,
        "icrc1_total_supply",
    )
}

/// `icrc1_balance_of` of one escrow account.
pub fn ledger_balance_of(
    q: &dyn IcQuery,
    ledger: &Principal,
    binding: &EscrowBinding,
) -> Result<u128, ReadError> {
    let (owner, subaccount) = binding.identity().map_err(ReadError::Failed)?;
    let account = Icrc1Account {
        owner: Principal::from_slice(&owner),
        // Send the subaccount only when the binding names one; the ledger
        // treats a missing and an all-zero subaccount as the same account.
        subaccount: binding.subaccount.map(|_| subaccount.to_vec()),
    };
    let arg = candid::encode_one(&account)
        .map_err(|e| ReadError::Failed(format!("cannot encode the account: {e}")))?;
    let reply = run(q, ledger, "icrc1_balance_of", arg)?;
    nat_to_u128(decode_nat(&reply, "icrc1_balance_of")?, "icrc1_balance_of")
}

/// Sum of the recorded supplies of the listed debt-backed chains.
///
/// Fails closed: no chain ids, an unregistered chain, a missing query method or
/// an overflow are errors, never a zero.
pub fn backend_chain_supplies(
    q: &dyn IcQuery,
    backend: &Principal,
    chain_ids: &[u32],
) -> Result<u128, ReadError> {
    if chain_ids.is_empty() {
        return Err(ReadError::UnavailableGap(
            "no debt-backed chain ids are configured, and the backend has no single query for all chain supplies".into(),
        ));
    }
    let mut total: u128 = 0;
    for chain in chain_ids {
        let arg = candid::encode_one(chain)
            .map_err(|e| ReadError::Failed(format!("cannot encode the chain id: {e}")))?;
        let reply = run(q, backend, "get_chain_public_launch_status", arg)?;
        let status = candid::decode_one::<LaunchStatusSlice>(&reply).map_err(|e| {
            ReadError::Failed(format!(
                "get_chain_public_launch_status: cannot decode the reply: {e}"
            ))
        })?;
        if status.status.is_none() {
            return Err(ReadError::Failed(format!(
                "chain {chain} is not registered on the backend; its recorded supply would read as 0"
            )));
        }
        let supply = nat_to_u128(status.chain_supply_e8s, "chain_supply_e8s")?;
        total = total.checked_add(supply).ok_or_else(|| {
            ReadError::Failed("the debt-backed chain supplies overflow u128".into())
        })?;
    }
    Ok(total)
}

// ---------------------------------------------------------------------------
// Real implementation over ic-agent
// ---------------------------------------------------------------------------

/// Query calls through `ic-agent`: anonymous identity, response verification on
/// (the agent's default; nothing here turns it off), IC mainnet root key (the
/// agent's built-in one; a local replica is not supported).
pub struct AgentQuery {
    agent: ic_agent::Agent,
    runtime: tokio::runtime::Runtime,
}

/// Largest query reply accepted.
const MAX_REPLY_BYTES: usize = 1 << 20;

impl AgentQuery {
    /// Build a client for the endpoint `url`. The URL is operator input and may
    /// carry a key, so no error here repeats it.
    pub fn new(url: &str) -> Result<AgentQuery, String> {
        check_endpoint_url(url)?;
        let client = reqwest::Client::builder()
            .use_rustls_tls()
            .timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| format!("cannot build the HTTP client: {}", e.without_url()))?;
        let transport =
            ic_agent::agent::http_transport::ReqwestTransport::create_with_client(url, client)
                .map_err(|_| "the IC endpoint URL is not usable".to_string())?
                .with_max_response_body_size(MAX_REPLY_BYTES);
        let agent = ic_agent::Agent::builder()
            .with_transport(transport)
            .with_identity(ic_agent::identity::AnonymousIdentity)
            .build()
            .map_err(|e| format!("cannot build the IC agent: {e}"))?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("cannot start the async runtime: {e}"))?;
        Ok(AgentQuery { agent, runtime })
    }
}

/// An endpoint must be https; plain http is accepted only for a loopback host
/// (a local mock). Never echoes the URL.
pub fn check_endpoint_url(url: &str) -> Result<(), String> {
    if url.starts_with("https://") {
        return Ok(());
    }
    if let Some(rest) = url.strip_prefix("http://") {
        let host = rest.split(['/', ':', '?']).next().unwrap_or("");
        if host == "localhost" || host == "127.0.0.1" || host == "[::1]" {
            return Ok(());
        }
    }
    Err("an endpoint URL must be https (http only for a loopback host)".into())
}

/// Classify an agent error. A reject saying the canister has no such query
/// method is `MethodNotFound`; everything else is `Failed`.
fn classify(error: &ic_agent::AgentError) -> IcQueryError {
    use ic_agent::AgentError::{CertifiedReject, UncertifiedReject};
    match error {
        CertifiedReject(r) | UncertifiedReject(r)
            if r.reject_message.contains("has no query method") =>
        {
            IcQueryError::MethodNotFound(r.reject_message.clone())
        }
        other => IcQueryError::Failed(other.to_string()),
    }
}

impl IcQuery for AgentQuery {
    fn query(
        &self,
        canister: &Principal,
        method: &str,
        arg: &[u8],
    ) -> Result<Vec<u8>, IcQueryError> {
        self.runtime
            .block_on(
                self.agent
                    .query(canister, method)
                    .with_arg(arg.to_vec())
                    .call(),
            )
            .map_err(|e| classify(&e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    type Handler = Box<dyn Fn(&Principal, &str, &[u8]) -> Result<Vec<u8>, IcQueryError>>;

    /// A scripted canister: answers by (canister, method) and records calls.
    pub(crate) struct Canned {
        pub handler: Handler,
        pub calls: RefCell<Vec<(Principal, String)>>,
    }

    impl Canned {
        pub(crate) fn new(handler: Handler) -> Canned {
            Canned {
                handler,
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl IcQuery for Canned {
        fn query(
            &self,
            canister: &Principal,
            method: &str,
            arg: &[u8],
        ) -> Result<Vec<u8>, IcQueryError> {
            self.calls
                .borrow_mut()
                .push((*canister, method.to_string()));
            (self.handler)(canister, method, arg)
        }
    }

    fn p(b: u8) -> Principal {
        Principal::from_slice(&[b, 0xAA])
    }

    fn nat(v: u128) -> Vec<u8> {
        candid::encode_one(Nat::from(v)).unwrap()
    }

    /// A larger record than we decode, as the real backend replies.
    #[derive(CandidType, Deserialize)]
    struct FullStatus {
        status: Option<ChainStatusSlice>,
        chain_supply_e8s: Nat,
        chain_reserve_backing_e8s: Nat,
        liquidation_configured: bool,
        finality_depth: Option<u32>,
    }

    fn status_reply(registered: bool, supply: u128) -> Vec<u8> {
        candid::encode_one(FullStatus {
            status: registered.then_some(ChainStatusSlice::Registered),
            chain_supply_e8s: Nat::from(supply),
            chain_reserve_backing_e8s: Nat::from(1u32),
            liquidation_configured: true,
            finality_depth: Some(12),
        })
        .unwrap()
    }

    #[test]
    fn ledger_total_supply_is_decoded_from_a_candid_reply() {
        let ledger = p(1);
        let q = Canned::new(Box::new(move |c, m, arg| {
            assert_eq!(*c, ledger);
            assert_eq!(m, "icrc1_total_supply");
            assert_eq!(arg, candid::encode_args(()).unwrap().as_slice());
            Ok(nat(123_456_789_012))
        }));
        assert_eq!(ledger_total_supply(&q, &ledger), Ok(123_456_789_012));
    }

    #[test]
    fn balance_of_sends_the_account_and_decodes_the_nat() {
        let ledger = p(1);
        let binding = EscrowBinding::from_principal_bytes(b"synthetic-escrow-x", None);
        let mut sub = [0u8; 32];
        sub[31] = 9;
        let with_sub = EscrowBinding::from_principal_bytes(b"synthetic-escrow-x", Some(sub));
        let q = Canned::new(Box::new(|_, m, arg| {
            assert_eq!(m, "icrc1_balance_of");
            let account: Icrc1Account = candid::decode_one(arg).unwrap();
            assert_eq!(account.owner.as_slice(), b"synthetic-escrow-x");
            // 200.5 icUSD when a subaccount is named, 7 e8s when it is not.
            Ok(nat(if account.subaccount.is_some() {
                20_050_000_000
            } else {
                7
            }))
        }));
        assert_eq!(ledger_balance_of(&q, &ledger, &binding), Ok(7));
        assert_eq!(
            ledger_balance_of(&q, &ledger, &with_sub),
            Ok(20_050_000_000)
        );
        // The subaccount went over the wire byte-exact.
        let q2 = Canned::new(Box::new(move |_, _, arg| {
            let a: Icrc1Account = candid::decode_one(arg).unwrap();
            assert_eq!(a.subaccount.as_deref(), Some(&sub[..]));
            Ok(nat(1))
        }));
        assert_eq!(ledger_balance_of(&q2, &ledger, &with_sub), Ok(1));
    }

    #[test]
    fn a_reply_that_is_not_a_nat_or_too_big_is_a_failed_read() {
        let ledger = p(1);
        let text = Canned::new(Box::new(|_, _, _| {
            Ok(candid::encode_one("not a nat").unwrap())
        }));
        assert!(matches!(
            ledger_total_supply(&text, &ledger),
            Err(ReadError::Failed(m)) if m.contains("cannot decode")
        ));
        let big = Canned::new(Box::new(|_, _, _| {
            // 2^128: one past u128::MAX.
            let n: Nat = Nat::parse(b"340282366920938463463374607431768211456").unwrap();
            Ok(candid::encode_one(n).unwrap())
        }));
        assert!(matches!(
            ledger_total_supply(&big, &ledger),
            Err(ReadError::Failed(m)) if m.contains("does not fit")
        ));
    }

    #[test]
    fn backend_chain_supplies_are_summed_from_the_launch_status_query() {
        let backend = p(2);
        let q = Canned::new(Box::new(move |c, m, arg| {
            assert_eq!(*c, backend);
            assert_eq!(m, "get_chain_public_launch_status");
            let chain: u32 = candid::decode_one(arg).unwrap();
            match chain {
                1030 => Ok(status_reply(true, 500_000_000)),
                143 => Ok(status_reply(true, 25)),
                other => panic!("unexpected chain {other}"),
            }
        }));
        assert_eq!(
            backend_chain_supplies(&q, &backend, &[1030, 143]),
            Ok(500_000_025)
        );
        assert_eq!(q.calls.borrow().len(), 2);
    }

    #[test]
    fn an_unregistered_chain_is_an_error_not_zero() {
        let backend = p(2);
        let q = Canned::new(Box::new(|_, _, _| Ok(status_reply(false, 0))));
        assert!(matches!(
            backend_chain_supplies(&q, &backend, &[4242]),
            Err(ReadError::Failed(m)) if m.contains("not registered")
        ));
    }

    #[test]
    fn a_missing_backend_query_is_unavailable_and_names_the_gap() {
        let backend = p(2);
        let q = Canned::new(Box::new(|_, _, _| {
            Err(IcQueryError::MethodNotFound(
                "Canister has no query method 'get_chain_public_launch_status'".into(),
            ))
        }));
        match backend_chain_supplies(&q, &backend, &[1030]) {
            Err(ReadError::UnavailableGap(g)) => {
                assert!(g.contains("get_chain_public_launch_status"), "{g}")
            }
            other => panic!("expected an unavailable gap, got {other:?}"),
        }
        // No chain ids configured: also a named gap, and no call is made.
        let none = Canned::new(Box::new(|_, _, _| panic!("must not be called")));
        assert!(matches!(
            backend_chain_supplies(&none, &backend, &[]),
            Err(ReadError::UnavailableGap(_))
        ));
    }

    #[test]
    fn chain_supply_overflow_is_an_error() {
        let backend = p(2);
        let big = Nat::parse(b"340282366920938463463374607431768211455").unwrap(); // u128::MAX
        let q = Canned::new(Box::new(move |_, _, _| {
            Ok(candid::encode_one(FullStatus {
                status: Some(ChainStatusSlice::Registered),
                chain_supply_e8s: big.clone(),
                chain_reserve_backing_e8s: Nat::from(0u32),
                liquidation_configured: false,
                finality_depth: None,
            })
            .unwrap())
        }));
        assert!(matches!(
            backend_chain_supplies(&q, &backend, &[1, 2]),
            Err(ReadError::Failed(m)) if m.contains("overflow")
        ));
    }

    #[test]
    fn endpoint_urls_must_be_https_or_loopback() {
        assert!(check_endpoint_url("https://example.invalid").is_ok());
        assert!(check_endpoint_url("http://127.0.0.1:8080").is_ok());
        assert!(check_endpoint_url("http://localhost/x").is_ok());
        assert!(check_endpoint_url("http://example.invalid").is_err());
        assert!(check_endpoint_url("ftp://example.invalid").is_err());
        assert!(check_endpoint_url("example.invalid").is_err());
    }

    #[test]
    fn agent_rejects_are_classified_without_the_url() {
        use ic_agent::AgentError;
        let reject = |msg: &str| {
            ic_agent::agent_error::AgentError::UncertifiedReject(ic_transport_types_reject(msg))
        };
        let missing = classify(&reject("Canister aaaaa-aa has no query method 'x'"));
        assert!(matches!(missing, IcQueryError::MethodNotFound(_)));
        let other = classify(&reject("Canister trapped"));
        assert!(matches!(other, IcQueryError::Failed(_)));
        assert!(matches!(
            classify(&AgentError::TimeoutWaitingForResponse()),
            IcQueryError::Failed(_)
        ));
    }

    fn ic_transport_types_reject(msg: &str) -> ic_agent::agent::RejectResponse {
        ic_agent::agent::RejectResponse {
            reject_code: ic_agent::agent::RejectCode::DestinationInvalid,
            reject_message: msg.to_string(),
            error_code: None,
        }
    }
}
