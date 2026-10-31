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
//! * a bridge instance's own `get_supply_audit : () -> (SupplyAudit) query`
//!   ([`instance_supply_audit`]), decoded from the exact record in
//!   `rumi_icusd_bridge.did`; it is a self-report that is only compared with
//!   the independent reads.
//!
//! * a Solana bridge instance has no `get_supply_audit`; its self-report is assembled from its two
//!   existing queries `get_buckets` and `get_bridge_info` ([`solana_instance_audit`]), decoded from
//!   the exact records in `rumi_icusd_sol_bridge.did`. It too is only compared with the independent
//!   reads and is the explanation of the custody (`RC`, `BU`, `stray`), never a measurement.
//!
//! [`AgentQuery`] is the real implementation over `ic-agent`: anonymous
//! identity, query calls only, response (query signature) verification left on,
//! which is the agent's default. This module never runs by itself; nothing in
//! the crate calls [`AgentQuery`] unless an explicit endpoint config is given.

use candid::{CandidType, Deserialize, Nat, Principal};

use crate::collect::{InstanceAudit, ReadError};
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

/// The fields of the instance's `SupplyAudit` record that the collector uses.
/// The names are the instance's own (Base-named by decision D1a) and are
/// checked against `rumi_icusd_bridge.did` in the tests. Candid decoding skips
/// the record's other fields, so a larger record decodes into this.
#[derive(CandidType, Deserialize, Debug, Clone)]
pub struct SupplyAuditSlice {
    pub escrow_account: Icrc1Account,
    pub escrow_balance_e8s: Option<Nat>,
    pub outstanding_base_e8s: Nat,
    pub pending_mint_e8s: Nat,
    pub inflight_mint_e8s: Nat,
    pub pending_release_e8s: Nat,
    pub pending_refund_e8s: Nat,
    pub held_e8s: Nat,
    pub fees_e8s: Nat,
    pub reserved_pulls_e8s: Nat,
    pub unknown_outflow_e8s: Nat,
    pub liabilities_e8s: Nat,
    pub required_escrow_e8s: Nat,
    pub coverage_ok: Option<bool>,
    pub base_total_supply_e8s: Option<Nat>,
    pub base_supply_block: Option<u64>,
}

/// The fields of the Solana instance's `BucketsView` the collector uses. Candid decoding skips the
/// record's other fields. Pinned to `rumi_icusd_sol_bridge.did` in the tests.
#[derive(CandidType, Deserialize, Debug, Clone)]
pub struct SolanaBucketsSlice {
    pub outstanding: Nat,
    pub pending_mint: Nat,
    pub return_custodied: Nat,
    pub burn_unknown: Nat,
    pub pending_release: Nat,
    pub pending_refund: Nat,
    pub held: Nat,
    pub fees: Nat,
    pub reserved_pulls: Nat,
    pub unknown_outflow: Nat,
    pub stray: Nat,
}

/// The one field of the Solana instance's `BridgeInfo` the collector uses: the escrow subaccount
/// (hex), the owner being the instance canister itself.
#[derive(CandidType, Deserialize, Debug, Clone)]
pub struct SolanaBridgeInfoSlice {
    pub escrow_subaccount: String,
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

/// The bridge instance's own `get_supply_audit` (a query). The reply is a
/// self-report: the collector shows it and checks it against independent reads,
/// and never uses it as an escrow balance or a destination supply.
pub fn instance_supply_audit(
    q: &dyn IcQuery,
    instance: &Principal,
) -> Result<InstanceAudit, ReadError> {
    let arg = candid::encode_args(())
        .map_err(|e| ReadError::Failed(format!("cannot encode the empty argument: {e}")))?;
    let reply = run(q, instance, "get_supply_audit", arg)?;
    let a = candid::decode_one::<SupplyAuditSlice>(&reply).map_err(|e| {
        ReadError::Failed(format!(
            "get_supply_audit: cannot decode the reply as a SupplyAudit: {e}"
        ))
    })?;
    let opt = |v: Option<Nat>, what: &str| v.map(|n| nat_to_u128(n, what)).transpose();
    let subaccount = match a.escrow_account.subaccount {
        None => None,
        Some(bytes) => Some(<[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| {
            ReadError::Failed("get_supply_audit: the escrow subaccount is not 32 bytes".into())
        })?),
    };
    Ok(InstanceAudit {
        escrow_account: EscrowBinding::from_principal_bytes(
            a.escrow_account.owner.as_slice(),
            subaccount,
        ),
        escrow_balance_e8s: opt(a.escrow_balance_e8s, "escrow_balance_e8s")?,
        outstanding_e8s: nat_to_u128(a.outstanding_base_e8s, "outstanding_base_e8s")?,
        pending_mint_e8s: nat_to_u128(a.pending_mint_e8s, "pending_mint_e8s")?,
        inflight_mint_e8s: nat_to_u128(a.inflight_mint_e8s, "inflight_mint_e8s")?,
        pending_release_e8s: nat_to_u128(a.pending_release_e8s, "pending_release_e8s")?,
        pending_refund_e8s: nat_to_u128(a.pending_refund_e8s, "pending_refund_e8s")?,
        held_e8s: nat_to_u128(a.held_e8s, "held_e8s")?,
        fees_e8s: nat_to_u128(a.fees_e8s, "fees_e8s")?,
        reserved_pulls_e8s: nat_to_u128(a.reserved_pulls_e8s, "reserved_pulls_e8s")?,
        unknown_outflow_e8s: nat_to_u128(a.unknown_outflow_e8s, "unknown_outflow_e8s")?,
        liabilities_e8s: nat_to_u128(a.liabilities_e8s, "liabilities_e8s")?,
        required_escrow_e8s: nat_to_u128(a.required_escrow_e8s, "required_escrow_e8s")?,
        coverage_ok: a.coverage_ok,
        reported_supply_e8s: opt(a.base_total_supply_e8s, "base_total_supply_e8s")?,
        reported_supply_block: a.base_supply_block,
        return_custodied_e8s: None,
        burn_unknown_e8s: None,
        stray_e8s: None,
    })
}

/// A Solana bridge instance's self-report, assembled from `get_buckets` and `get_bridge_info`. The
/// instance has no query for its in-flight mint, so `inflight_mint_e8s` is `pending_mint` (its upper
/// bound): the supply bound can only be looser than the instance's own, never tighter. `liabilities`
/// and `required_escrow` are the instance's own formulas applied to its reported buckets
/// (`O + PM + RC + BU + PR + pending refund + held + fees`, and that less `unknown_outflow`, floored
/// at zero). The escrow account is the instance canister with its escrow subaccount. Nothing here is
/// a measurement: it is shown beside the independent reads and checked against them.
pub fn solana_instance_audit(
    q: &dyn IcQuery,
    instance: &Principal,
) -> Result<InstanceAudit, ReadError> {
    let none = candid::encode_args(())
        .map_err(|e| ReadError::Failed(format!("cannot encode the empty argument: {e}")))?;
    let buckets_reply = run(q, instance, "get_buckets", none.clone())?;
    let b = candid::decode_one::<SolanaBucketsSlice>(&buckets_reply).map_err(|e| {
        ReadError::Failed(format!("get_buckets: cannot decode the reply as a BucketsView: {e}"))
    })?;
    let info_reply = run(q, instance, "get_bridge_info", none)?;
    let info = candid::decode_one::<SolanaBridgeInfoSlice>(&info_reply).map_err(|e| {
        ReadError::Failed(format!("get_bridge_info: cannot decode the reply as a BridgeInfo: {e}"))
    })?;
    let subaccount: [u8; 32] = {
        let text = info.escrow_subaccount.as_str();
        let valid = text.len() == 64 && text.bytes().all(|c| c.is_ascii_hexdigit());
        if !valid {
            return Err(ReadError::Failed(
                "get_bridge_info: the escrow subaccount is not 64 hex characters".into(),
            ));
        }
        let mut out = [0u8; 32];
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = u8::from_str_radix(&text[2 * i..2 * i + 2], 16)
                .map_err(|e| ReadError::Failed(format!("get_bridge_info: {e}")))?;
        }
        out
    };
    let outstanding = nat_to_u128(b.outstanding, "outstanding")?;
    let pending_mint = nat_to_u128(b.pending_mint, "pending_mint")?;
    let rc = nat_to_u128(b.return_custodied, "return_custodied")?;
    let bu = nat_to_u128(b.burn_unknown, "burn_unknown")?;
    let pending_release = nat_to_u128(b.pending_release, "pending_release")?;
    let pending_refund = nat_to_u128(b.pending_refund, "pending_refund")?;
    let held = nat_to_u128(b.held, "held")?;
    let fees = nat_to_u128(b.fees, "fees")?;
    let reserved_pulls = nat_to_u128(b.reserved_pulls, "reserved_pulls")?;
    let unknown_outflow = nat_to_u128(b.unknown_outflow, "unknown_outflow")?;
    let stray = nat_to_u128(b.stray, "stray")?;
    let liabilities = [outstanding, pending_mint, rc, bu, pending_release, pending_refund, held, fees]
        .iter()
        .try_fold(0u128, |acc, v| acc.checked_add(*v))
        .ok_or_else(|| ReadError::Failed("get_buckets: the liabilities overflow u128".into()))?;
    Ok(InstanceAudit {
        escrow_account: EscrowBinding::from_principal_bytes(instance.as_slice(), Some(subaccount)),
        escrow_balance_e8s: None,
        outstanding_e8s: outstanding,
        pending_mint_e8s: pending_mint,
        inflight_mint_e8s: pending_mint,
        pending_release_e8s: pending_release,
        pending_refund_e8s: pending_refund,
        held_e8s: held,
        fees_e8s: fees,
        reserved_pulls_e8s: reserved_pulls,
        unknown_outflow_e8s: unknown_outflow,
        liabilities_e8s: liabilities,
        required_escrow_e8s: liabilities.saturating_sub(unknown_outflow),
        coverage_ok: None,
        reported_supply_e8s: None,
        reported_supply_block: None,
        return_custodied_e8s: Some(rc),
        burn_unknown_e8s: Some(bu),
        stray_e8s: Some(stray),
    })
}

/// A chain id listed twice would be counted twice in the sum, so the list must
/// be a set. Returns the first repeated id.
pub fn check_unique_chain_ids(chain_ids: &[u32]) -> Result<(), String> {
    let mut seen = std::collections::BTreeSet::new();
    for id in chain_ids {
        if !seen.insert(*id) {
            return Err(format!(
                "debt-backed chain id {id} is listed more than once; each chain's supply is counted once"
            ));
        }
    }
    Ok(())
}

/// Sum of the recorded supplies of the listed debt-backed chains.
///
/// Fails closed: no chain ids, a duplicate chain id (rejected before any
/// query), an unregistered chain, a missing query method or an overflow are
/// errors, never a zero.
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
    check_unique_chain_ids(chain_ids).map_err(ReadError::Failed)?;
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
    fn a_duplicate_chain_id_is_rejected_before_any_query() {
        // [1030, 1030] would add the same supply twice.
        let backend = p(2);
        let none = Canned::new(Box::new(|_, _, _| panic!("must not be called")));
        assert!(matches!(
            backend_chain_supplies(&none, &backend, &[1030, 1030]),
            Err(ReadError::Failed(m)) if m.contains("1030") && m.contains("more than once")
        ));
        // Not adjacent either.
        assert!(backend_chain_supplies(&none, &backend, &[1030, 143, 1030]).is_err());
        assert!(none.calls.borrow().is_empty());
        assert!(check_unique_chain_ids(&[1030, 143]).is_ok());
        assert!(check_unique_chain_ids(&[]).is_ok());
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

    /// The instance's full `SupplyAudit` record, field for field as in
    /// `rumi_icusd_bridge.did`. `supply_audit_record_matches_the_bridge_did`
    /// pins that, so a canned reply built from it is a real-shaped reply.
    #[derive(CandidType, Deserialize, Clone)]
    struct FullSupplyAudit {
        fees_e8s: Nat,
        icp_ledger_total_supply_e8s: Option<Nat>,
        inflight_mint_e8s: Nat,
        pending_refund_e8s: Nat,
        base_total_supply_e8s: Option<Nat>,
        reserved_pulls_e8s: Nat,
        pending_mint_e8s: Nat,
        escrow_account: Icrc1Account,
        circulating_on_base_e8s: Option<Nat>,
        base_supply_exact_match: Option<bool>,
        escrow_balance_e8s: Option<Nat>,
        outstanding_base_e8s: Nat,
        surplus_e8s: Option<Nat>,
        base_supply_block: Option<u64>,
        reporting_note: String,
        required_escrow_e8s: Nat,
        liabilities_e8s: Nat,
        held_e8s: Nat,
        circulating_on_icp_e8s: Option<Nat>,
        pending_release_e8s: Nat,
        escrow_balance_checked_at_ns: Option<u64>,
        deficit_e8s: Option<Nat>,
        unknown_outflow_e8s: Nat,
        coverage_ok: Option<bool>,
        base_supply_within_bound: Option<bool>,
    }

    fn full_audit() -> FullSupplyAudit {
        let n = |v: u128| Nat::from(v);
        FullSupplyAudit {
            fees_e8s: n(3),
            icp_ledger_total_supply_e8s: Some(n(100_000_000_000)),
            inflight_mint_e8s: n(12_345_678),
            pending_refund_e8s: n(5),
            base_total_supply_e8s: Some(n(20_000_000_000)),
            reserved_pulls_e8s: n(7),
            pending_mint_e8s: n(12_345_678),
            escrow_account: Icrc1Account {
                owner: Principal::from_slice(b"synthetic-instance-x"),
                subaccount: Some(vec![9u8; 32]),
            },
            circulating_on_base_e8s: Some(n(20_000_000_000)),
            base_supply_exact_match: Some(true),
            escrow_balance_e8s: Some(n(20_012_345_700)),
            outstanding_base_e8s: n(20_000_000_000),
            surplus_e8s: Some(n(0)),
            base_supply_block: Some(1234),
            reporting_note: "synthetic".into(),
            required_escrow_e8s: n(20_012_345_700),
            liabilities_e8s: n(20_012_345_700),
            held_e8s: n(11),
            circulating_on_icp_e8s: Some(n(79_987_654_300)),
            pending_release_e8s: n(13),
            escrow_balance_checked_at_ns: Some(1),
            deficit_e8s: None,
            unknown_outflow_e8s: n(2),
            coverage_ok: Some(true),
            base_supply_within_bound: Some(true),
        }
    }

    /// The `type SupplyAudit = record { ... }` block of the bridge's .did as
    /// `(field, type)` pairs.
    fn did_supply_audit_fields() -> Vec<(String, String)> {
        let did = include_str!("../../../rumi_icusd_bridge/rumi_icusd_bridge.did");
        let start = did
            .find("type SupplyAudit = record {")
            .expect("SupplyAudit in the bridge .did");
        let body = &did[start..];
        let end = body.find("};").expect("end of SupplyAudit");
        body["type SupplyAudit = record {".len()..end]
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(|l| {
                let (name, ty) = l.trim_end_matches(';').split_once(" : ").unwrap();
                (name.to_string(), ty.to_string())
            })
            .collect()
    }

    #[test]
    fn supply_audit_record_matches_the_bridge_did() {
        use candid::types::TypeInner;
        let mut ours: Vec<(String, String)> = match <FullSupplyAudit as CandidType>::ty().as_ref() {
            TypeInner::Record(fields) => fields
                .iter()
                .map(|f| (f.id.to_string(), f.ty.to_string()))
                .collect(),
            other => panic!("not a record: {other:?}"),
        };
        let mut theirs = did_supply_audit_fields();
        // The .did names the account type; ours is the same record inline.
        for (_, ty) in theirs.iter_mut() {
            if ty == "Account" {
                *ty = "record {owner:principal; subaccount:opt vec nat8}".to_string();
            }
        }
        for (_, ty) in ours.iter_mut() {
            *ty = ty.replace("blob", "vec nat8");
        }
        ours.sort();
        theirs.sort();
        // Compare field names exactly, and types ignoring spacing.
        let norm = |v: &[(String, String)]| -> Vec<(String, String)> {
            v.iter()
                .map(|(n, t)| (n.clone(), t.replace(' ', "")))
                .collect()
        };
        assert_eq!(norm(&ours), norm(&theirs));
        assert_eq!(ours.len(), 25);
    }

    #[test]
    fn instance_audit_is_decoded_from_the_real_record_shape() {
        let instance = p(7);
        let reply = candid::encode_one(full_audit()).unwrap();
        let q = Canned::new(Box::new(move |c, m, arg| {
            assert_eq!(*c, instance);
            assert_eq!(m, "get_supply_audit");
            assert_eq!(arg, candid::encode_args(()).unwrap().as_slice());
            Ok(reply.clone())
        }));
        let a = instance_supply_audit(&q, &instance).unwrap();
        assert_eq!(a.outstanding_e8s, 20_000_000_000);
        assert_eq!(a.pending_mint_e8s, 12_345_678);
        assert_eq!(a.inflight_mint_e8s, 12_345_678);
        assert_eq!(a.pending_release_e8s, 13);
        assert_eq!(a.pending_refund_e8s, 5);
        assert_eq!(a.held_e8s, 11);
        assert_eq!(a.fees_e8s, 3);
        assert_eq!(a.reserved_pulls_e8s, 7);
        assert_eq!(a.unknown_outflow_e8s, 2);
        assert_eq!(a.liabilities_e8s, 20_012_345_700);
        assert_eq!(a.required_escrow_e8s, 20_012_345_700);
        assert_eq!(a.coverage_ok, Some(true));
        assert_eq!(a.escrow_balance_e8s, Some(20_012_345_700));
        assert_eq!(a.reported_supply_e8s, Some(20_000_000_000));
        assert_eq!(a.reported_supply_block, Some(1234));
        // The escrow account survives byte-exact.
        let (owner, sub) = a.escrow_account.identity().unwrap();
        assert_eq!(owner, b"synthetic-instance-x".to_vec());
        assert_eq!(sub, [9u8; 32]);
    }

    #[test]
    fn instance_audit_failures_are_named() {
        let instance = p(7);
        // Optional fields may be null.
        let mut sparse = full_audit();
        sparse.escrow_balance_e8s = None;
        sparse.base_total_supply_e8s = None;
        sparse.base_supply_block = None;
        sparse.coverage_ok = None;
        sparse.escrow_account.subaccount = None;
        let reply = candid::encode_one(sparse).unwrap();
        let q = Canned::new(Box::new(move |_, _, _| Ok(reply.clone())));
        let a = instance_supply_audit(&q, &instance).unwrap();
        assert_eq!(
            (a.escrow_balance_e8s, a.reported_supply_e8s, a.coverage_ok),
            (None, None, None)
        );
        assert_eq!(a.escrow_account.subaccount, None);

        // A missing method is an unavailable gap naming get_supply_audit.
        let gone = Canned::new(Box::new(|_, _, _| {
            Err(IcQueryError::MethodNotFound("has no query method".into()))
        }));
        assert!(matches!(
            instance_supply_audit(&gone, &instance),
            Err(ReadError::UnavailableGap(g)) if g.contains("get_supply_audit")
        ));
        // A reply that is not a SupplyAudit, and a short subaccount, fail.
        let junk = Canned::new(Box::new(|_, _, _| Ok(nat(1))));
        assert!(matches!(
            instance_supply_audit(&junk, &instance),
            Err(ReadError::Failed(m)) if m.contains("cannot decode")
        ));
        let mut short = full_audit();
        short.escrow_account.subaccount = Some(vec![1, 2, 3]);
        let reply = candid::encode_one(short).unwrap();
        let q = Canned::new(Box::new(move |_, _, _| Ok(reply.clone())));
        assert!(matches!(
            instance_supply_audit(&q, &instance),
            Err(ReadError::Failed(m)) if m.contains("32 bytes")
        ));
    }

    /// The full `BucketsView` of the Solana instance, field for field as in
    /// `rumi_icusd_sol_bridge.did` (`solana_buckets_record_matches_the_instance_did` pins that).
    #[derive(CandidType, Deserialize, Clone)]
    struct FullBuckets {
        reserved_pulls: Nat,
        pending_mint: Nat,
        unknown_outflow: Nat,
        pending_refund: Nat,
        outstanding: Nat,
        fees: Nat,
        held: Nat,
        burn_unknown: Nat,
        stray: Nat,
        return_custodied: Nat,
        pending_release: Nat,
    }

    fn full_buckets() -> FullBuckets {
        let n = |v: u128| Nat::from(v);
        FullBuckets {
            reserved_pulls: n(7),
            pending_mint: n(100),
            unknown_outflow: n(2),
            pending_refund: n(5),
            outstanding: n(9_000_000_000),
            fees: n(3),
            held: n(11),
            burn_unknown: n(200_000_000),
            stray: n(50_000_000),
            return_custodied: n(1_000_000_000),
            pending_release: n(13),
        }
    }

    /// The fields of a `type <name> = record { ... };` block of the Solana instance's .did.
    fn sol_did_record_fields(name: &str) -> Vec<(String, String)> {
        let did = include_str!("../../../rumi_icusd_sol_bridge/rumi_icusd_sol_bridge.did");
        let header = format!("type {name} = record {{");
        let start = did.find(&header).unwrap_or_else(|| panic!("{name} in the Solana .did"));
        let body = &did[start..];
        let end = body.find("};").expect("end of the record");
        body[header.len()..end]
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(|l| {
                let (field, ty) = l.trim_end_matches(';').split_once(" : ").unwrap();
                (field.to_string(), ty.to_string())
            })
            .collect()
    }

    #[test]
    fn solana_buckets_record_matches_the_instance_did() {
        use candid::types::TypeInner;
        let mut ours: Vec<(String, String)> = match <FullBuckets as CandidType>::ty().as_ref() {
            TypeInner::Record(fields) => fields.iter().map(|f| (f.id.to_string(), f.ty.to_string())).collect(),
            other => panic!("not a record: {other:?}"),
        };
        let mut theirs = sol_did_record_fields("BucketsView");
        ours.sort();
        theirs.sort();
        assert_eq!(ours, theirs);
        assert_eq!(ours.len(), 11);
        // The fields the collector reads are all there, with the names it uses.
        let wanted: Vec<String> = match <SolanaBucketsSlice as CandidType>::ty().as_ref() {
            TypeInner::Record(fields) => fields.iter().map(|f| f.id.to_string()).collect(),
            other => panic!("not a record: {other:?}"),
        };
        for w in wanted {
            assert!(theirs.iter().any(|(f, ty)| *f == w && ty == "nat"), "BucketsView has no nat field {w}");
        }
        let info = sol_did_record_fields("BridgeInfo");
        assert!(info.iter().any(|(f, ty)| f == "escrow_subaccount" && ty == "text"));
    }

    /// A `BridgeInfo`-shaped reply (a superset of the one field the collector uses).
    #[derive(CandidType, Deserialize, Clone)]
    struct FullInfo {
        escrow_subaccount: String,
        genesis_hash: String,
        initialized: bool,
    }

    fn info_reply(subaccount_hex: &str) -> Vec<u8> {
        candid::encode_one(FullInfo {
            escrow_subaccount: subaccount_hex.into(),
            genesis_hash: "synthetic".into(),
            initialized: true,
        })
        .unwrap()
    }

    #[test]
    fn a_solana_instance_audit_is_assembled_from_get_buckets_and_get_bridge_info() {
        let instance = p(8);
        let sub_hex = "12f37a581370e84a29695a45f63396cd0b13e8a51179e770e0a2ae7d02b720a4";
        let buckets = candid::encode_one(full_buckets()).unwrap();
        let info = info_reply(sub_hex);
        let q = Canned::new(Box::new(move |c, m, arg| {
            assert_eq!(*c, instance);
            assert_eq!(arg, candid::encode_args(()).unwrap().as_slice());
            match m {
                "get_buckets" => Ok(buckets.clone()),
                "get_bridge_info" => Ok(info.clone()),
                other => panic!("unexpected {other}"),
            }
        }));
        let a = solana_instance_audit(&q, &instance).unwrap();
        assert_eq!(a.outstanding_e8s, 9_000_000_000);
        assert_eq!(
            (a.return_custodied_e8s, a.burn_unknown_e8s, a.stray_e8s),
            (Some(1_000_000_000), Some(200_000_000), Some(50_000_000))
        );
        // pending_mint is the upper bound of the in-flight mint (there is no query for the in-flight part).
        assert_eq!((a.pending_mint_e8s, a.inflight_mint_e8s), (100, 100));
        // O + PM + RC + BU + PR + pending refund + held + fees, and that less unknown_outflow.
        let liabilities = 9_000_000_000u128 + 100 + 1_000_000_000 + 200_000_000 + 13 + 5 + 11 + 3;
        assert_eq!((a.liabilities_e8s, a.required_escrow_e8s), (liabilities, liabilities - 2));
        // Nothing is invented for what the instance cannot report.
        assert_eq!((a.escrow_balance_e8s, a.coverage_ok, a.reported_supply_e8s, a.reported_supply_block), (None, None, None, None));
        // The escrow is the instance canister with its escrow subaccount.
        let (owner, sub) = a.escrow_account.identity().unwrap();
        assert_eq!(owner, instance.as_slice().to_vec());
        assert_eq!(hex_of(&sub), sub_hex);
        assert_eq!(q.calls.borrow().iter().map(|(_, m)| m.clone()).collect::<Vec<_>>(), vec!["get_buckets", "get_bridge_info"]);
    }

    fn hex_of(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn a_solana_instance_audit_fails_closed() {
        let instance = p(8);
        let buckets = candid::encode_one(full_buckets()).unwrap();
        // A missing method is an unavailable gap naming it.
        let gone = Canned::new(Box::new(|_, _, _| Err(IcQueryError::MethodNotFound("has no query method".into()))));
        assert!(matches!(solana_instance_audit(&gone, &instance), Err(ReadError::UnavailableGap(g)) if g.contains("get_buckets")));
        // A reply that is not a BucketsView fails.
        let junk = Canned::new(Box::new(|_, _, _| Ok(nat(1))));
        assert!(matches!(solana_instance_audit(&junk, &instance), Err(ReadError::Failed(m)) if m.contains("cannot decode")));
        // A bad escrow subaccount fails (not 64 hex characters, or not hex).
        for bad in ["", "12", &"zz".repeat(32)] {
            let b = buckets.clone();
            let i = info_reply(bad);
            let q = Canned::new(Box::new(move |_, m, _| Ok(if m == "get_buckets" { b.clone() } else { i.clone() })));
            assert!(matches!(solana_instance_audit(&q, &instance), Err(ReadError::Failed(m)) if m.contains("64 hex")), "{bad:?}");
        }
        // An overflowing sum is a failed read, never a wrapped number.
        let mut huge = full_buckets();
        let max = Nat::parse(b"340282366920938463463374607431768211455").unwrap();
        huge.outstanding = max.clone();
        huge.held = max;
        let reply = candid::encode_one(huge).unwrap();
        let i = info_reply(&"ab".repeat(32));
        let q = Canned::new(Box::new(move |_, m, _| Ok(if m == "get_buckets" { reply.clone() } else { i.clone() })));
        assert!(matches!(solana_instance_audit(&q, &instance), Err(ReadError::Failed(m)) if m.contains("overflow")));
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
