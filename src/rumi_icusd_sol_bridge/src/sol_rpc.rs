//! Typed SOL RPC mirror (`chain-solana.md` section 5): hand-mirrored Candid for ic-cdk 0.12, the
//! domain-level [`SolRpc`] transport trait the nonce lane runs against, and the wasm transport.
//!
//! The typed `sol_rpc_client` needs ic-cdk 0.20 (the pin conflict recorded in
//! `2026-06-01-solana-integration-design.md` section 4), so the wire types are mirrored here.
//! The mirror covers exactly the calls reconcile needs:
//!
//! | Call | Use |
//! |------|-----|
//! | `getAccountInfo` (base64, finalized) | the nonce account: owner, authority, durable nonce value |
//! | `getTransaction` (base64, finalized) | exact message bytes and `meta.status` of a landed transaction |
//! | `getSignaturesForAddress` | discovery on a nonce account; `before`/`until` are explicit parameters |
//! | `getSignatureStatuses` | cheap landed check (mirrored; the lane verifies through `getTransaction`) |
//! | `sendTransaction` | broadcast; every outcome is "maybe sent" |
//! | `getSlot`, `getBlock` | the pool bootstrap blockhash (the README route, no `getLatestBlockhash`) |
//!
//! Every call is made at commitment `finalized`. Consensus comes from [`RpcSettings`]: strict
//! `verify` for everything that can change a decision, lenient `discover` for the candidate list
//! (note section 4, P4). The SOL RPC canister id is `BridgeConfig::sol_rpc`, an install argument.
//!
//! # Provenance and what is NOT verified
//!
//! The types below follow `tests/fixtures/sol_rpc_canister.did`. That file is the exact git blob
//! `cd456a3b397e2296fd00ca764faa6ababfb7cd49` (39,673 bytes) of `canister/sol_rpc_canister.did` in
//! the official `dfinity/sol-rpc-canister` repository at commit
//! `5ac94de29be388eeff758bbed953705e341a90da`:
//! <https://github.com/dfinity/sol-rpc-canister/blob/5ac94de29be388eeff758bbed953705e341a90da/canister/sol_rpc_canister.did>
//! (verified read-only by the coordinator and Miles; `tests::the_fixture_is_the_pinned_upstream_file`
//! pins its length and SHA-256 here). It was first found as a copy in another project's tree.
//!
//! That proves source-to-fixture equality ONLY. It does not prove the interface of the deployed
//! SOL RPC canister, nor how its providers behave (consensus, history depth, rounding). The
//! `tests::mirror_*` tests check this mirror against the fixture offline (arguments encode into
//! the fixture's argument types, and replies built in the fixture's reply types decode into this
//! mirror). Activation must repeat the comparison against the deployed canister's own interface
//! (blocker `solana-rpc-consensus-unmeasured`).
//!
//! Decode safety: reply types keep only the fields the lane reads. Everything else (provider
//! errors, transaction errors, inconsistent per-provider vectors) is `candid::Reserved`, so the
//! whole-type-table subtype check passes without enumerating large variants.

use crate::tx::{decode_wire_transaction, signature_to_base58, split_wire_transaction, Pubkey, Signature, TxError};
use crate::types::{ConsensusSetting, RpcSettings, SolRpcCluster};
use base64::Engine as _;

/// Why a SOL RPC call produced no usable value. "Null or `Inconsistent`: keep waiting" is the
/// lane's reading of both variants; neither is ever evidence of absence or of non-delivery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RpcFailure {
    /// The providers disagreed (no consensus).
    Inconsistent,
    /// Anything else: transport, provider, validation, undecodable reply, not configured.
    Error(String),
}

pub type RpcResult<T> = Result<T, RpcFailure>;

// ---------------------------------------------------------------------------
// Domain types (what the lane sees)
// ---------------------------------------------------------------------------

/// An account as read at `finalized`, `data` base64-decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountSnapshot {
    pub owner: Pubkey,
    pub data: Vec<u8>,
}

/// A landed transaction at `finalized`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LandedTx {
    pub slot: u64,
    /// The wire transaction (`[signatures][message]`), base64-decoded.
    pub wire: Vec<u8>,
    /// `meta.status` was `Err`: the transaction landed but failed in execution.
    pub failed: bool,
    /// The parts of `meta` the return observer decides on (49c). The lane ignores them.
    pub meta: TxMeta,
}

/// One entry of `preTokenBalances` / `postTokenBalances`. `owner` may be omitted by the RPC
/// (source S6), and then the entry cannot prove who owns the account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenBalanceEntry {
    /// Index into the message's account keys (a lookup-free message: the static keys).
    pub account_index: u8,
    pub mint: Pubkey,
    pub owner: Option<Pubkey>,
    /// The raw base-unit amount.
    pub amount: u64,
    pub decimals: u8,
}

/// The transaction metadata the observer reads. `None` fields were not recorded by the RPC and
/// make a return unverifiable.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TxMeta {
    /// How many inner-instruction groups the transaction recorded; `None` if not recorded.
    pub inner_instruction_groups: Option<u32>,
    pub pre_token_balances: Option<Vec<TokenBalanceEntry>>,
    pub post_token_balances: Option<Vec<TokenBalanceEntry>>,
}

/// One row of `getSignaturesForAddress` (newest first).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureEntry {
    pub signature: Signature,
    pub slot: u64,
    pub failed: bool,
    /// The memo text the RPC derived for this transaction, if any. Discovery only.
    pub memo: Option<String>,
}

/// One row of `getSignatureStatuses`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureStatus {
    pub slot: u64,
    pub failed: bool,
}

/// The transport the nonce lane runs against. The wasm implementation is [`CandidRpc`]; unit
/// tests use the in-crate mock (`testkit::MockChain`).
///
/// `Ok(None)` is a consistent null (account or transaction not found at `finalized`). It is not
/// proof of absence of anything: the lane treats it as "not yet".
#[allow(async_fn_in_trait)]
pub trait SolRpc {
    /// `getAccountInfo(address, finalized, base64)`.
    async fn get_account(&self, address: &Pubkey) -> RpcResult<Option<AccountSnapshot>>;
    /// `getTransaction(signature, finalized, base64, maxSupportedTransactionVersion 0)`.
    async fn get_transaction(&self, signature: &Signature) -> RpcResult<Option<LandedTx>>;
    /// `getSignaturesForAddress(address, finalized, limit, before, until)`, newest first.
    async fn get_signatures_for_address(
        &self,
        address: &Pubkey,
        before: Option<&Signature>,
        until: Option<&Signature>,
        limit: u32,
    ) -> RpcResult<Vec<SignatureEntry>>;
    /// `getSignatureStatuses(signatures, searchTransactionHistory)`.
    async fn get_signature_statuses(
        &self,
        signatures: &[Signature],
        search_transaction_history: bool,
    ) -> RpcResult<Vec<Option<SignatureStatus>>>;
    /// `sendTransaction(wire, base64)`. Every failure is "maybe sent" to the caller.
    async fn send_transaction(&self, wire: &[u8]) -> RpcResult<Signature>;
    /// `getSlot(finalized)`, rounded down by the canister.
    async fn get_slot(&self) -> RpcResult<u64>;
    /// `getBlock(slot, finalized, transactionDetails none)`: the block's blockhash, `None` when
    /// the slot has no confirmed block.
    async fn get_block_blockhash(&self, slot: u64) -> RpcResult<Option<[u8; 32]>>;
}

// ---------------------------------------------------------------------------
// Wire mirror
// ---------------------------------------------------------------------------

/// Candid types as they appear in the SOL RPC canister interface. Field and variant names are
/// the interface's (camelCase, lower-case variants), hence the lint allowances.
#[allow(non_snake_case, non_camel_case_types)]
pub mod wire {
    use candid::{CandidType, Deserialize, Reserved};

    // ---- request side (encoded by us; subsets of the interface's types are valid senders) ----

    #[derive(CandidType, Clone, Debug)]
    pub enum SolanaCluster {
        Mainnet,
        Devnet,
        Testnet,
    }

    /// `Custom(vec RpcSource)` exists in the interface and is never sent.
    #[derive(CandidType, Clone, Debug)]
    pub enum RpcSources {
        Default(SolanaCluster),
    }

    #[derive(CandidType, Clone, Debug)]
    pub enum ConsensusStrategy {
        Equality,
        Threshold { total: Option<u8>, min: u8 },
    }

    #[derive(CandidType, Clone, Debug)]
    pub struct RpcConfig {
        pub responseSizeEstimate: Option<u64>,
        pub responseConsensus: Option<ConsensusStrategy>,
    }

    #[derive(CandidType, Clone, Debug)]
    pub struct GetSlotRpcConfig {
        pub responseSizeEstimate: Option<u64>,
        pub responseConsensus: Option<ConsensusStrategy>,
        pub roundingError: Option<u64>,
    }

    #[derive(CandidType, Clone, Debug)]
    pub enum CommitmentLevel {
        processed,
        confirmed,
        finalized,
    }

    #[derive(CandidType, Clone, Debug)]
    pub enum GetAccountInfoEncoding {
        base58,
        base64,
        jsonParsed,
    }

    #[derive(CandidType, Clone, Debug)]
    pub struct DataSlice {
        pub length: u32,
        pub offset: u32,
    }

    #[derive(CandidType, Clone, Debug)]
    pub struct GetAccountInfoParams {
        pub pubkey: String,
        pub commitment: Option<CommitmentLevel>,
        pub encoding: Option<GetAccountInfoEncoding>,
        pub dataSlice: Option<DataSlice>,
        pub minContextSlot: Option<u64>,
    }

    #[derive(CandidType, Clone, Debug)]
    pub enum BinaryEncoding {
        base58,
        base64,
    }

    #[derive(CandidType, Clone, Debug)]
    pub struct GetTransactionParams {
        pub signature: String,
        pub commitment: Option<CommitmentLevel>,
        pub maxSupportedTransactionVersion: Option<u8>,
        pub encoding: Option<BinaryEncoding>,
    }

    #[derive(CandidType, Clone, Debug)]
    pub struct GetSignaturesForAddressParams {
        pub pubkey: String,
        pub commitment: Option<CommitmentLevel>,
        pub minContextSlot: Option<u64>,
        pub limit: Option<u32>,
        pub before: Option<String>,
        pub until: Option<String>,
    }

    #[derive(CandidType, Clone, Debug)]
    pub struct GetSignatureStatusesParams {
        pub signatures: Vec<String>,
        pub searchTransactionHistory: Option<bool>,
    }

    #[derive(CandidType, Clone, Debug)]
    pub struct GetSlotParams {
        pub commitment: Option<CommitmentLevel>,
        pub minContextSlot: Option<u64>,
    }

    #[derive(CandidType, Clone, Debug)]
    pub enum TransactionDetails {
        accounts,
        none,
        signatures,
    }

    #[derive(CandidType, Clone, Debug)]
    pub enum BlockCommitment {
        confirmed,
        finalized,
    }

    #[derive(CandidType, Clone, Debug)]
    pub struct GetBlockParams {
        pub slot: u64,
        pub transactionDetails: Option<TransactionDetails>,
        pub commitment: Option<BlockCommitment>,
        pub maxSupportedTransactionVersion: Option<u8>,
        pub rewards: Option<bool>,
    }

    #[derive(CandidType, Clone, Debug)]
    pub struct SendTransactionParams {
        pub transaction: String,
        pub encoding: Option<BinaryEncoding>,
        pub skipPreflight: Option<bool>,
        pub preflightCommitment: Option<CommitmentLevel>,
        pub maxRetries: Option<u32>,
        pub minContextSlot: Option<u64>,
    }

    // ---- reply side (decoded by us; supersets of what the canister may send) ----

    #[derive(CandidType, Deserialize, Clone, Debug)]
    pub struct JsonRpcError {
        pub code: i64,
        pub message: String,
    }

    /// `ProviderError` and `HttpOutcallError` payloads are `Reserved`: only their arm name is read.
    #[derive(CandidType, Deserialize, Clone, Debug)]
    pub enum RpcError {
        JsonRpcError(JsonRpcError),
        ProviderError(Reserved),
        ValidationError(String),
        HttpOutcallError(Reserved),
    }

    /// `Multi*Result`: the per-provider vector of `Inconsistent` is never inspected.
    #[derive(CandidType, Deserialize, Clone, Debug)]
    pub enum Multi<T> {
        Consistent(Result<T, RpcError>),
        Inconsistent(Reserved),
    }

    /// `AccountData`: the `binary` arm carries `(text, AccountEncoding)`; the encoding (whose
    /// `"base64+zstd"` label is not a Rust identifier) is `Reserved`, because the request asks
    /// for base64 and a reply in any other encoding fails the length and parse checks.
    #[derive(CandidType, Deserialize, Clone, Debug)]
    pub enum AccountData {
        legacyBinary(String),
        json(Reserved),
        binary((String, Reserved)),
    }

    #[derive(CandidType, Deserialize, Clone, Debug)]
    pub struct AccountInfo {
        pub data: AccountData,
        pub owner: String,
    }

    #[derive(CandidType, Deserialize, Clone, Debug)]
    pub enum EncodedTransaction {
        binary((String, Reserved)),
        legacyBinary(String),
    }

    /// `status : variant { Ok; Err : TransactionError }`.
    #[derive(CandidType, Deserialize, Clone, Debug)]
    pub enum ExecutionStatus {
        Ok,
        Err(Reserved),
    }

    /// `TokenAmount`: only the raw amount and the decimals are read.
    #[derive(CandidType, Deserialize, Clone, Debug)]
    pub struct TokenAmount {
        pub decimals: u8,
        pub amount: String,
    }

    #[derive(CandidType, Deserialize, Clone, Debug)]
    pub struct TransactionTokenBalance {
        pub owner: Option<String>,
        pub mint: String,
        pub accountIndex: u8,
        pub uiTokenAmount: TokenAmount,
    }

    #[derive(CandidType, Deserialize, Clone, Debug)]
    pub struct TransactionStatusMeta {
        pub status: ExecutionStatus,
        /// `opt vec InnerInstructions`: only the number of groups is read.
        pub innerInstructions: Option<Vec<Reserved>>,
        pub preTokenBalances: Option<Vec<TransactionTokenBalance>>,
        pub postTokenBalances: Option<Vec<TransactionTokenBalance>>,
    }

    #[derive(CandidType, Deserialize, Clone, Debug)]
    pub struct EncodedTransactionWithStatusMeta {
        pub meta: Option<TransactionStatusMeta>,
        pub transaction: EncodedTransaction,
    }

    #[derive(CandidType, Deserialize, Clone, Debug)]
    pub struct EncodedConfirmedTransactionWithStatusMeta {
        pub slot: u64,
        pub transaction: EncodedTransactionWithStatusMeta,
    }

    #[derive(CandidType, Deserialize, Clone, Debug)]
    pub struct ConfirmedTransactionStatusWithSignature {
        pub signature: String,
        pub slot: u64,
        pub err: Option<Reserved>,
        pub memo: Option<String>,
    }

    #[derive(CandidType, Deserialize, Clone, Debug)]
    pub struct TransactionStatus {
        pub slot: u64,
        pub err: Option<Reserved>,
    }

    #[derive(CandidType, Deserialize, Clone, Debug)]
    pub struct ConfirmedBlock {
        pub blockhash: String,
    }
}

// ---------------------------------------------------------------------------
// Settings and request construction (pure)
// ---------------------------------------------------------------------------

fn wire_cluster(c: SolRpcCluster) -> wire::SolanaCluster {
    match c {
        SolRpcCluster::Mainnet => wire::SolanaCluster::Mainnet,
        SolRpcCluster::Devnet => wire::SolanaCluster::Devnet,
        SolRpcCluster::Testnet => wire::SolanaCluster::Testnet,
    }
}

fn wire_consensus(c: ConsensusSetting) -> wire::ConsensusStrategy {
    match c {
        ConsensusSetting::Equality => wire::ConsensusStrategy::Equality,
        ConsensusSetting::Threshold { total, min } => wire::ConsensusStrategy::Threshold { total, min },
    }
}

/// `RpcSources::Default(cluster)`, or a clear error when the cluster is not configured. Nothing
/// is sent to a default cluster: an unconfigured instance refuses every call.
pub fn rpc_sources(settings: &RpcSettings) -> RpcResult<wire::RpcSources> {
    settings
        .cluster
        .map(|c| wire::RpcSources::Default(wire_cluster(c)))
        .ok_or_else(|| RpcFailure::Error("SOL RPC cluster is not configured".into()))
}

pub fn rpc_config(settings: &RpcSettings, consensus: ConsensusSetting) -> wire::RpcConfig {
    wire::RpcConfig { responseSizeEstimate: settings.response_size_estimate, responseConsensus: Some(wire_consensus(consensus)) }
}

/// `getSlot` takes its own config, with the canister's slot rounding left at its default (20),
/// which is what makes consensus on a per-slot value possible at all.
pub fn slot_config(settings: &RpcSettings) -> wire::GetSlotRpcConfig {
    wire::GetSlotRpcConfig {
        responseSizeEstimate: settings.response_size_estimate,
        responseConsensus: Some(wire_consensus(settings.verify)),
        roundingError: None,
    }
}

pub fn account_info_params(address: &Pubkey) -> wire::GetAccountInfoParams {
    wire::GetAccountInfoParams {
        pubkey: crate::tx::pubkey_to_base58(address),
        commitment: Some(wire::CommitmentLevel::finalized),
        encoding: Some(wire::GetAccountInfoEncoding::base64),
        dataSlice: None,
        minContextSlot: None,
    }
}

pub fn transaction_params(signature: &Signature) -> wire::GetTransactionParams {
    wire::GetTransactionParams {
        signature: crate::tx::signature_to_base58(signature),
        commitment: Some(wire::CommitmentLevel::finalized),
        // Version 0 so a versioned transaction is returned instead of erroring; the lane then
        // rejects anything that is not its own legacy bytes.
        maxSupportedTransactionVersion: Some(0),
        encoding: Some(wire::BinaryEncoding::base64),
    }
}

pub fn signatures_for_address_params(
    address: &Pubkey,
    before: Option<&Signature>,
    until: Option<&Signature>,
    limit: u32,
) -> wire::GetSignaturesForAddressParams {
    wire::GetSignaturesForAddressParams {
        pubkey: crate::tx::pubkey_to_base58(address),
        commitment: Some(wire::CommitmentLevel::finalized),
        minContextSlot: None,
        limit: Some(limit),
        before: before.map(crate::tx::signature_to_base58),
        until: until.map(crate::tx::signature_to_base58),
    }
}

pub fn signature_statuses_params(signatures: &[Signature], search_transaction_history: bool) -> wire::GetSignatureStatusesParams {
    wire::GetSignatureStatusesParams {
        signatures: signatures.iter().map(crate::tx::signature_to_base58).collect(),
        searchTransactionHistory: Some(search_transaction_history),
    }
}

pub fn send_transaction_params(wire_tx: &[u8]) -> wire::SendTransactionParams {
    wire::SendTransactionParams {
        transaction: base64::engine::general_purpose::STANDARD.encode(wire_tx),
        encoding: Some(wire::BinaryEncoding::base64),
        // Keep the provider's preflight simulation, as the backend rail does.
        skipPreflight: Some(false),
        preflightCommitment: Some(wire::CommitmentLevel::finalized),
        maxRetries: None,
        minContextSlot: None,
    }
}

pub fn slot_params() -> wire::GetSlotParams {
    wire::GetSlotParams { commitment: Some(wire::CommitmentLevel::finalized), minContextSlot: None }
}

pub fn block_params(slot: u64) -> wire::GetBlockParams {
    wire::GetBlockParams {
        slot,
        // `none` is the interface default; stated so the request does not depend on it.
        transactionDetails: Some(wire::TransactionDetails::none),
        commitment: Some(wire::BlockCommitment::finalized),
        maxSupportedTransactionVersion: Some(0),
        rewards: Some(false),
    }
}

// ---------------------------------------------------------------------------
// Reply conversion (pure)
// ---------------------------------------------------------------------------

fn describe_rpc_error(e: &wire::RpcError) -> String {
    match e {
        wire::RpcError::JsonRpcError(j) => format!("json-rpc error {}: {}", j.code, j.message),
        wire::RpcError::ProviderError(_) => "provider error".to_string(),
        wire::RpcError::ValidationError(m) => format!("validation error: {m}"),
        wire::RpcError::HttpOutcallError(_) => "http outcall error".to_string(),
    }
}

/// `Consistent(Ok(v))` is a value; `Consistent(Err(e))` an error; `Inconsistent` is its own
/// failure so the lane can tell "providers disagree" from "the call failed".
pub fn unwrap_multi<T>(m: wire::Multi<T>) -> RpcResult<T> {
    match m {
        wire::Multi::Consistent(Ok(v)) => Ok(v),
        wire::Multi::Consistent(Err(e)) => Err(RpcFailure::Error(describe_rpc_error(&e))),
        wire::Multi::Inconsistent(_) => Err(RpcFailure::Inconsistent),
    }
}

fn decode_base64(text: &str, what: &str) -> RpcResult<Vec<u8>> {
    base64::engine::general_purpose::STANDARD.decode(text).map_err(|e| RpcFailure::Error(format!("{what}: bad base64: {e}")))
}

pub fn account_from_wire(info: Option<wire::AccountInfo>) -> RpcResult<Option<AccountSnapshot>> {
    let Some(info) = info else { return Ok(None) };
    let owner = crate::tx::pubkey_from_base58(&info.owner).map_err(|e| RpcFailure::Error(format!("account owner: {e}")))?;
    let data = match info.data {
        wire::AccountData::binary((text, _encoding)) => decode_base64(&text, "account data")?,
        wire::AccountData::legacyBinary(_) | wire::AccountData::json(_) => {
            return Err(RpcFailure::Error("account data was not returned as base64 binary".into()));
        }
    };
    Ok(Some(AccountSnapshot { owner, data }))
}

/// A lookup must return the transaction it was asked for. A transaction is identified by its FIRST
/// signature, so the returned wire's own first signature must equal the requested one, and it must
/// carry exactly the signature count its header names. Anything else is a wrong answer from the RPC
/// (the same valid bytes under another key would otherwise become a second identity), never a
/// transaction to record anything from. A message that does not decode strictly is still bound by the
/// signature list the wire itself carries.
pub fn check_bound(requested: &Signature, tx: &LandedTx) -> Result<(), String> {
    let first = match decode_wire_transaction(&tx.wire) {
        Ok((signatures, _, _)) => signatures.first().copied(),
        Err(TxError::SignatureCountMismatch { header, wire }) => {
            return Err(format!("the returned transaction carries {wire} signatures but its header requires {header}"));
        }
        Err(_) => match split_wire_transaction(&tx.wire) {
            Ok((signatures, _)) => signatures.first().copied(),
            Err(e) => return Err(format!("the returned transaction bytes do not parse: {e}")),
        },
    };
    match first {
        None => Err("the returned transaction carries no signature".into()),
        Some(s) if s == *requested => Ok(()),
        Some(s) => Err(format!(
            "the signature of the returned transaction ({}) is not the requested one ({})",
            signature_to_base58(&s),
            signature_to_base58(requested)
        )),
    }
}

pub fn transaction_from_wire(tx: Option<wire::EncodedConfirmedTransactionWithStatusMeta>) -> RpcResult<Option<LandedTx>> {
    let Some(tx) = tx else { return Ok(None) };
    let text = match tx.transaction.transaction {
        wire::EncodedTransaction::binary((text, _encoding)) => text,
        wire::EncodedTransaction::legacyBinary(_) => {
            return Err(RpcFailure::Error("transaction was not returned as base64 binary".into()));
        }
    };
    let wire_tx = decode_base64(&text, "transaction")?;
    // A landed transaction without metadata cannot be classified as success or failure.
    let meta = tx.transaction.meta.ok_or_else(|| RpcFailure::Error("landed transaction has no status meta".into()))?;
    let failed = matches!(meta.status, wire::ExecutionStatus::Err(_));
    let balances = |rows: Option<Vec<wire::TransactionTokenBalance>>, what: &str| -> RpcResult<Option<Vec<TokenBalanceEntry>>> {
        rows.map(|rows| {
            rows.into_iter()
                .map(|r| {
                    let mint = crate::tx::pubkey_from_base58(&r.mint).map_err(|e| RpcFailure::Error(format!("{what} mint: {e}")))?;
                    let owner = r
                        .owner
                        .map(|o| crate::tx::pubkey_from_base58(&o).map_err(|e| RpcFailure::Error(format!("{what} owner: {e}"))))
                        .transpose()?;
                    let amount = r
                        .uiTokenAmount
                        .amount
                        .parse::<u64>()
                        .map_err(|e| RpcFailure::Error(format!("{what} amount {:?}: {e}", r.uiTokenAmount.amount)))?;
                    Ok(TokenBalanceEntry { account_index: r.accountIndex, mint, owner, amount, decimals: r.uiTokenAmount.decimals })
                })
                .collect::<RpcResult<Vec<_>>>()
        })
        .transpose()
    };
    let meta_view = TxMeta {
        inner_instruction_groups: meta.innerInstructions.map(|g| g.len() as u32),
        pre_token_balances: balances(meta.preTokenBalances, "preTokenBalances")?,
        post_token_balances: balances(meta.postTokenBalances, "postTokenBalances")?,
    };
    Ok(Some(LandedTx { slot: tx.slot, wire: wire_tx, failed, meta: meta_view }))
}

pub fn signatures_from_wire(rows: Vec<wire::ConfirmedTransactionStatusWithSignature>) -> RpcResult<Vec<SignatureEntry>> {
    rows.into_iter()
        .map(|r| {
            let signature = crate::tx::signature_from_base58(&r.signature).map_err(|e| RpcFailure::Error(format!("listed signature: {e}")))?;
            Ok(SignatureEntry { signature, slot: r.slot, failed: r.err.is_some(), memo: r.memo })
        })
        .collect()
}

pub fn statuses_from_wire(rows: Vec<Option<wire::TransactionStatus>>) -> Vec<Option<SignatureStatus>> {
    rows.into_iter().map(|r| r.map(|s| SignatureStatus { slot: s.slot, failed: s.err.is_some() })).collect()
}

pub fn blockhash_from_wire(block: Option<wire::ConfirmedBlock>) -> RpcResult<Option<[u8; 32]>> {
    let Some(block) = block else { return Ok(None) };
    crate::tx::pubkey_from_base58(&block.blockhash).map(Some).map_err(|e| RpcFailure::Error(format!("blockhash: {e}")))
}

// ---------------------------------------------------------------------------
// The wasm transport
// ---------------------------------------------------------------------------

/// Candid call wrapper with cycles attached. Off-chain (unit tests) every call is an error.
#[cfg(target_arch = "wasm32")]
async fn call_sol_rpc<A, R>(canister: candid::Principal, method: &str, args: A, cycles: u128) -> RpcResult<R>
where
    A: candid::utils::ArgumentEncoder,
    R: for<'a> candid::utils::ArgumentDecoder<'a>,
{
    ic_cdk::api::call::call_with_payment128(canister, method, args, cycles)
        .await
        .map_err(|(code, message)| RpcFailure::Error(format!("{method} on {canister} failed ({code:?}): {message}")))
}

#[cfg(not(target_arch = "wasm32"))]
async fn call_sol_rpc<A, R>(canister: candid::Principal, method: &str, _args: A, _cycles: u128) -> RpcResult<R>
where
    A: candid::utils::ArgumentEncoder,
    R: for<'a> candid::utils::ArgumentDecoder<'a>,
{
    Err(RpcFailure::Error(format!("{method} on {canister}: inter-canister calls are unavailable off-chain")))
}

/// The real transport: typed calls to the SOL RPC canister.
#[derive(Clone, Debug)]
pub struct CandidRpc {
    pub canister: candid::Principal,
    pub settings: RpcSettings,
}

impl CandidRpc {
    /// Built from the stored config: the canister id is an install argument, never a default.
    pub fn from_config(canister: candid::Principal, settings: RpcSettings) -> RpcResult<Self> {
        if canister == candid::Principal::anonymous() {
            return Err(RpcFailure::Error("SOL RPC canister id is not configured".into()));
        }
        rpc_sources(&settings)?;
        Ok(CandidRpc { canister, settings })
    }

    fn cycles(&self) -> u128 {
        self.settings.cycles_per_call as u128
    }
}

impl SolRpc for CandidRpc {
    async fn get_account(&self, address: &Pubkey) -> RpcResult<Option<AccountSnapshot>> {
        let args = (rpc_sources(&self.settings)?, Some(rpc_config(&self.settings, self.settings.verify)), account_info_params(address));
        let (reply,): (wire::Multi<Option<wire::AccountInfo>>,) = call_sol_rpc(self.canister, "getAccountInfo", args, self.cycles()).await?;
        account_from_wire(unwrap_multi(reply)?)
    }

    async fn get_transaction(&self, signature: &Signature) -> RpcResult<Option<LandedTx>> {
        let args = (rpc_sources(&self.settings)?, Some(rpc_config(&self.settings, self.settings.verify)), transaction_params(signature));
        let (reply,): (wire::Multi<Option<wire::EncodedConfirmedTransactionWithStatusMeta>>,) =
            call_sol_rpc(self.canister, "getTransaction", args, self.cycles()).await?;
        let landed = transaction_from_wire(unwrap_multi(reply)?)?;
        if let Some(tx) = &landed {
            check_bound(signature, tx).map_err(RpcFailure::Error)?;
        }
        Ok(landed)
    }

    async fn get_signatures_for_address(
        &self,
        address: &Pubkey,
        before: Option<&Signature>,
        until: Option<&Signature>,
        limit: u32,
    ) -> RpcResult<Vec<SignatureEntry>> {
        let args = (
            rpc_sources(&self.settings)?,
            Some(rpc_config(&self.settings, self.settings.discover)),
            signatures_for_address_params(address, before, until, limit),
        );
        let (reply,): (wire::Multi<Vec<wire::ConfirmedTransactionStatusWithSignature>>,) =
            call_sol_rpc(self.canister, "getSignaturesForAddress", args, self.cycles()).await?;
        signatures_from_wire(unwrap_multi(reply)?)
    }

    async fn get_signature_statuses(
        &self,
        signatures: &[Signature],
        search_transaction_history: bool,
    ) -> RpcResult<Vec<Option<SignatureStatus>>> {
        let args = (
            rpc_sources(&self.settings)?,
            Some(rpc_config(&self.settings, self.settings.verify)),
            signature_statuses_params(signatures, search_transaction_history),
        );
        let (reply,): (wire::Multi<Vec<Option<wire::TransactionStatus>>>,) =
            call_sol_rpc(self.canister, "getSignatureStatuses", args, self.cycles()).await?;
        Ok(statuses_from_wire(unwrap_multi(reply)?))
    }

    async fn send_transaction(&self, wire_tx: &[u8]) -> RpcResult<Signature> {
        let args = (rpc_sources(&self.settings)?, Some(rpc_config(&self.settings, self.settings.verify)), send_transaction_params(wire_tx));
        let (reply,): (wire::Multi<String>,) = call_sol_rpc(self.canister, "sendTransaction", args, self.cycles()).await?;
        let text = unwrap_multi(reply)?;
        crate::tx::signature_from_base58(&text).map_err(|e| RpcFailure::Error(format!("sendTransaction reply: {e}")))
    }

    async fn get_slot(&self) -> RpcResult<u64> {
        let args = (rpc_sources(&self.settings)?, Some(slot_config(&self.settings)), Some(slot_params()));
        let (reply,): (wire::Multi<u64>,) = call_sol_rpc(self.canister, "getSlot", args, self.cycles()).await?;
        unwrap_multi(reply)
    }

    async fn get_block_blockhash(&self, slot: u64) -> RpcResult<Option<[u8; 32]>> {
        let args = (rpc_sources(&self.settings)?, Some(rpc_config(&self.settings, self.settings.verify)), block_params(slot));
        let (reply,): (wire::Multi<Option<wire::ConfirmedBlock>>,) = call_sol_rpc(self.canister, "getBlock", args, self.cycles()).await?;
        blockhash_from_wire(unwrap_multi(reply)?)
    }
}

/// The README bootstrap route for one real blockhash (no `getLatestBlockhash`, whose per-slot
/// value cannot reach consensus): `getSlot` (rounded down by the canister), then `getBlock` of
/// that slot, walking back over skipped slots at most `max_back` times.
pub async fn bootstrap_blockhash<R: SolRpc>(rpc: &R, max_back: u64) -> RpcResult<[u8; 32]> {
    let tip = rpc.get_slot().await?;
    for back in 0..=max_back {
        let Some(slot) = tip.checked_sub(back) else { break };
        if let Some(hash) = rpc.get_block_blockhash(slot).await? {
            return Ok(hash);
        }
    }
    Err(RpcFailure::Error(format!("no block found within {max_back} slots of finalized slot {tip}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{block_on, MockChain};
    use crate::tx::{pubkey_to_base58, signature_to_base58};
    use candid::{Encode, Principal};
    use candid_parser::utils::CandidSource;

    const FIXTURE: &str = include_str!("../tests/fixtures/sol_rpc_canister.did");

    /// The fixture is the pinned upstream file (git blob cd456a3b..., 39,673 bytes, see the module
    /// docs); an edit to it, even a comment, must be a deliberate re-pin.
    #[test]
    fn the_fixture_is_the_pinned_upstream_file() {
        use sha2::{Digest as _, Sha256};
        assert_eq!(FIXTURE.len(), 39_673);
        assert_eq!(hex::encode(Sha256::digest(FIXTURE.as_bytes())), "7c8698882a6cc6f99fdc32329883b058cb5758889892fd845e18fc79b9fcc104");
    }

    fn settings() -> RpcSettings {
        RpcSettings { cluster: Some(SolRpcCluster::Devnet), ..RpcSettings::default() }
    }

    fn fixture_env() -> (candid::TypeEnv, candid::types::Type) {
        let (env, actor) = CandidSource::Text(FIXTURE).load().expect("the pinned interface parses");
        (env, actor.expect("the interface has a service"))
    }

    /// Our encoded request must decode against the fixture's argument types of `method`.
    fn assert_request_fits(method: &str, bytes: Vec<u8>) {
        let (env, actor) = fixture_env();
        let func = env.get_method(&actor, method).unwrap_or_else(|e| panic!("{method}: {e}"));
        candid::IDLArgs::from_bytes_with_types(&bytes, &env, &func.args).unwrap_or_else(|e| panic!("{method}: our arguments do not fit the interface: {e}"));
    }

    /// Build a reply from candid text in the fixture's return type, then decode it with ours.
    fn reply_bytes(method: &str, text: &str) -> Vec<u8> {
        let (env, actor) = fixture_env();
        let func = env.get_method(&actor, method).unwrap();
        let args = candid_parser::parse_idl_args(text).unwrap_or_else(|e| panic!("{method}: {e}\n{text}"));
        args.to_bytes_with_types(&env, &func.rets).unwrap_or_else(|e| panic!("{method}: the sample reply does not fit the interface: {e}\n{text}"))
    }

    fn addr() -> Pubkey {
        [0x55; 32]
    }

    fn sig(b: u8) -> Signature {
        [b; 64]
    }

    #[test]
    fn mirror_requests_fit_the_pinned_interface() {
        let s = settings();
        let sources = || rpc_sources(&s).unwrap();
        let cfg = |c| Some(rpc_config(&s, c));
        assert_request_fits("getAccountInfo", Encode!(&sources(), &cfg(s.verify), &account_info_params(&addr())).unwrap());
        assert_request_fits("getTransaction", Encode!(&sources(), &cfg(s.verify), &transaction_params(&sig(7))).unwrap());
        assert_request_fits(
            "getSignaturesForAddress",
            Encode!(&sources(), &cfg(s.discover), &signatures_for_address_params(&addr(), Some(&sig(1)), Some(&sig(2)), 10)).unwrap(),
        );
        assert_request_fits(
            "getSignaturesForAddress",
            Encode!(&sources(), &cfg(s.discover), &signatures_for_address_params(&addr(), None, None, 10)).unwrap(),
        );
        assert_request_fits("getSignatureStatuses", Encode!(&sources(), &cfg(s.verify), &signature_statuses_params(&[sig(1), sig(2)], true)).unwrap());
        assert_request_fits("sendTransaction", Encode!(&sources(), &cfg(s.verify), &send_transaction_params(&[1, 2, 3])).unwrap());
        assert_request_fits("getSlot", Encode!(&sources(), &Some(slot_config(&s)), &Some(slot_params())).unwrap());
        assert_request_fits("getBlock", Encode!(&sources(), &cfg(s.verify), &block_params(42)).unwrap());
        // Every consensus setting we can configure is accepted.
        for c in [ConsensusSetting::Equality, ConsensusSetting::Threshold { total: Some(3), min: 2 }, ConsensusSetting::Threshold { total: None, min: 1 }] {
            assert_request_fits("getAccountInfo", Encode!(&sources(), &cfg(c), &account_info_params(&addr())).unwrap());
        }
        // The three clusters.
        for cluster in [SolRpcCluster::Mainnet, SolRpcCluster::Devnet, SolRpcCluster::Testnet] {
            let st = RpcSettings { cluster: Some(cluster), ..RpcSettings::default() };
            assert_request_fits("getAccountInfo", Encode!(&rpc_sources(&st).unwrap(), &Some(rpc_config(&st, st.verify)), &account_info_params(&addr())).unwrap());
        }
    }

    #[test]
    fn the_request_parameters_are_what_the_lane_relies_on() {
        let p = account_info_params(&addr());
        assert!(matches!(p.commitment, Some(wire::CommitmentLevel::finalized)));
        assert!(matches!(p.encoding, Some(wire::GetAccountInfoEncoding::base64)));
        assert_eq!(p.pubkey, pubkey_to_base58(&addr()));
        let t = transaction_params(&sig(9));
        assert!(matches!(t.commitment, Some(wire::CommitmentLevel::finalized)));
        assert!(matches!(t.encoding, Some(wire::BinaryEncoding::base64)));
        assert_eq!(t.maxSupportedTransactionVersion, Some(0));
        let l = signatures_for_address_params(&addr(), Some(&sig(1)), Some(&sig(2)), 7);
        assert_eq!((l.limit, l.before.as_deref(), l.until.as_deref()), (Some(7), Some(signature_to_base58(&sig(1)).as_str()), Some(signature_to_base58(&sig(2)).as_str())));
        assert!(matches!(l.commitment, Some(wire::CommitmentLevel::finalized)));
        let st = signature_statuses_params(&[sig(1)], true);
        assert_eq!(st.searchTransactionHistory, Some(true));
        let send = send_transaction_params(&[0xde, 0xad]);
        assert_eq!(send.transaction, "3q0=");
        assert_eq!(send.skipPreflight, Some(false));
        assert!(matches!(slot_params().commitment, Some(wire::CommitmentLevel::finalized)));
        let b = block_params(5);
        assert!(matches!(b.transactionDetails, Some(wire::TransactionDetails::none)));
        assert!(matches!(b.commitment, Some(wire::BlockCommitment::finalized)));
        // An unconfigured cluster is refused, never defaulted.
        assert_eq!(rpc_sources(&RpcSettings::default()).unwrap_err(), RpcFailure::Error("SOL RPC cluster is not configured".into()));
    }

    #[test]
    fn mirror_replies_decode_from_the_pinned_interface() {
        let owner = pubkey_to_base58(&crate::tx::SYSTEM_PROGRAM_ID);
        let nonce_b64 = base64::engine::general_purpose::STANDARD.encode(crate::testkit::nonce_account_data(1, 1, &[0x11; 32], &[0x66; 32]));

        // getAccountInfo: present, absent, error, inconsistent.
        let present = format!(
            r#"(variant {{ Consistent = variant {{ Ok = opt record {{ lamports = 1_447_680 : nat64; data = variant {{ binary = record {{ "{nonce_b64}"; variant {{ base64 }} }} }}; owner = "{owner}"; executable = false; rentEpoch = 0 : nat64; space = 80 : nat64 }} }} }})"#
        );
        let (reply,): (wire::Multi<Option<wire::AccountInfo>>,) = candid::decode_args(&reply_bytes("getAccountInfo", &present)).unwrap();
        let snap = account_from_wire(unwrap_multi(reply).unwrap()).unwrap().unwrap();
        assert_eq!(snap.owner, crate::tx::SYSTEM_PROGRAM_ID);
        assert_eq!(snap.data, crate::testkit::nonce_account_data(1, 1, &[0x11; 32], &[0x66; 32]));
        let (absent,): (wire::Multi<Option<wire::AccountInfo>>,) =
            candid::decode_args(&reply_bytes("getAccountInfo", "(variant { Consistent = variant { Ok = null } })")).unwrap();
        assert_eq!(account_from_wire(unwrap_multi(absent).unwrap()).unwrap(), None, "a consistent null");
        let err_text = r#"(variant { Consistent = variant { Err = variant { ProviderError = variant { TooFewCycles = record { expected = 5 : nat; received = 1 : nat } } } } })"#;
        let (err,): (wire::Multi<Option<wire::AccountInfo>>,) = candid::decode_args(&reply_bytes("getAccountInfo", err_text)).unwrap();
        assert_eq!(unwrap_multi(err).err(), Some(RpcFailure::Error("provider error".into())));
        let json_err = r#"(variant { Consistent = variant { Err = variant { JsonRpcError = record { code = -32000 : int64; message = "boom" } } } })"#;
        let (err,): (wire::Multi<Option<wire::AccountInfo>>,) = candid::decode_args(&reply_bytes("getAccountInfo", json_err)).unwrap();
        assert_eq!(unwrap_multi(err).err(), Some(RpcFailure::Error("json-rpc error -32000: boom".into())));
        let inconsistent = r#"(variant { Inconsistent = vec { record { variant { Supported = variant { AlchemyDevnet } }; variant { Ok = null } }; record { variant { Supported = variant { AnkrDevnet } }; variant { Err = variant { ValidationError = "x" } } } } })"#;
        let (inc,): (wire::Multi<Option<wire::AccountInfo>>,) = candid::decode_args(&reply_bytes("getAccountInfo", inconsistent)).unwrap();
        assert_eq!(unwrap_multi(inc).err(), Some(RpcFailure::Inconsistent));
        // Account data that is not base64 binary is an error, not a guess.
        let json_data = format!(
            r#"(variant {{ Consistent = variant {{ Ok = opt record {{ lamports = 1 : nat64; data = variant {{ legacyBinary = "abc" }}; owner = "{owner}"; executable = false; rentEpoch = 0 : nat64; space = 3 : nat64 }} }} }})"#
        );
        let (bad,): (wire::Multi<Option<wire::AccountInfo>>,) = candid::decode_args(&reply_bytes("getAccountInfo", &json_data)).unwrap();
        assert!(account_from_wire(unwrap_multi(bad).unwrap()).is_err());

        // getTransaction: success, failure (a TransactionError payload), absent.
        let wire_tx = vec![1u8, 2, 3, 4];
        let tx_b64 = base64::engine::general_purpose::STANDARD.encode(&wire_tx);
        let tx_text = |status: &str| {
            format!(
                r#"(variant {{ Consistent = variant {{ Ok = opt record {{ slot = 99 : nat64; blockTime = null; transaction = record {{ meta = opt record {{ fee = 5000 : nat64; status = {status}; innerInstructions = null; postTokenBalances = null; preBalances = vec {{}}; postBalances = vec {{}}; returnData = null; logMessages = null; rewards = null; loadedAddresses = null; preTokenBalances = null; computeUnitsConsumed = null; cost_units = null }}; transaction = variant {{ binary = record {{ "{tx_b64}"; variant {{ base64 }} }} }}; version = opt variant {{ legacy }} }} }} }} }})"#
            )
        };
        let (ok,): (wire::Multi<Option<wire::EncodedConfirmedTransactionWithStatusMeta>>,) = candid::decode_args(&reply_bytes("getTransaction", &tx_text("variant { Ok }"))).unwrap();
        assert_eq!(transaction_from_wire(unwrap_multi(ok).unwrap()).unwrap(), Some(LandedTx { slot: 99, wire: wire_tx.clone(), failed: false, meta: TxMeta::default() }));
        let failed_status = "variant { Err = variant { InstructionError = record { 1 : nat8; variant { Custom = 7 : nat32 } } } }";
        let (failed,): (wire::Multi<Option<wire::EncodedConfirmedTransactionWithStatusMeta>>,) = candid::decode_args(&reply_bytes("getTransaction", &tx_text(failed_status))).unwrap();
        assert_eq!(transaction_from_wire(unwrap_multi(failed).unwrap()).unwrap(), Some(LandedTx { slot: 99, wire: wire_tx, failed: true, meta: TxMeta::default() }));
        let (none,): (wire::Multi<Option<wire::EncodedConfirmedTransactionWithStatusMeta>>,) =
            candid::decode_args(&reply_bytes("getTransaction", "(variant { Consistent = variant { Ok = null } })")).unwrap();
        assert_eq!(transaction_from_wire(unwrap_multi(none).unwrap()).unwrap(), None);

        // getSignaturesForAddress.
        let listed = format!(
            r#"(variant {{ Consistent = variant {{ Ok = vec {{ record {{ signature = "{}"; slot = 5 : nat64; err = null; memo = opt "[9] rumi1|m|ab"; blockTime = null; confirmationStatus = opt variant {{ finalized }} }}; record {{ signature = "{}"; slot = 4 : nat64; err = opt variant {{ AccountInUse }}; memo = null; blockTime = opt (1 : int64); confirmationStatus = null }} }} }} }})"#,
            signature_to_base58(&sig(1)),
            signature_to_base58(&sig(2))
        );
        let (rows,): (wire::Multi<Vec<wire::ConfirmedTransactionStatusWithSignature>>,) = candid::decode_args(&reply_bytes("getSignaturesForAddress", &listed)).unwrap();
        let entries = signatures_from_wire(unwrap_multi(rows).unwrap()).unwrap();
        assert_eq!(entries[0], SignatureEntry { signature: sig(1), slot: 5, failed: false, memo: Some("[9] rumi1|m|ab".into()) });
        assert_eq!(entries[1], SignatureEntry { signature: sig(2), slot: 4, failed: true, memo: None });

        // getSignatureStatuses.
        let statuses = r#"(variant { Consistent = variant { Ok = vec { opt record { slot = 5 : nat64; status = variant { Ok }; err = null; confirmationStatus = opt variant { finalized } }; null; opt record { slot = 6 : nat64; status = variant { Err = variant { BlockhashNotFound } }; err = opt variant { BlockhashNotFound }; confirmationStatus = null } } } })"#;
        let (st,): (wire::Multi<Vec<Option<wire::TransactionStatus>>>,) = candid::decode_args(&reply_bytes("getSignatureStatuses", statuses)).unwrap();
        assert_eq!(statuses_from_wire(unwrap_multi(st).unwrap()), vec![Some(SignatureStatus { slot: 5, failed: false }), None, Some(SignatureStatus { slot: 6, failed: true })]);

        // sendTransaction, getSlot, getBlock.
        let send = format!(r#"(variant {{ Consistent = variant {{ Ok = "{}" }} }})"#, signature_to_base58(&sig(3)));
        let (s,): (wire::Multi<String>,) = candid::decode_args(&reply_bytes("sendTransaction", &send)).unwrap();
        assert_eq!(unwrap_multi(s).unwrap(), signature_to_base58(&sig(3)));
        let (slot,): (wire::Multi<u64>,) = candid::decode_args(&reply_bytes("getSlot", "(variant { Consistent = variant { Ok = 1_234 : nat64 } })")).unwrap();
        assert_eq!(unwrap_multi(slot).unwrap(), 1_234);
        let hash = pubkey_to_base58(&[0x77; 32]);
        let block = format!(
            r#"(variant {{ Consistent = variant {{ Ok = opt record {{ previousBlockhash = "{hash}"; blockhash = "{hash}"; parentSlot = 1 : nat64; blockTime = null; blockHeight = null; signatures = null; rewards = null; numRewardPartition = null; transactions = null }} }} }})"#
        );
        let (b,): (wire::Multi<Option<wire::ConfirmedBlock>>,) = candid::decode_args(&reply_bytes("getBlock", &block)).unwrap();
        assert_eq!(blockhash_from_wire(unwrap_multi(b).unwrap()).unwrap(), Some([0x77; 32]));
        let (nb,): (wire::Multi<Option<wire::ConfirmedBlock>>,) = candid::decode_args(&reply_bytes("getBlock", "(variant { Consistent = variant { Ok = null } })")).unwrap();
        assert_eq!(blockhash_from_wire(unwrap_multi(nb).unwrap()).unwrap(), None);
    }

    #[test]
    fn conversions_reject_malformed_replies() {
        assert!(decode_base64("!!!", "x").is_err());
        assert!(signatures_from_wire(vec![wire::ConfirmedTransactionStatusWithSignature { signature: "short".into(), slot: 1, err: None, memo: None }]).is_err());
        assert!(blockhash_from_wire(Some(wire::ConfirmedBlock { blockhash: "1111".into() })).is_err());
        let no_meta = wire::EncodedConfirmedTransactionWithStatusMeta {
            slot: 1,
            transaction: wire::EncodedTransactionWithStatusMeta { meta: None, transaction: wire::EncodedTransaction::binary(("AQID".into(), candid::Reserved)) },
        };
        assert!(transaction_from_wire(Some(no_meta)).is_err(), "a transaction without meta cannot be classified");
        let legacy = wire::EncodedConfirmedTransactionWithStatusMeta {
            slot: 1,
            transaction: wire::EncodedTransactionWithStatusMeta {
                meta: Some(wire::TransactionStatusMeta { status: wire::ExecutionStatus::Ok, innerInstructions: None, preTokenBalances: None, postTokenBalances: None }),
                transaction: wire::EncodedTransaction::legacyBinary("x".into()),
            },
        };
        assert!(transaction_from_wire(Some(legacy)).is_err());
    }

    #[test]
    fn the_transport_needs_a_configured_canister_and_cluster_and_is_off_chain_in_tests() {
        assert!(CandidRpc::from_config(Principal::anonymous(), settings()).is_err(), "no default canister id");
        assert!(CandidRpc::from_config(Principal::from_slice(&[1; 10]), RpcSettings::default()).is_err(), "no default cluster");
        let rpc = CandidRpc::from_config(Principal::from_slice(&[1; 10]), settings()).unwrap();
        let r = block_on(rpc.get_slot());
        assert!(matches!(r, Err(RpcFailure::Error(m)) if m.contains("unavailable off-chain")));
        assert!(block_on(rpc.get_account(&addr())).is_err());
        assert!(block_on(rpc.send_transaction(&[1])).is_err());
    }

    #[test]
    fn bootstrap_blockhash_walks_back_over_skipped_slots() {
        let chain = MockChain::new();
        chain.tip_slot.set(100);
        chain.blocks.borrow_mut().insert(98, [0x42; 32]);
        assert_eq!(block_on(bootstrap_blockhash(&chain, 5)).unwrap(), [0x42; 32]);
        assert!(block_on(bootstrap_blockhash(&chain, 1)).is_err(), "gives up after max_back slots");
        chain.fail_next("get_slot", crate::testkit::Fault::Inconsistent);
        assert_eq!(block_on(bootstrap_blockhash(&chain, 5)), Err(RpcFailure::Inconsistent));
    }

    #[test]
    fn transaction_meta_carries_inner_instruction_groups_and_token_balances() {
        use base64::Engine as _;
        let wire_tx = vec![1u8, 2, 3, 4];
        let tx_b64 = base64::engine::general_purpose::STANDARD.encode(&wire_tx);
        let mint = crate::tx::pubkey_to_base58(&[0x44; 32]);
        let owner = crate::tx::pubkey_to_base58(&[0x21; 32]);
        let bal = |idx: u8, amount: &str, owner: Option<&str>| {
            let owner = owner.map(|o| format!("opt \"{o}\"")).unwrap_or_else(|| "null".to_string());
            format!(
                r#"record {{ owner = {owner}; mint = "{mint}"; programId = null; accountIndex = {idx} : nat8; uiTokenAmount = record {{ decimals = 8 : nat8; uiAmount = null; uiAmountString = "0"; amount = "{amount}" }} }}"#
            )
        };
        let reply = |inner: &str, pre: &str, post: &str| {
            format!(
                r#"(variant {{ Consistent = variant {{ Ok = opt record {{ slot = 7 : nat64; blockTime = null; transaction = record {{ meta = opt record {{ fee = 5000 : nat64; status = variant {{ Ok }}; innerInstructions = {inner}; postTokenBalances = {post}; preBalances = vec {{}}; postBalances = vec {{}}; returnData = null; logMessages = null; rewards = null; loadedAddresses = null; preTokenBalances = {pre}; computeUnitsConsumed = null; cost_units = null }}; transaction = variant {{ binary = record {{ "{tx_b64}"; variant {{ base64 }} }} }}; version = opt variant {{ legacy }} }} }} }} }})"#
            )
        };
        let pre = format!("opt vec {{ {}; {} }}", bal(1, "500", Some(&owner)), bal(2, "100", None));
        let post = format!("opt vec {{ {}; {} }}", bal(1, "300", Some(&owner)), bal(2, "300", None));
        let text = reply("opt vec {}", &pre, &post);
        let (r,): (wire::Multi<Option<wire::EncodedConfirmedTransactionWithStatusMeta>>,) = candid::decode_args(&reply_bytes("getTransaction", &text)).unwrap();
        let tx = transaction_from_wire(unwrap_multi(r).unwrap()).unwrap().unwrap();
        assert_eq!(tx.meta.inner_instruction_groups, Some(0));
        let m = [0x44; 32];
        assert_eq!(
            tx.meta.pre_token_balances,
            Some(vec![
                TokenBalanceEntry { account_index: 1, mint: m, owner: Some([0x21; 32]), amount: 500, decimals: 8 },
                TokenBalanceEntry { account_index: 2, mint: m, owner: None, amount: 100, decimals: 8 },
            ])
        );
        assert_eq!(tx.meta.post_token_balances.as_ref().map(|b| (b[0].amount, b[1].amount)), Some((300, 300)));
        // Not recorded stays "not recorded": the observer must not read it as empty or zero.
        let text = reply("null", "null", "null");
        let (r,): (wire::Multi<Option<wire::EncodedConfirmedTransactionWithStatusMeta>>,) = candid::decode_args(&reply_bytes("getTransaction", &text)).unwrap();
        let tx = transaction_from_wire(unwrap_multi(r).unwrap()).unwrap().unwrap();
        assert_eq!(tx.meta, TxMeta::default());
        // Inner instruction groups are counted.
        let inner = r#"opt vec { record { instructions = vec {}; index = 0 : nat8 } }"#;
        let text = reply(inner, "null", "null");
        let (r,): (wire::Multi<Option<wire::EncodedConfirmedTransactionWithStatusMeta>>,) = candid::decode_args(&reply_bytes("getTransaction", &text)).unwrap();
        assert_eq!(transaction_from_wire(unwrap_multi(r).unwrap()).unwrap().unwrap().meta.inner_instruction_groups, Some(1));
        // A balance the observer cannot read is an error, not a guess.
        let bad = format!("opt vec {{ {} }}", bal(1, "not a number", None));
        let text = reply("opt vec {}", &bad, &bad);
        let (r,): (wire::Multi<Option<wire::EncodedConfirmedTransactionWithStatusMeta>>,) = candid::decode_args(&reply_bytes("getTransaction", &text)).unwrap();
        assert!(transaction_from_wire(unwrap_multi(r).unwrap()).is_err());
    }
}
