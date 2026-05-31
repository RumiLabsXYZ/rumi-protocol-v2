//! EVM RPC wrapper for Monad chain state reads and signed-tx broadcasting.
//!
//! Design: most reads call the EVM RPC canister (`7hfb6-caaaa-aaaar-qadga-cai`)
//! via the generic `request` escape-hatch method using `call_with_payment128`.
//! This avoids the `evm_rpc_client` crate (which requires ic-cdk 0.19,
//! incompatible with this project's ic-cdk 0.12 pin).  JSON-RPC responses are
//! parsed with `serde_json` (already a workspace dep).
//!
//! Exception — finalized-height reads (`fetch_block_numbers`): these go through
//! the TYPED `eth_getBlockByNumber(Number(N))` method (candid types mirrored
//! below), NOT `request`.  A volatile chain-head read (`eth_blockNumber`, or any
//! `latest`/`finalized` tag) differs across the EVM RPC canister's subnet
//! replicas on a fast-finality chain like Monad → IC HTTPS-outcall consensus
//! never agrees → the call fails every tick.  Probing a SPECIFIC, already-final
//! block number is byte-identical across replicas, so it reaches consensus.
//!
//! Cycle cost: `EVM_RPC_CALL_CYCLES` (2_000_000_000) per request.  This is
//! intentionally generous — the actual HTTPS-outcall cost depends on response
//! size and subnet node count but is typically in the hundreds-of-millions
//! range.  The constant is marked tunable; a developer-gated setter can be
//! added once we have production measurements.
//!
//! Provider fallback: `call_evm_rpc` iterates over the chain's configured
//! `rpc_endpoints` in order and returns the first `Ok` response; on all-fail
//! it returns the last error.
//!
//! Pure parsers (`parse_hex_quantity`, `BurnLog::from_raw`,
//! `MintLog::from_raw`) are unit-tested in `tests_evm_rpc`.  The async
//! network functions are covered by the Task 17 PocketIC mock.
//!
//! Candid types (`RpcApi`, `RpcService`, `RequestResult`, `RpcError`) are
//! defined inline here from the live .did (verified 2026-05-28 by fetching
//! `7hfb6-caaaa-aaaar-qadga-cai candid:service`).

use candid::{CandidType, Deserialize, Principal};
use ic_canister_log::log;
use crate::chains::config::ChainId;
use crate::state::read_state;
use crate::logs::{DEBUG, INFO};

// ─── Cycle cost ─────────────────────────────────────────────────────────────

/// Cycles attached per `request` call to the EVM RPC canister.
///
/// Tunable constant.  The canister's HTTPS-outcall forwarding cost is roughly:
///   base(49_140_000) + 5_200*request_bytes + 10_400*response_bytes,
/// scaled by the number of nodes in the subnet (13 for the NNS app subnet).
/// For a typical 200-byte request and 2 KB response on a 13-node subnet that
/// is approximately 400M cycles.  2B gives ~5x headroom and matches the
/// approach used for the XRC `get_exchange_rate` call.
pub const EVM_RPC_CALL_CYCLES: u128 = 2_000_000_000;

/// Maximum response bytes requested from the EVM RPC canister per call.
/// Sized to cover typical JSON-RPC responses; `eth_getLogs` responses can be
/// large — callers using a narrow block range stay within this limit.
const EVM_RPC_MAX_RESPONSE_BYTES: u64 = 8192;

/// Max blocks the burn-watch cursor advances per observer tick. The observer
/// reads chain head by probing a SPECIFIC block number `last_observed + this`
/// (consensus-safe), never a volatile `latest`/`finalized` tag.
///
/// ## Sizing (must be >= blocks-produced-per-observer-interval)
///
/// The probe jumps exactly this many blocks whenever block `last_observed +
/// this` exists and is final; otherwise the cursor waits. Over the long run the
/// cursor therefore advances at most `this` blocks per tick, so to keep pace with
/// the chain it MUST be >= the number of blocks Monad produces per observer
/// interval. Falling below that makes the cursor lag unboundedly (burns observed
/// with ever-growing delay).
///
/// Measured 2026-05-31: Monad testnet produces ~1.93 blocks/s, and the staging
/// observer interval is 300s (set live for cycle-burn mitigation), i.e. ~578
/// blocks/tick. The previous value (256) was sized for a 30s interval (~60
/// blocks/tick) and could NOT keep pace at 300s — that, together with the
/// 100-block getLogs cap, is why the Gate-4 cursor stalled. 1024 comfortably
/// exceeds 578 (1.77x margin, covers up to ~3.4 blocks/s at 300s) and bounds the
/// observation lag at roughly one window (~512s). It also stays correct at the
/// 30s default interval (the cursor advances in 1024-block bursts rather than
/// every tick, but still keeps pace). If the observer interval is raised well
/// beyond 300s, or Monad's block rate rises substantially, raise this too.
///
/// `get_logs` chunks the per-tick scan into <= `MONAD_GETLOGS_MAX_RANGE`-block
/// sub-queries, so this window is NOT bounded by the provider's per-query
/// getLogs cap (a 1024-block window scans as ceil(1024/100) = 11 sub-queries).
/// Note: the total getLogs volume per day is set by blocks/day ÷ 100 regardless
/// of this window — a larger window only changes burst size and lag, not cost.
pub const MAX_BLOCK_SCAN_WINDOW: u64 = 1024;

/// Maximum `eth_getLogs` block range (`toBlock - fromBlock`) the Monad testnet
/// RPC accepts in a SINGLE query. A difference of 101 returns HTTP 413 with
/// JSON-RPC code -32614 ("eth_getLogs is limited to a 100 range"); a difference
/// of 100 (a 101-block span) is accepted. Empirically confirmed against
/// `https://testnet-rpc.monad.xyz` on 2026-05-31. `get_logs` pages any wider
/// range into sequential sub-queries each respecting this cap, so callers (the
/// burn-watch loop and mint-confirm scan) never need to know about it. Kept in
/// sync with the same-named constant in `src/monad_rpc_mock/src/lib.rs`.
pub const MONAD_GETLOGS_MAX_RANGE: u64 = 100;

// ─── Topic0 constants ────────────────────────────────────────────────────────

/// keccak256("Burn(uint256,address,uint256)")
/// Computed via pycryptodome Keccak-256 (verified against the Transfer
/// event well-known hash to confirm the implementation is correct).
pub const BURN_EVENT_TOPIC0: &str =
    "0xe1b6e34006e9871307436c226f232f9c5e7690c1d2c4f4adda4f607a75a9beca";

/// keccak256("Mint(uint256,address,uint256)")
pub const MINT_EVENT_TOPIC0: &str =
    "0x4e3883c75cc9c752bb1db2e406a822e4a75067ae77ad9a0a4d179f2709b9e1f6";

// ─── Candid types for the EVM RPC canister `request` method ─────────────────
//
// Source: live .did of `7hfb6-caaaa-aaaar-qadga-cai` (fetched 2026-05-28).
// We define only the minimal surface needed for `request` + `RequestResult`.
// The full .did has many more variants (EthMainnet, EthSepolia, etc.) which
// we do not need since Monad is always addressed via the `Custom` variant.

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct EvmRpcHttpHeader {
    pub name: String,
    pub value: String,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct RpcApi {
    pub url: String,
    pub headers: Option<Vec<EvmRpcHttpHeader>>,
}

/// Minimal `RpcService` — only the `Custom` variant is needed for Monad
/// (all other variants address built-in chains by name).
#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum RpcService {
    Custom(RpcApi),
    // Other built-in variants exist in the .did but are unused here.
    // Candid decoding is flexible: we only need to encode Custom.
}

/// Mirrors the live .did `JsonRpcError` record.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
}

/// Mirrors the live .did `ProviderError` variant.
/// `TooFewCycles` carries an anonymous record `{ expected: nat; received: nat }`.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct TooFewCyclesRecord {
    pub expected: candid::Nat,
    pub received: candid::Nat,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ProviderError {
    TooFewCycles(TooFewCyclesRecord),
    MissingRequiredProvider,
    ProviderNotFound,
    NoPermission,
    InvalidRpcConfig(String),
}

/// Mirrors the live .did `RejectionCode` variant (fieldless).
///
/// The EVM RPC canister returns `IcError { code: RejectionCode; ... }` where
/// `RejectionCode` is a Candid variant.  Previously this was declared as `u32`
/// which caused a Candid decode trap whenever the canister returned an
/// `HttpOutcallError::IcError` (e.g. during no-consensus scenarios).
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum RejectionCode {
    NoError,
    CanisterError,
    SysTransient,
    DestinationInvalid,
    Unknown,
    SysFatal,
    CanisterReject,
}

/// Mirrors the live .did `HttpOutcallError` variant.
/// `IcError` carries `{ code: RejectionCode; message: text }`.
/// `InvalidHttpJsonRpcResponse` carries `{ status: nat16; body: text; parsingError: opt text }`.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct IcErrorRecord {
    pub code: RejectionCode,
    pub message: String,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct InvalidHttpJsonRpcRecord {
    pub status: u16,
    pub body: String,
    #[serde(rename = "parsingError")]
    pub parsing_error: Option<String>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum HttpOutcallError {
    IcError(IcErrorRecord),
    InvalidHttpJsonRpcResponse(InvalidHttpJsonRpcRecord),
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ValidationError {
    Custom(String),
    InvalidHex(String),
}

/// Full `RpcError` as in the live .did.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum RpcError {
    JsonRpcError(JsonRpcError),
    ProviderError(ProviderError),
    ValidationError(ValidationError),
    HttpOutcallError(HttpOutcallError),
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

/// `RequestResult` from the EVM RPC canister `request` method.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum RequestResult {
    Ok(String),
    Err(RpcError),
}

// ─── Candid types for the TYPED `eth_getBlockByNumber` method ────────────────
//
// Source: live .did of `7hfb6-caaaa-aaaar-qadga-cai` (see
// `docs/evm_rpc_reference.did`).  Unlike the `request` escape-hatch above,
// `eth_getBlockByNumber` takes the PLURAL `RpcServices` (multi-provider) and a
// `BlockTag`, and returns a `MultiGetBlockByNumberResult`.  We use this method
// (with a SPECIFIC `Number(N)` tag) for the consensus-safe finalized-height
// probe — a fixed block number is byte-identical across all IC replicas,
// unlike the volatile `eth_blockNumber` / `latest` / `finalized` reads.

/// PLURAL `RpcServices` (distinct from the singular `RpcService` above).
/// Only the `Custom` arm is mirrored — Monad is always a single custom
/// provider keyed by `chainId`.  The other arms (`EthMainnet`, etc.) exist in
/// the .did but are never encoded here.
#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum RpcServices {
    Custom {
        #[serde(rename = "chainId")]
        chain_id: u64,
        services: Vec<RpcApi>,
    },
}

/// Mirrors the live .did `BlockTag`.  We only ever SEND `Number(N)` (a
/// specific, already-final block number → consensus-safe).  The other tags are
/// mirrored for fidelity but never encoded.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum BlockTag {
    Earliest,
    Safe,
    Finalized,
    Latest,
    Number(candid::Nat),
    Pending,
}

/// `ConsensusStrategy` from the live .did.  Mirrored so `RpcConfig` is
/// well-formed; the single-provider probe never constructs one (passes
/// `responseConsensus = null`).
#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum ConsensusStrategy {
    Equality,
    Threshold {
        total: Option<u8>,
        min: u8,
    },
}

/// `RpcConfig` from the live .did.  The typed `eth_getBlockByNumber` takes
/// `opt RpcConfig`; we always pass `None` (a single provider needs no
/// consensus strategy), but the type must exist so the candid arg tuple
/// `(RpcServices, opt RpcConfig, BlockTag)` is unambiguous.
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct RpcConfig {
    #[serde(rename = "responseSizeEstimate")]
    pub response_size_estimate: Option<u64>,
    #[serde(rename = "responseConsensus")]
    pub response_consensus: Option<ConsensusStrategy>,
}

/// Mirrors the live .did `Block` record, but with **only the `number` field**.
///
/// Candid record-subtyping lets a reader declaring FEWER fields decode the
/// full wire record (the extra fields — `hash`, `timestamp`, `miner`, etc. —
/// are ignored).  The round-trip unit test in `tests_evm_rpc` proves this.
/// `number` is the only field the finalized-height probe reads.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub number: candid::Nat,
}

/// `GetBlockByNumberResult = variant { Ok : Block; Err : RpcError }`.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum GetBlockByNumberResult {
    Ok(Block),
    Err(RpcError),
}

/// `MultiGetBlockByNumberResult` from the live .did.  We only ever send ONE
/// `Custom` provider, so the canister always returns `Consistent`;
/// `Inconsistent` is never on the wire and never decoded in practice (it
/// references the singular `RpcService`, which is fine since we never decode
/// that arm).
///
/// NOTE (Phase 1b / M-1): `RpcService` above mirrors only the `Custom` variant.
/// If multi-provider support is ever introduced (e.g. a second `Custom` URL or
/// a built-in `EthMainnet`-style arm), the EVM RPC canister MAY return an
/// `Inconsistent` result containing a non-`Custom` `RpcService` variant, which
/// would TRAP on Candid decode because this enum lacks those arms. Acceptable
/// for Phase 1b (Monad is always addressed via a single `Custom` provider);
/// revisit by mirroring the full `RpcService` variant set if multi-provider is
/// introduced.
#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum MultiGetBlockByNumberResult {
    Consistent(GetBlockByNumberResult),
    Inconsistent(Vec<(RpcService, GetBlockByNumberResult)>),
}

// ─── Pure parsers ─────────────────────────────────────────────────────────────

/// Parse a 0x-prefixed hex string into a `u128`.
///
/// Strips the leading `"0x"` prefix (case-insensitive) and then interprets the
/// remainder as a big-endian hex integer.  Returns `Err` on malformed input.
pub fn parse_hex_quantity(s: &str) -> Result<u128, String> {
    let hex = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .ok_or_else(|| format!("missing 0x prefix: {:?}", s))?;
    u128::from_str_radix(hex, 16)
        .map_err(|e| format!("invalid hex quantity {:?}: {}", s, e))
}

// ─── BurnLog ─────────────────────────────────────────────────────────────────

/// Parsed representation of a `Burn(uint256 vault_id, address burner,
/// uint256 amount)` log emitted by the icUSD EVM contract on Monad.
///
/// ABI layout:
///   topics[0] = BURN_EVENT_TOPIC0
///   topics[1] = vault_id  (indexed, padded to 32 bytes)
///   topics[2] = burner    (indexed, EVM address padded to 32 bytes)
///   data      = amount    (uint256, 32 bytes)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BurnLog {
    pub vault_id: u64,
    pub amount_e8s: u128,
    pub tx_hash: String,
    pub block_number: u64,
}

impl BurnLog {
    /// Decode a `BurnLog` from raw log fields.
    ///
    /// `topics` must have at least 3 entries; `data` is the ABI-encoded amount.
    /// topic0 comparison is case-insensitive (RPC responses vary in
    /// EIP-55 mixed-case vs lowercase; our constant is lowercase).
    pub fn from_raw(
        topics: &[String],
        data: &str,
        tx_hash: &str,
        block_number: u64,
    ) -> Result<Self, String> {
        if topics.len() < 3 {
            return Err(format!(
                "BurnLog: expected >=3 topics, got {}",
                topics.len()
            ));
        }
        if !topics[0].eq_ignore_ascii_case(BURN_EVENT_TOPIC0) {
            return Err(format!(
                "BurnLog: wrong topic0: expected {} got {}",
                BURN_EVENT_TOPIC0, topics[0]
            ));
        }
        let vault_id_raw = parse_hex_quantity(&topics[1])?;
        if vault_id_raw > u64::MAX as u128 {
            return Err(format!("BurnLog: vault_id overflow: {}", vault_id_raw));
        }
        let amount_e8s = parse_hex_quantity(data)?;
        Ok(BurnLog {
            vault_id: vault_id_raw as u64,
            amount_e8s,
            tx_hash: tx_hash.to_string(),
            block_number,
        })
    }
}

/// Convenience free fn delegating to `BurnLog::from_raw`.
pub fn decode_burn_log(
    topics: &[String],
    data: &str,
    tx_hash: &str,
    block_number: u64,
) -> Result<BurnLog, String> {
    BurnLog::from_raw(topics, data, tx_hash, block_number)
}

// ─── MintLog ─────────────────────────────────────────────────────────────────

/// Parsed representation of a `Mint(uint256 vault_id, address recipient,
/// uint256 amount)` log.
///
/// ABI layout:
///   topics[0] = MINT_EVENT_TOPIC0
///   topics[1] = vault_id   (indexed)
///   topics[2] = recipient  (indexed, EVM address padded to 32 bytes)
///   data      = amount     (uint256)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MintLog {
    pub vault_id: u64,
    /// EVM address of the mint recipient (0x-prefixed, lowercase).
    pub recipient: String,
    pub amount_e8s: u128,
    pub tx_hash: String,
    pub block_number: u64,
}

impl MintLog {
    /// Decode a `MintLog` from raw log fields.
    pub fn from_raw(
        topics: &[String],
        data: &str,
        tx_hash: &str,
        block_number: u64,
    ) -> Result<Self, String> {
        if topics.len() < 3 {
            return Err(format!(
                "MintLog: expected >=3 topics, got {}",
                topics.len()
            ));
        }
        if !topics[0].eq_ignore_ascii_case(MINT_EVENT_TOPIC0) {
            return Err(format!(
                "MintLog: wrong topic0: expected {} got {}",
                MINT_EVENT_TOPIC0, topics[0]
            ));
        }
        let vault_id_raw = parse_hex_quantity(&topics[1])?;
        if vault_id_raw > u64::MAX as u128 {
            return Err(format!("MintLog: vault_id overflow: {}", vault_id_raw));
        }
        // Recipient address: last 20 bytes of the 32-byte padded topic2.
        // We store the full 32-byte topic as a 0x-prefixed hex string,
        // but callers can extract the address from the last 40 hex chars.
        let recipient = {
            let raw = topics[2]
                .strip_prefix("0x")
                .or_else(|| topics[2].strip_prefix("0X"))
                .unwrap_or(&topics[2]);
            // Ethereum addresses are 20 bytes (40 hex chars); topic is padded to 32 bytes (64 hex chars).
            let addr_hex = if raw.len() >= 40 {
                &raw[raw.len() - 40..]
            } else {
                raw
            };
            format!("0x{}", addr_hex.to_lowercase())
        };
        let amount_e8s = parse_hex_quantity(data)?;
        Ok(MintLog {
            vault_id: vault_id_raw as u64,
            recipient,
            amount_e8s,
            tx_hash: tx_hash.to_string(),
            block_number,
        })
    }
}

/// Convenience free fn delegating to `MintLog::from_raw`.
pub fn decode_mint_log(
    topics: &[String],
    data: &str,
    tx_hash: &str,
    block_number: u64,
) -> Result<MintLog, String> {
    MintLog::from_raw(topics, data, tx_hash, block_number)
}

// ─── Internal helpers ────────────────────────────────────────────────────────

/// Returns the EVM RPC canister principal.
///
/// Uses the developer-gated `evm_rpc_principal_override` from State when set
/// (enables PocketIC and staging to point at a mock canister).  Falls back to
/// the production canister on the IC app subnet.
fn evm_rpc_principal() -> Principal {
    read_state(|s| s.evm_rpc_override())
        .unwrap_or_else(|| {
            Principal::from_text("7hfb6-caaaa-aaaar-qadga-cai")
                .expect("static EVM RPC principal is valid")
        })
}

/// Send a raw JSON-RPC payload to the EVM RPC canister using the `request`
/// escape-hatch method.  Tries each of the chain's configured RPC endpoints in
/// order and returns the first successful `Ok` inner text.  On all-fail returns
/// the last error string.
async fn call_evm_rpc(chain: ChainId, json_payload: &str) -> Result<String, String> {
    let endpoints: Vec<String> = read_state(|s| {
        s.multi_chain
            .chain_configs
            .get(&chain)
            .map(|c| c.rpc_endpoints.clone())
            .unwrap_or_default()
    });

    if endpoints.is_empty() {
        return Err(format!("no RPC endpoints configured for chain {:?}", chain));
    }

    let canister = evm_rpc_principal();
    let mut last_err = String::new();

    for url in &endpoints {
        let rpc_service = RpcService::Custom(RpcApi {
            url: url.clone(),
            headers: None,
        });

        let result: Result<(RequestResult,), _> =
            ic_cdk::api::call::call_with_payment128(
                canister,
                "request",
                (rpc_service, json_payload.to_string(), EVM_RPC_MAX_RESPONSE_BYTES),
                EVM_RPC_CALL_CYCLES,
            )
            .await;

        match result {
            Ok((RequestResult::Ok(text),)) => {
                log!(DEBUG, "[evm_rpc] call ok via {}", url);
                return Ok(text);
            }
            Ok((RequestResult::Err(rpc_err),)) => {
                last_err = format!("RPC error from {}: {:?}", url, rpc_err);
                log!(DEBUG, "[evm_rpc] provider error via {}: {:?}", url, rpc_err);
            }
            Err((code, msg)) => {
                last_err = format!("call error to {} ({:?}): {}", url, code, msg);
                log!(DEBUG, "[evm_rpc] call error via {}: {:?} {}", url, code, msg);
            }
        }
    }

    Err(last_err)
}

// ─── JSON-RPC request ID counter ─────────────────────────────────────────────

use std::sync::atomic::{AtomicU64, Ordering};
static RPC_ID: AtomicU64 = AtomicU64::new(1);

fn next_rpc_id() -> u64 {
    RPC_ID.fetch_add(1, Ordering::Relaxed)
}

// ─── Public async interface ──────────────────────────────────────────────────

/// Consensus-safe probe: does finalized block `n` exist? Routes through the
/// TYPED eth_getBlockByNumber(Number(n)) (a specific number is byte-identical
/// across IC replicas, unlike a volatile block tag). Returns Ok(Some(number))
/// if present, Ok(None) if not yet reached (benign — caught up / future block),
/// Err only on a genuine infra failure (call error / Inconsistent).
///
/// A one-off Ok(None) is expected when the chain hasn't produced that block yet.
/// A PERSISTENT stream of Ok(None) in the INFO log (with an rpc_err in the
/// message) indicates a real provider or consensus problem and means the
/// burn-watch cursor is stalled — it will not advance until the probe starts
/// returning Ok(Some(...)). Investigate the RPC endpoint and the EVM RPC
/// canister's cycle balance if you see this repeating.
async fn eth_get_block_number_at(chain: ChainId, n: u64) -> Result<Option<u64>, String> {
    // Read the chain's configured endpoints (same source as `call_evm_rpc`).
    // The typed method needs a single Custom provider; use the first endpoint.
    let endpoints: Vec<String> = read_state(|s| {
        s.multi_chain
            .chain_configs
            .get(&chain)
            .map(|c| c.rpc_endpoints.clone())
            .unwrap_or_default()
    });
    let first = endpoints
        .first()
        .ok_or_else(|| format!("no RPC endpoints configured for chain {:?}", chain))?
        .clone();

    let rpc_services = RpcServices::Custom {
        chain_id: chain.0 as u64,
        services: vec![RpcApi {
            url: first,
            headers: None,
        }],
    };

    let canister = evm_rpc_principal();
    let result: Result<(MultiGetBlockByNumberResult,), _> =
        ic_cdk::api::call::call_with_payment128(
            canister,
            "eth_getBlockByNumber",
            (
                rpc_services,
                Option::<RpcConfig>::None,
                BlockTag::Number(candid::Nat::from(n)),
            ),
            EVM_RPC_CALL_CYCLES,
        )
        .await;

    match result {
        Ok((MultiGetBlockByNumberResult::Consistent(GetBlockByNumberResult::Ok(block)),)) => {
            // candid::Nat → u64; overflow is a hard error (a block number that
            // exceeds u64 is impossible in practice and would corrupt the cursor).
            let num: u64 = u64::try_from(block.number.0.clone())
                .map_err(|_| format!("block number {} overflows u64", block.number))?;
            Ok(Some(num))
        }
        Ok((MultiGetBlockByNumberResult::Consistent(GetBlockByNumberResult::Err(rpc_err)),)) => {
            // Block-not-found is the common benign case (chain hasn't reached
            // block n yet). HOWEVER a rate-limit, TooFewCycles, or IcError also
            // maps here — both collapse to Ok(None) so the cursor does not advance.
            // A single occurrence is harmless; a PERSISTENT stream means a real
            // provider / consensus problem and the cursor is stalled. See the
            // helper's doc comment for the monitoring note.
            log!(
                INFO,
                "[evm_rpc] eth_getBlockByNumber(Number({})) chain={:?} returned no block ({:?}); treating as not-yet-final (cursor will not advance this tick)",
                n,
                chain,
                rpc_err
            );
            Ok(None)
        }
        Ok((MultiGetBlockByNumberResult::Inconsistent(_),)) => {
            Err("unexpected inconsistent eth_getBlockByNumber result".to_string())
        }
        Err((code, msg)) => Err(format!(
            "eth_getBlockByNumber call error ({:?}): {}",
            code, msg
        )),
    }
}

/// Returns `(latest_block, finalized_block)` for the given chain.
///
/// Both elements are the same **consensus-safe** probed height. We do NOT read
/// the volatile chain head (`eth_blockNumber` or a `latest`/`finalized` tag):
/// on a fast-finality chain like Monad those differ across the EVM RPC
/// canister's subnet replicas, so the IC HTTPS-outcall consensus never agrees
/// and the call fails every tick. Instead we probe a SPECIFIC block number
/// `last_observed + MAX_BLOCK_SCAN_WINDOW` via the typed
/// `eth_getBlockByNumber(Number(N))` (a fixed, already-final number is
/// byte-identical across replicas):
///   - if that block exists & is final → advance the window up to it
///   - if not (chain hasn't reached it yet) → return the current cursor (no
///     new blocks this tick)
///
/// `last_observed == 0` means the burn-watch cursor is unseeded; the caller
/// (`run_observer`) skips burn-watch entirely in that case (no genesis crawl).
pub async fn fetch_block_numbers(chain: ChainId) -> Result<(u64, u64), String> {
    let last_observed = read_state(|s| {
        s.multi_chain
            .last_observed_block
            .get(&chain)
            .copied()
            .unwrap_or(0)
    });
    let candidate = last_observed.saturating_add(MAX_BLOCK_SCAN_WINDOW);
    match eth_get_block_number_at(chain, candidate).await? {
        Some(_) => Ok((candidate, candidate)), // block exists & final → advance window
        None => Ok((last_observed, last_observed)), // not advanced enough yet → nothing new
    }
}

/// Returns the ETH/native balance (in wei) of `address` at the latest block.
pub async fn get_balance(chain: ChainId, address: &str) -> Result<u128, String> {
    let payload = format!(
        r#"{{"jsonrpc":"2.0","method":"eth_getBalance","params":[{:?},"latest"],"id":{}}}"#,
        address,
        next_rpc_id()
    );
    let text = call_evm_rpc(chain, &payload).await?;
    let val: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("eth_getBalance parse: {}", e))?;
    let hex = val["result"]
        .as_str()
        .ok_or_else(|| format!("eth_getBalance: missing result in {:?}", text))?;
    parse_hex_quantity(hex)
}

/// Returns the transaction count (nonce) of `address` at the latest block.
pub async fn get_transaction_count(chain: ChainId, address: &str) -> Result<u64, String> {
    let payload = format!(
        r#"{{"jsonrpc":"2.0","method":"eth_getTransactionCount","params":[{:?},"latest"],"id":{}}}"#,
        address,
        next_rpc_id()
    );
    let text = call_evm_rpc(chain, &payload).await?;
    let val: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("eth_getTransactionCount parse: {}", e))?;
    let hex = val["result"]
        .as_str()
        .ok_or_else(|| format!("eth_getTransactionCount: missing result in {:?}", text))?;
    Ok(parse_hex_quantity(hex)? as u64)
}

/// Returns logs matching `(contract, topic0, fromBlock..toBlock)`.
///
/// Each entry in the returned `Vec` is
/// `(topics, data, txHash, blockNumber, logIndex)`.
///
/// `logIndex` is the EVM log index within the transaction — the canonical
/// on-chain identity of a log is `(tx_hash, log_index)`, not
/// `(tx_hash, vault_id, amount)`. Two identical Burn events emitted in the
/// same transaction (same tx_hash, same vault, same amount) have DIFFERENT
/// log indices and must both be credited.
///
/// If the JSON entry has no `logIndex` field (should not happen for real finalized
/// logs), the entry's position in the returned array is used as a stable fallback,
/// ensuring two distinct logs still get distinct indices.
///
/// The caller is responsible for decoding via `decode_burn_log` /
/// `decode_mint_log` etc.
///
/// ## Chunking (Gate-4 fix)
///
/// The Monad testnet RPC caps a single `eth_getLogs` at a 100-block range
/// (`toBlock - fromBlock` <= `MONAD_GETLOGS_MAX_RANGE`); a wider range returns
/// HTTP 413 / JSON-RPC -32614. This function pages `[from_block, to_block]` into
/// sequential sub-queries each within that cap and concatenates their results in
/// block order. Single-block scans (mint-confirm passes `from == to`) collapse to
/// exactly one sub-query, unchanged.
///
/// Error policy: a sub-query RPC error fails the WHOLE call (the `?` below
/// propagates it and no partial `Vec` is returned). This is deliberate — the
/// burn-watch loop must retry the full range and must NOT advance its cursor past
/// a range that was only partially scanned, or it would silently miss the burns
/// in the un-scanned chunks (a supply-accounting hole). Ordering and the
/// `(tx_hash, log_index)` dedup are unaffected: chunks cover disjoint, ascending
/// block ranges, so concatenating them yields the same flat, block-ordered Vec a
/// single wide query would have, and every log keeps its own block + log_index.
pub async fn get_logs(
    chain: ChainId,
    contract: &str,
    topic0: &str,
    from_block: u64,
    to_block: u64,
) -> Result<Vec<(Vec<String>, String, String, u64, u64)>, String> {
    // Defensive: an empty/inverted range has no logs (callers pass from <= to).
    if from_block > to_block {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let mut start = from_block;
    loop {
        // `to - from <= MONAD_GETLOGS_MAX_RANGE` per sub-query (the provider cap).
        let chunk_to = start.saturating_add(MONAD_GETLOGS_MAX_RANGE).min(to_block);
        let mut chunk = get_logs_single_range(chain, contract, topic0, start, chunk_to).await?;
        out.append(&mut chunk);
        if chunk_to >= to_block {
            break;
        }
        start = chunk_to + 1;
    }
    Ok(out)
}

/// Single `eth_getLogs` query over `[from_block, to_block]` (caller must keep the
/// range within the provider's `MONAD_GETLOGS_MAX_RANGE` cap — `get_logs` does
/// the chunking). Parses the JSON-RPC response into
/// `(topics, data, txHash, blockNumber, logIndex)` tuples.
async fn get_logs_single_range(
    chain: ChainId,
    contract: &str,
    topic0: &str,
    from_block: u64,
    to_block: u64,
) -> Result<Vec<(Vec<String>, String, String, u64, u64)>, String> {
    let payload = format!(
        r#"{{"jsonrpc":"2.0","method":"eth_getLogs","params":[{{"address":{:?},"topics":[{:?}],"fromBlock":"0x{:x}","toBlock":"0x{:x}"}}],"id":{}}}"#,
        contract,
        topic0,
        from_block,
        to_block,
        next_rpc_id()
    );
    let text = call_evm_rpc(chain, &payload).await?;
    let val: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("eth_getLogs parse: {}", e))?;

    if let Some(err) = val.get("error") {
        return Err(format!("eth_getLogs RPC error: {}", err));
    }

    let logs = val["result"]
        .as_array()
        .ok_or_else(|| format!("eth_getLogs: result is not an array in {:?}", text))?;

    let mut out = Vec::with_capacity(logs.len());
    for (position, entry) in logs.iter().enumerate() {
        let topics: Vec<String> = entry["topics"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| t.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let data = entry["data"].as_str().unwrap_or("0x").to_string();
        let tx_hash = entry["transactionHash"].as_str().unwrap_or("0x").to_string();
        let block_number_hex = entry["blockNumber"].as_str().unwrap_or("0x0");
        let block_number = parse_hex_quantity(block_number_hex)? as u64;
        // The log_index is the authoritative per-log identity within a tx.
        // Fall back to the array position if the field is absent (should not
        // happen for real finalized logs, but ensures distinct indices even in
        // that edge case).
        let log_index = match entry["logIndex"].as_str() {
            Some(hex) => parse_hex_quantity(hex)? as u64,
            None => position as u64,
        };
        out.push((topics, data, tx_hash, block_number, log_index));
    }
    Ok(out)
}

/// Returns `None` if the transaction is still pending, or `Some((success,
/// block_number))` once mined, where `success` is `true` iff status == 0x1.
pub async fn get_transaction_receipt(
    chain: ChainId,
    tx_hash: &str,
) -> Result<Option<(bool, u64)>, String> {
    let payload = format!(
        r#"{{"jsonrpc":"2.0","method":"eth_getTransactionReceipt","params":[{:?}],"id":{}}}"#,
        tx_hash,
        next_rpc_id()
    );
    let text = call_evm_rpc(chain, &payload).await?;
    let val: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("eth_getTransactionReceipt parse: {}", e))?;

    if val["result"].is_null() {
        return Ok(None); // transaction still pending
    }

    let status_hex = val["result"]["status"].as_str().unwrap_or("0x0");
    let success = parse_hex_quantity(status_hex)? == 1;
    let block_number_hex = val["result"]["blockNumber"]
        .as_str()
        .ok_or_else(|| format!("receipt missing blockNumber in {:?}", text))?;
    let block_number = parse_hex_quantity(block_number_hex)? as u64;
    Ok(Some((success, block_number)))
}

/// Broadcasts a signed raw transaction.  Returns the transaction hash on
/// success.
pub async fn send_raw_transaction(chain: ChainId, raw_tx_hex: &str) -> Result<String, String> {
    let payload = format!(
        r#"{{"jsonrpc":"2.0","method":"eth_sendRawTransaction","params":[{:?}],"id":{}}}"#,
        raw_tx_hex,
        next_rpc_id()
    );
    let text = call_evm_rpc(chain, &payload).await?;
    let val: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("eth_sendRawTransaction parse: {}", e))?;

    if let Some(err) = val.get("error") {
        return Err(format!("eth_sendRawTransaction RPC error: {}", err));
    }

    val["result"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| format!("eth_sendRawTransaction: missing result in {:?}", text))
}

/// Returns `(base_fee_wei, priority_fee_wei)` for gas estimation.
///
/// Unit: **wei** (not gwei).  Uses `eth_gasPrice` as a simple heuristic;
/// splits the result 90/10 into base_fee/priority_fee.  If `eth_gasPrice`
/// fails, returns a conservative default of (1_000_000_000 wei = 1 gwei,
/// 100_000_000 wei = 0.1 gwei) and logs the failure.
///
/// A more accurate implementation can switch to `eth_feeHistory` once
/// production gas data is available from Monad testnet.
pub async fn fetch_fees(chain: ChainId) -> Result<(u128, u128), String> {
    let payload = format!(
        r#"{{"jsonrpc":"2.0","method":"eth_gasPrice","params":[],"id":{}}}"#,
        next_rpc_id()
    );
    match call_evm_rpc(chain, &payload).await {
        Ok(text) => {
            let val: serde_json::Value = serde_json::from_str(&text)
                .map_err(|e| format!("eth_gasPrice parse: {}", e))?;
            let hex = val["result"]
                .as_str()
                .ok_or_else(|| format!("eth_gasPrice: missing result in {:?}", text))?;
            let gas_price = parse_hex_quantity(hex)?;
            // Simple split: 90% base, 10% priority tip.
            let base_fee = gas_price * 9 / 10;
            let priority_fee = gas_price - base_fee;
            Ok((base_fee, priority_fee))
        }
        Err(e) => {
            log!(
                DEBUG,
                "[evm_rpc] fetch_fees failed for chain {:?}: {}; using default",
                chain,
                e
            );
            // Conservative default: 1 gwei base + 0.1 gwei priority.
            Ok((1_000_000_000, 100_000_000))
        }
    }
}
