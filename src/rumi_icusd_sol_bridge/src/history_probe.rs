//! History-availability probe (`chain-solana.md` section 4, P5; blocker
//! `solana-history-availability-unmeasured`).
//!
//! The nonce lane relies on the SOL RPC canister still serving a transaction, or the signature
//! list of a nonce account, after the nonce advanced. Whether that holds for a signature older
//! than `history_horizon` is unmeasured. This module is the measuring instrument, and ONLY that:
//! it reads, reports and decides nothing for the lane.
//!
//! Given an explicit configuration (SOL RPC canister id, cluster and consensus settings, a list of
//! old signatures, a nonce account and a horizon in slots) it makes, through the crate's
//! [`SolRpc`] mirror:
//!
//! * `getTransaction` for each signature,
//! * `getSignatureStatuses` for each signature with `searchTransactionHistory: true`,
//! * `getSignaturesForAddress` for the nonce account,
//! * one `getSlot`, to age every result against the finalized tip,
//!
//! and reports, per call, whether the answer was **Consistent and present**, Consistent and
//! absent, Inconsistent, or an error, and whether a present answer is older than the horizon.
//!
//! It never runs by itself, has no default endpoint, canister id, cluster or signature, and the
//! canister helper that exposes it is behind the `history_probe` cargo feature (a throwaway
//! build, never the production wasm). Executing it against a public cluster needs separate
//! authorization (see `HISTORY_PROBE.md`).
//!
//! Reading the result: a `served_old_*` flag is true only if at least one result was Consistent,
//! present and at least `horizon_slots` older than the tip. `false` means "not shown by this run",
//! never "history is absent": an absent or inconsistent answer for a signature that really exists
//! is the finding (the providers behind that consensus did not serve it), and a missing finding
//! is not retention. The horizon the lane may use must stay strictly below what this shows.

use crate::sol_rpc::{RpcFailure, SolRpc};
use crate::tx::{pubkey_from_base58, signature_from_base58, signature_to_base58, Pubkey, Signature};
use crate::types::RpcSettings;
use candid::{CandidType, Deserialize, Principal};

/// Most signatures one run probes (two calls each).
pub const MAX_PROBE_SIGNATURES: usize = 8;
/// Default and largest `getSignaturesForAddress` page of a run.
pub const DEFAULT_LIST_LIMIT: u32 = 10;
pub const MAX_LIST_LIMIT: u32 = 100;
/// Hard upper bound on calls one run makes: `getSlot`, two per signature, one listing.
pub const MAX_PROBE_CALLS: u32 = 2 * MAX_PROBE_SIGNATURES as u32 + 2;

/// The explicit configuration of one run. Nothing is read from the stored bridge config: the
/// probe works on an instance that was never configured, with exactly the settings given here.
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ProbeArgs {
    /// The SOL RPC canister to probe. No default.
    pub sol_rpc: Principal,
    /// Cluster, consensus (`verify` for `getTransaction`, `getSignatureStatuses`, `getSlot`;
    /// `discover` for `getSignaturesForAddress`) and cycles per call. The cluster is required.
    pub rpc: RpcSettings,
    /// Base58 signatures believed to be older than the horizon (at least one, at most 8).
    pub signatures: Vec<String>,
    /// Base58 address whose signature list is read (a nonce account that has been written to).
    pub nonce_account: String,
    /// A result counts as old history only when `tip_slot - slot >= horizon_slots`. About 2.5
    /// slots per second: a 6 hour horizon is about 54,000 slots.
    pub horizon_slots: u64,
    /// `getSignaturesForAddress` limit (default 10, at most 100).
    pub list_limit: Option<u32>,
}

/// The answer of one call.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum CallVerdict {
    /// Consistent and present. `age_slots` and `older_than_horizon` are `None` when the tip slot
    /// could not be read.
    Present { slot: u64, age_slots: Option<u64>, older_than_horizon: Option<bool> },
    /// Consistent and not found (a consistent null).
    Absent,
    /// The providers disagreed.
    Inconsistent,
    Error(String),
}

impl CallVerdict {
    fn served_old(&self) -> bool {
        matches!(self, CallVerdict::Present { older_than_horizon: Some(true), .. })
    }
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SignatureProbe {
    pub signature: String,
    /// `getTransaction` at finalized.
    pub transaction: CallVerdict,
    /// `getSignatureStatuses` with `searchTransactionHistory: true`.
    pub status: CallVerdict,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ListingVerdict {
    /// Consistent and non-empty.
    Present {
        count: u32,
        oldest_slot: u64,
        newest_slot: u64,
        oldest_age_slots: Option<u64>,
        /// The oldest listed entry is at least `horizon_slots` old.
        older_than_horizon: Option<bool>,
        /// Which of the probed signatures appear in the list.
        contains: Vec<String>,
    },
    /// Consistent and empty.
    Absent,
    Inconsistent,
    Error(String),
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ProbeSummary {
    pub signatures_probed: u32,
    pub transaction_served_old: bool,
    pub status_served_old: bool,
    pub listing_served_old: bool,
    pub note: String,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ProbeReport {
    /// The finalized tip the ages are measured against (`None`: `getSlot` failed).
    pub tip_slot: Option<u64>,
    pub tip_error: Option<String>,
    pub horizon_slots: u64,
    pub results: Vec<SignatureProbe>,
    pub listing: ListingVerdict,
    pub summary: ProbeSummary,
    pub calls_made: u32,
}

/// A validated run.
#[derive(Clone, Debug)]
struct Plan {
    signatures: Vec<Signature>,
    nonce_account: Pubkey,
    horizon_slots: u64,
    list_limit: u32,
}

fn plan(args: &ProbeArgs) -> Result<Plan, String> {
    if args.signatures.is_empty() {
        return Err("at least one signature is required".into());
    }
    if args.signatures.len() > MAX_PROBE_SIGNATURES {
        return Err(format!("at most {MAX_PROBE_SIGNATURES} signatures per run, got {}", args.signatures.len()));
    }
    let mut signatures = Vec::with_capacity(args.signatures.len());
    for s in &args.signatures {
        let sig = signature_from_base58(s).map_err(|e| format!("signature {s:?}: {e}"))?;
        if signatures.contains(&sig) {
            return Err(format!("duplicate signature {s}"));
        }
        signatures.push(sig);
    }
    let nonce_account = pubkey_from_base58(&args.nonce_account).map_err(|e| format!("nonce_account: {e}"))?;
    if args.horizon_slots == 0 {
        return Err("horizon_slots must be > 0".into());
    }
    let list_limit = args.list_limit.unwrap_or(DEFAULT_LIST_LIMIT);
    if list_limit == 0 || list_limit > MAX_LIST_LIMIT {
        return Err(format!("list_limit must be in 1..={MAX_LIST_LIMIT}"));
    }
    if args.rpc.cluster.is_none() {
        return Err("rpc.cluster is required: the probe has no default cluster".into());
    }
    if args.sol_rpc == Principal::anonymous() {
        return Err("sol_rpc is required: the probe has no default canister id".into());
    }
    crate::state::validate_rpc_settings(&args.rpc)?;
    Ok(Plan { signatures, nonce_account, horizon_slots: args.horizon_slots, list_limit })
}

fn failure(e: RpcFailure) -> CallVerdict {
    match e {
        RpcFailure::Inconsistent => CallVerdict::Inconsistent,
        RpcFailure::Error(m) => CallVerdict::Error(m),
    }
}

fn aged(slot: u64, tip: Option<u64>, horizon: u64) -> (Option<u64>, Option<bool>) {
    match tip {
        Some(t) => {
            let age = t.saturating_sub(slot);
            (Some(age), Some(age >= horizon))
        }
        None => (None, None),
    }
}

fn present(slot: u64, tip: Option<u64>, horizon: u64) -> CallVerdict {
    let (age_slots, older_than_horizon) = aged(slot, tip, horizon);
    CallVerdict::Present { slot, age_slots, older_than_horizon }
}

/// Run one probe through `rpc`. Pure with respect to the bridge state: it neither reads nor
/// writes it. Returns an error only for an invalid configuration; every call outcome, however
/// bad, is part of the report.
pub async fn run<R: SolRpc>(rpc: &R, args: &ProbeArgs) -> Result<ProbeReport, String> {
    let plan = plan(args)?;
    let mut calls = 0u32;

    calls += 1;
    let (tip_slot, tip_error) = match rpc.get_slot().await {
        Ok(slot) => (Some(slot), None),
        Err(e) => (None, Some(format!("{e:?}"))),
    };

    let mut results = Vec::with_capacity(plan.signatures.len());
    for sig in &plan.signatures {
        calls += 1;
        let transaction = match rpc.get_transaction(sig).await {
            Ok(Some(tx)) => present(tx.slot, tip_slot, plan.horizon_slots),
            Ok(None) => CallVerdict::Absent,
            Err(e) => failure(e),
        };
        calls += 1;
        let status = match rpc.get_signature_statuses(std::slice::from_ref(sig), true).await {
            Ok(rows) => match rows.into_iter().next().flatten() {
                Some(st) => present(st.slot, tip_slot, plan.horizon_slots),
                None => CallVerdict::Absent,
            },
            Err(e) => failure(e),
        };
        results.push(SignatureProbe { signature: signature_to_base58(sig), transaction, status });
    }

    calls += 1;
    let listing = match rpc.get_signatures_for_address(&plan.nonce_account, None, None, plan.list_limit).await {
        Ok(entries) if entries.is_empty() => ListingVerdict::Absent,
        Ok(entries) => {
            let oldest_slot = entries.iter().map(|e| e.slot).min().unwrap_or(0);
            let newest_slot = entries.iter().map(|e| e.slot).max().unwrap_or(0);
            let (oldest_age_slots, older_than_horizon) = aged(oldest_slot, tip_slot, plan.horizon_slots);
            let contains = plan
                .signatures
                .iter()
                .filter(|s| entries.iter().any(|e| &e.signature == *s))
                .map(signature_to_base58)
                .collect();
            ListingVerdict::Present { count: entries.len() as u32, oldest_slot, newest_slot, oldest_age_slots, older_than_horizon, contains }
        }
        Err(RpcFailure::Inconsistent) => ListingVerdict::Inconsistent,
        Err(RpcFailure::Error(m)) => ListingVerdict::Error(m),
    };

    let summary = ProbeSummary {
        signatures_probed: results.len() as u32,
        transaction_served_old: results.iter().any(|r| r.transaction.served_old()),
        status_served_old: results.iter().any(|r| r.status.served_old()),
        listing_served_old: matches!(listing, ListingVerdict::Present { older_than_horizon: Some(true), .. }),
        note: "A false flag means 'not shown by this run', not 'absent'. An Absent or Inconsistent answer for a signature that exists is the finding. Keep history_horizon strictly below what this shows (blocker solana-history-availability-unmeasured).".into(),
    };
    Ok(ProbeReport { tip_slot, tip_error, horizon_slots: plan.horizon_slots, results, listing, summary, calls_made: calls })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{block_on, Fault, MockChain};
    use crate::tx::{assemble_wire_tx, compile_message, serialize_message, MessageVersion, MEMO_PROGRAM_V3_ID};
    use crate::types::SolRpcCluster;

    const NONCE_ACCOUNT: Pubkey = [0x55; 32];

    fn wire(tag: u8) -> (Vec<u8>, Signature) {
        let msg = compile_message(MessageVersion::Legacy, &[crate::tx::memo_instruction(&MEMO_PROGRAM_V3_ID, &[tag])], &[0x90 + tag; 32], [0; 32]).unwrap();
        let sig = [tag; 64];
        (assemble_wire_tx(&[sig], &serialize_message(&msg).unwrap()).unwrap(), sig)
    }

    fn args(signatures: Vec<Signature>) -> ProbeArgs {
        ProbeArgs {
            sol_rpc: Principal::from_slice(&[9; 10]),
            rpc: RpcSettings { cluster: Some(SolRpcCluster::Devnet), ..RpcSettings::default() },
            signatures: signatures.iter().map(signature_to_base58).collect(),
            nonce_account: crate::tx::pubkey_to_base58(&NONCE_ACCOUNT),
            horizon_slots: 500,
            list_limit: None,
        }
    }

    /// A chain with an old transaction (slot 1000) that mentions the nonce account, tip 2000.
    fn chain_with_old_history() -> (MockChain, Signature) {
        let chain = MockChain::new();
        let (w, sig) = wire(1);
        chain.land_noise(&w, &NONCE_ACCOUNT);
        (chain, sig)
    }

    #[test]
    fn consistent_and_present_for_an_old_signature_is_reported_as_served() {
        let (chain, sig) = chain_with_old_history();
        let report = block_on(run(&chain, &args(vec![sig]))).unwrap();
        assert_eq!(report.tip_slot, Some(2000));
        let r = &report.results[0];
        let expected = CallVerdict::Present { slot: 1000, age_slots: Some(1000), older_than_horizon: Some(true) };
        assert_eq!(r.transaction, expected);
        assert_eq!(r.status, expected);
        match &report.listing {
            ListingVerdict::Present { count, oldest_slot, newest_slot, oldest_age_slots, older_than_horizon, contains } => {
                assert_eq!((*count, *oldest_slot, *newest_slot, *oldest_age_slots, *older_than_horizon), (1, 1000, 1000, Some(1000), Some(true)));
                assert_eq!(contains, &vec![signature_to_base58(&sig)]);
            }
            other => panic!("{other:?}"),
        }
        assert!(report.summary.transaction_served_old && report.summary.status_served_old && report.summary.listing_served_old);
        assert_eq!(report.calls_made, 4, "getSlot, getTransaction, getSignatureStatuses, getSignaturesForAddress");
        assert_eq!(chain.call_count("get_signature_statuses"), 1);
    }

    #[test]
    fn statuses_are_requested_with_search_transaction_history() {
        // The mock records the flag it was called with through a wrapper transport.
        struct Spy<'a> {
            inner: &'a MockChain,
            search: std::cell::Cell<Option<bool>>,
        }
        impl SolRpc for Spy<'_> {
            async fn get_account(&self, a: &Pubkey) -> crate::sol_rpc::RpcResult<Option<crate::sol_rpc::AccountSnapshot>> {
                self.inner.get_account(a).await
            }
            async fn get_transaction(&self, s: &Signature) -> crate::sol_rpc::RpcResult<Option<crate::sol_rpc::LandedTx>> {
                self.inner.get_transaction(s).await
            }
            async fn get_signatures_for_address(&self, a: &Pubkey, b: Option<&Signature>, u: Option<&Signature>, l: u32) -> crate::sol_rpc::RpcResult<Vec<crate::sol_rpc::SignatureEntry>> {
                assert_eq!((b, u), (None, None));
                self.inner.get_signatures_for_address(a, b, u, l).await
            }
            async fn get_signature_statuses(&self, s: &[Signature], search: bool) -> crate::sol_rpc::RpcResult<Vec<Option<crate::sol_rpc::SignatureStatus>>> {
                self.search.set(Some(search));
                self.inner.get_signature_statuses(s, search).await
            }
            async fn send_transaction(&self, w: &[u8]) -> crate::sol_rpc::RpcResult<Signature> {
                panic!("the probe never sends: {} bytes", w.len())
            }
            async fn get_slot(&self) -> crate::sol_rpc::RpcResult<u64> {
                self.inner.get_slot().await
            }
            async fn get_block_blockhash(&self, s: u64) -> crate::sol_rpc::RpcResult<Option<[u8; 32]>> {
                self.inner.get_block_blockhash(s).await
            }
        }
        let (chain, sig) = chain_with_old_history();
        let spy = Spy { inner: &chain, search: std::cell::Cell::new(None) };
        block_on(run(&spy, &args(vec![sig]))).unwrap();
        assert_eq!(spy.search.get(), Some(true), "searchTransactionHistory is true");
    }

    #[test]
    fn a_consistent_absent_answer_is_reported_as_absent_not_as_served() {
        let chain = MockChain::new();
        let report = block_on(run(&chain, &args(vec![[7; 64]]))).unwrap();
        assert_eq!(report.results[0].transaction, CallVerdict::Absent);
        assert_eq!(report.results[0].status, CallVerdict::Absent);
        assert_eq!(report.listing, ListingVerdict::Absent);
        assert!(!report.summary.transaction_served_old && !report.summary.status_served_old && !report.summary.listing_served_old);
    }

    #[test]
    fn inconsistent_answers_are_reported_per_call() {
        let (chain, sig) = chain_with_old_history();
        chain.fail_next("get_transaction", Fault::Inconsistent);
        chain.fail_next("get_signature_statuses", Fault::Inconsistent);
        chain.fail_next("get_signatures_for_address", Fault::Inconsistent);
        let report = block_on(run(&chain, &args(vec![sig]))).unwrap();
        assert_eq!(report.results[0].transaction, CallVerdict::Inconsistent);
        assert_eq!(report.results[0].status, CallVerdict::Inconsistent);
        assert_eq!(report.listing, ListingVerdict::Inconsistent);
        assert!(!report.summary.transaction_served_old && !report.summary.listing_served_old);
        // Each call fails on its own: a later clean run sees the history again.
        let again = block_on(run(&chain, &args(vec![sig]))).unwrap();
        assert!(again.summary.transaction_served_old);
    }

    #[test]
    fn errors_are_reported_and_a_failed_tip_leaves_ages_unknown() {
        let (chain, sig) = chain_with_old_history();
        chain.fail_next("get_slot", Fault::Error("slot outcall failed".into()));
        chain.fail_next("get_transaction", Fault::Error("provider 500".into()));
        let report = block_on(run(&chain, &args(vec![sig]))).unwrap();
        assert_eq!(report.tip_slot, None);
        assert!(report.tip_error.as_deref().unwrap().contains("slot outcall failed"));
        assert_eq!(report.results[0].transaction, CallVerdict::Error("provider 500".into()));
        // Present, but its age cannot be judged: not counted as old history.
        assert_eq!(report.results[0].status, CallVerdict::Present { slot: 1000, age_slots: None, older_than_horizon: None });
        assert!(!report.summary.status_served_old);
        match &report.listing {
            ListingVerdict::Present { older_than_horizon, oldest_age_slots, .. } => assert_eq!((*older_than_horizon, *oldest_age_slots), (None, None)),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_present_result_younger_than_the_horizon_is_present_but_not_old() {
        let chain = MockChain::new();
        chain.tip_slot.set(1_200);
        let (w, sig) = wire(2);
        chain.land_noise(&w, &NONCE_ACCOUNT); // slot 1000, age 200 < 500
        let report = block_on(run(&chain, &args(vec![sig]))).unwrap();
        assert_eq!(report.results[0].transaction, CallVerdict::Present { slot: 1000, age_slots: Some(200), older_than_horizon: Some(false) });
        assert!(!report.summary.transaction_served_old, "present is not enough: it must be older than the horizon");
        // Exactly at the horizon counts.
        chain.tip_slot.set(1_500);
        let report = block_on(run(&chain, &args(vec![sig]))).unwrap();
        assert!(report.summary.transaction_served_old);
    }

    #[test]
    fn several_signatures_report_independently() {
        let chain = MockChain::new();
        let (w1, s1) = wire(1);
        let (w2, s2) = wire(2);
        chain.land_noise(&w1, &NONCE_ACCOUNT);
        chain.land_noise(&w2, &NONCE_ACCOUNT);
        let missing = [9u8; 64];
        let report = block_on(run(&chain, &args(vec![s1, missing, s2]))).unwrap();
        assert_eq!(report.results.len(), 3);
        assert!(matches!(report.results[0].transaction, CallVerdict::Present { slot: 1000, .. }));
        assert_eq!(report.results[1].transaction, CallVerdict::Absent);
        assert!(matches!(report.results[2].transaction, CallVerdict::Present { slot: 1001, .. }));
        assert_eq!(report.summary.signatures_probed, 3);
        assert_eq!(report.calls_made, 1 + 3 * 2 + 1);
        assert!(report.calls_made <= MAX_PROBE_CALLS);
    }

    #[test]
    fn the_configuration_is_validated_and_has_no_defaults() {
        let chain = MockChain::new();
        let ok = args(vec![[1; 64]]);
        assert!(block_on(run(&chain, &ok)).is_ok());
        let bad = |f: &dyn Fn(&mut ProbeArgs)| {
            let mut a = ok.clone();
            f(&mut a);
            block_on(run(&chain, &a)).unwrap_err()
        };
        assert!(bad(&|a| a.signatures.clear()).contains("at least one"));
        assert!(bad(&|a| a.signatures = (0..9).map(|i| signature_to_base58(&[i; 64])).collect()).contains("at most 8"));
        assert!(bad(&|a| a.signatures = vec!["not a signature".into()]).contains("signature"));
        assert!(bad(&|a| a.signatures = vec![signature_to_base58(&[1; 64]), signature_to_base58(&[1; 64])]).contains("duplicate"));
        assert!(bad(&|a| a.nonce_account = "short".into()).contains("nonce_account"));
        assert!(bad(&|a| a.horizon_slots = 0).contains("horizon_slots"));
        assert!(bad(&|a| a.list_limit = Some(0)).contains("list_limit"));
        assert!(bad(&|a| a.list_limit = Some(101)).contains("list_limit"));
        assert!(bad(&|a| a.rpc.cluster = None).contains("no default cluster"));
        assert!(bad(&|a| a.sol_rpc = Principal::anonymous()).contains("no default canister id"));
        assert!(bad(&|a| a.rpc.cycles_per_call = 0).contains("cycles_per_call"));
        assert_eq!(chain.call_count("get_slot"), 1, "an invalid configuration makes no calls (only the valid run did)");
    }

    #[test]
    fn the_probe_neither_reads_nor_writes_the_bridge_state() {
        crate::state::reset_for_tests();
        let before = crate::state::read_core(|c| c.clone());
        let audit_before = crate::state::audit_len();
        let (chain, sig) = chain_with_old_history();
        block_on(run(&chain, &args(vec![sig]))).unwrap();
        assert_eq!(crate::state::read_core(|c| c.clone()), before);
        assert_eq!(crate::state::audit_len(), audit_before);
        assert_eq!(crate::state::active_op_count(), 0);
    }

    #[test]
    fn the_wasm_transport_refuses_to_run_off_chain() {
        let rpc = crate::sol_rpc::CandidRpc::from_config(Principal::from_slice(&[9; 10]), args(vec![[1; 64]]).rpc).unwrap();
        let report = block_on(run(&rpc, &args(vec![[1; 64]]))).unwrap();
        assert!(matches!(report.results[0].transaction, CallVerdict::Error(ref m) if m.contains("unavailable off-chain")));
        assert!(matches!(report.listing, ListingVerdict::Error(_)));
        assert_eq!(report.tip_slot, None);
    }

    /// The call in the runbook, with its placeholders replaced, type-checks against the probe
    /// interface and decodes into `ProbeArgs` that pass validation.
    #[test]
    fn the_runbook_example_matches_the_probe_interface() {
        use candid_parser::utils::CandidSource;
        let md = include_str!("../HISTORY_PROBE.md");
        let line = md.lines().find(|l| l.contains("probe_history_availability '(")).expect("the runbook has the call");
        let args_text = line.split("probe_history_availability '").nth(1).unwrap().trim_end_matches(['`', '\'']);
        let args_text = args_text
            .replace("<SOL_RPC_CANISTER_ID>", &Principal::from_slice(&[9; 10]).to_text())
            .replace("<OLD_SIGNATURE_1>", &signature_to_base58(&[1; 64]))
            .replace("<NONCE_ACCOUNT>", &crate::tx::pubkey_to_base58(&NONCE_ACCOUNT))
            .replace("<SLOTS>", "54000");
        let (env, actor) = CandidSource::Text(include_str!("../rumi_icusd_sol_bridge.history_probe.did")).load().expect("the probe interface parses");
        let actor = actor.expect("service");
        let func = env.get_method(&actor, "probe_history_availability").expect("the probe method is in the interface");
        let parsed = candid_parser::parse_idl_args(&args_text).unwrap_or_else(|e| panic!("{e}\n{args_text}"));
        let bytes = parsed.to_bytes_with_types(&env, &func.args).unwrap_or_else(|e| panic!("the runbook call does not fit the interface: {e}"));
        let (decoded,): (ProbeArgs,) = candid::decode_args(&bytes).unwrap();
        assert_eq!(decoded.horizon_slots, 54_000);
        assert_eq!(decoded.rpc.cluster, Some(SolRpcCluster::Devnet));
        assert!(plan(&decoded).is_ok());
    }
}

