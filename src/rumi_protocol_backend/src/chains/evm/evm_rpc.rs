//! EVM RPC wrapper for Monad chain state reads and signed-tx broadcasting.
//!
//! Design: most reads call the EVM RPC canister (`7hfb6-caaaa-aaaar-qadga-cai`)
//! via the generic `request` escape-hatch method using `call_with_payment128`.
//! This avoids the `evm_rpc_client` crate (which requires ic-cdk 0.19,
//! incompatible with this project's ic-cdk 0.12 pin).  JSON-RPC responses are
//! parsed with `serde_json` (already a workspace dep).
//!
//! Exception (finalized-height reads, `fetch_block_numbers` /
//! `eth_get_block_number_at`): these still probe a SPECIFIC, already-final
//! block number rather than a volatile tag (a volatile read, `eth_blockNumber`
//! or any `latest`/`finalized` tag, differs across the EVM RPC canister's
//! subnet replicas on a fast-finality chain, so IC HTTPS-outcall consensus
//! never agrees and the call fails every tick). A fixed block number is
//! byte-identical across replicas, so it reaches consensus.
//!
//! That probe is issued as a RAW `eth_getBlockTransactionCountByNumber`
//! request through `call_evm_rpc` (the same per-provider raw-request quorum
//! every other read in this file uses), NOT the TYPED
//! `eth_getBlockByNumber` candid method (mirrored below, still kept for wire
//! fidelity but currently unused). Incident 2026-08-21: the typed method's
//! OWN internal 2-of-3 threshold consensus has a materially higher cost model
//! than a raw scalar read. A live cost query for the exact 3 configured
//! Conflux providers at the probe's target block, 2-of-3 threshold,
//! `responseSizeEstimate=null`, measured 3,714,459,200 cycles, well above the
//! `EVM_RPC_CALL_CYCLES` (2B) attached to the typed call; that gap is enough
//! to establish underfunding as the cause. The live canary's own `Inconsistent`
//! response collapsed the per-provider results without exposing their
//! individual `RpcError` variants, so this cost-query comparison, not a
//! directly-observed `TooFewCycles` on every provider, is the evidence.
//! `eth_getBlockTransactionCountByNumber` answers the same "does block N
//! exist" question with a tiny scalar (a tx count, e.g. `"0x0"` for an empty
//! block, `null` for a block not yet produced, both confirmed live against
//! all 3 Conflux providers) at the existing per-provider `EVM_RPC_CALL_CYCLES`
//! cost, so it reuses the already-audited `call_evm_rpc` quorum instead of
//! needing its own cycles budget.
//!
//! Cycle cost: `EVM_RPC_CALL_CYCLES` (2_000_000_000) per PROVIDER per raw
//! `request` call.  This is intentionally generous (the actual HTTPS-outcall
//! cost depends on response size and subnet node count but is typically in
//! the hundreds-of-millions range for the small scalar/JSON responses every
//! call in this file reads).  The constant is marked tunable; a
//! developer-gated setter can be added once we have production measurements.
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
use crate::chains::config::{effective_min_quorum_providers, ChainId};
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

/// Per-chain max eth_getLogs range. Reads the compile-time EvmChainConfig;
/// falls back to the conservative Monad cap (100) for unknown chains.
pub(crate) fn getlogs_max_range_for(chain: ChainId) -> u64 {
    crate::chains::evm::evm_chain_config(chain)
        .map(|c| c.getlogs_max_range)
        .unwrap_or(MONAD_GETLOGS_MAX_RANGE)
}

// ─── Topic0 constants ────────────────────────────────────────────────────────

/// keccak256("Burn(uint256,address,uint256)")
/// Computed via pycryptodome Keccak-256 (verified against the Transfer
/// event well-known hash to confirm the implementation is correct).
pub const BURN_EVENT_TOPIC0: &str =
    "0xe1b6e34006e9871307436c226f232f9c5e7690c1d2c4f4adda4f607a75a9beca";

/// keccak256("Mint(uint256,address,uint256)")
pub const MINT_EVENT_TOPIC0: &str =
    "0x4e3883c75cc9c752bb1db2e406a822e4a75067ae77ad9a0a4d179f2709b9e1f6";

/// keccak256("Transfer(address,address,uint256)") — the canonical ERC-20
/// Transfer event topic0. Used by the liquidation-swap confirm to read the
/// REALIZED settle-stable output (the `Transfer(_, reserve, amount)` log).
pub const TRANSFER_EVENT_TOPIC0: &str =
    "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";

/// `getReserves()[:4]` selector (UniswapV2 pair).
pub const GET_RESERVES_SELECTOR: &str = "0x0902f1ac";
/// `token0()[:4]` selector (UniswapV2 pair).
pub const TOKEN0_SELECTOR: &str = "0x0dfe1681";
/// `getPair(address,address)[:4]` selector (UniswapV2 factory).
pub const GET_PAIR_SELECTOR: &str = "0xe6a43905";
/// `balanceOf(address)[:4]` selector (ERC-20).
pub const BALANCE_OF_SELECTOR: &str = "0x70a08231";

/// `keccak256("totalSupply()")[:4]`, the ERC-20 `totalSupply()` selector.
/// `IcUSD.sol` uses 8 decimals, so the returned value is e8s (1:1 with the
/// ICP-side `chain_supplies` accounting, no scaling needed).
pub const TOTAL_SUPPLY_SELECTOR: &str = "0x18160ddd";

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

/// `RpcService` — we only ever ENCODE `Custom` (Monad is always a custom
/// provider), but the multi-provider `eth_getBlockByNumber` (M-06) can return an
/// `Inconsistent` result whose per-provider vector is keyed by `RpcService`. To
/// avoid a Candid decode TRAP (the FLAG-9 caveat) we mirror the FULL variant set
/// from the live .did. The built-in provider arms (which we never send, so never
/// expect on the wire, but might appear if the canister echoes a default
/// provider) carry `candid::Reserved` payloads — they decode without inspecting
/// their contents, exactly the idiom the Solana `sol_rpc` mirror uses. Only the
/// `Custom` arm carries a real `RpcApi` (the one we ever read back).
#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum RpcService {
    Provider(u64),
    Custom(RpcApi),
    EthMainnet(candid::Reserved),
    EthSepolia(candid::Reserved),
    ArbitrumOne(candid::Reserved),
    BaseMainnet(candid::Reserved),
    OptimismMainnet(candid::Reserved),
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

/// `MultiGetBlockByNumberResult` from the live .did.
///
/// M-06 (QUORUM-3): the finalized-height probe now passes ALL distinct
/// configured providers (not just the first), so on disagreement the canister
/// returns `Inconsistent` carrying a per-provider vector keyed by `RpcService`.
/// We decode and TALLY that vector by distinct provider rather than trapping.
/// The FLAG-9 caveat (a non-`Custom` provider variant in that vector would
/// previously trap the decode) is handled by mirroring the full `RpcService`
/// variant set above (built-in arms carry `candid::Reserved`).
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

/// Decode an `eth_call` result word (a 0x-prefixed 32-byte ABI uint) into a
/// `u128`. Rejects an empty `"0x"` (a revert/empty return, which must NOT be
/// read as 0, or the supply gate would wrongly believe supply dropped to zero)
/// and any value exceeding `u128::MAX` (icUSD supply cannot approach that, so a
/// larger value signals a malformed response, fail loudly rather than wrap).
pub fn parse_eth_call_u128(result_hex: &str) -> Result<u128, String> {
    let hex = result_hex
        .strip_prefix("0x")
        .or_else(|| result_hex.strip_prefix("0X"))
        .ok_or_else(|| format!("eth_call result missing 0x prefix: {:?}", result_hex))?;
    if hex.is_empty() {
        return Err(format!("eth_call returned empty result {:?} (revert/empty)", result_hex));
    }
    u128::from_str_radix(hex, 16)
        .map_err(|e| format!("eth_call result {:?} not a u128: {}", result_hex, e))
}

/// Decode an `eth_call` result word containing an ABI address into a normalized
/// lowercase `0x` EVM address. Used for UniswapV2 factory `getPair`.
pub fn parse_eth_call_address(result_hex: &str) -> Result<String, String> {
    let hex = result_hex
        .strip_prefix("0x")
        .or_else(|| result_hex.strip_prefix("0X"))
        .ok_or_else(|| format!("eth_call address result missing 0x prefix: {:?}", result_hex))?;
    if hex.len() != 64 {
        return Err(format!("eth_call address result must be one ABI word: {:?}", result_hex));
    }
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("eth_call address result not hex: {:?}", result_hex));
    }
    let word = hex;
    if !word[..24].bytes().all(|b| b == b'0') {
        return Err(format!("eth_call address result has non-zero padding: {:?}", result_hex));
    }
    let address = &word[24..64];
    if address.bytes().all(|b| b == b'0') {
        return Err(format!("eth_call address result is zero address: {:?}", result_hex));
    }
    Ok(format!("0x{}", address.to_lowercase()))
}

fn abi_word_address_hex(addr: &str) -> Result<String, String> {
    let hex = addr.strip_prefix("0x").or_else(|| addr.strip_prefix("0X")).unwrap_or(addr);
    if hex.len() != 40 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("invalid EVM address {:?}", addr));
    }
    Ok(format!("{:0>64}", hex.to_lowercase()))
}

/// ABI-encode UniswapV2 factory `getPair(address,address)`.
pub fn encode_get_pair_calldata(token_a: &str, token_b: &str) -> Result<String, String> {
    Ok(format!(
        "{}{}{}",
        GET_PAIR_SELECTOR,
        abi_word_address_hex(token_a)?,
        abi_word_address_hex(token_b)?
    ))
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

/// Parsed Burn log including the indexed `burner` address. The legacy
/// `BurnLog` shape is kept because existing call sites only need the amount.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BurnLogWithBurner {
    pub vault_id: u64,
    pub burner: String,
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

/// Decode a `Burn` event and return the indexed burner address as a normalized
/// 0x-prefixed lowercase EVM address.
pub fn decode_burn_log_with_burner(
    topics: &[String],
    data: &str,
    tx_hash: &str,
    block_number: u64,
) -> Result<BurnLogWithBurner, String> {
    let burn = BurnLog::from_raw(topics, data, tx_hash, block_number)?;
    let raw = topics
        .get(2)
        .ok_or_else(|| "BurnLogWithBurner: missing burner topic".to_string())?;
    let hex = raw
        .strip_prefix("0x")
        .or_else(|| raw.strip_prefix("0X"))
        .unwrap_or(raw);
    if hex.len() < 40 {
        return Err(format!("BurnLogWithBurner: burner topic too short: {raw}"));
    }
    Ok(BurnLogWithBurner {
        vault_id: burn.vault_id,
        burner: format!("0x{}", hex[hex.len() - 40..].to_ascii_lowercase()),
        amount_e8s: burn.amount_e8s,
        tx_hash: burn.tx_hash,
        block_number: burn.block_number,
    })
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

/// A decoded ERC-20 `Transfer(from, to, amount)` log. The liquidation-swap
/// confirm reads the `Transfer(_, reserve_recipient, amount)` log to learn the
/// REALIZED settle-stable output (spec §4.8 — never trust min-out as the actual
/// amount). `to` and `amount` are the indexed recipient + the data value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferLog {
    /// Recipient address (0x-prefixed, lowercase) from the indexed `to` topic.
    pub to: String,
    /// Transferred amount (native base units of the token).
    pub amount: u128,
}

impl TransferLog {
    /// Decode from `[Transfer topic0, from(indexed), to(indexed)]` + `data=amount`.
    pub fn from_raw(topics: &[String], data: &str) -> Result<Self, String> {
        if topics.len() < 3 {
            return Err(format!("TransferLog: expected >=3 topics, got {}", topics.len()));
        }
        if !topics[0].eq_ignore_ascii_case(TRANSFER_EVENT_TOPIC0) {
            return Err(format!("TransferLog: wrong topic0: {}", topics[0]));
        }
        let to = {
            let raw = topics[2]
                .strip_prefix("0x")
                .or_else(|| topics[2].strip_prefix("0X"))
                .unwrap_or(&topics[2]);
            let addr_hex = if raw.len() >= 40 { &raw[raw.len() - 40..] } else { raw };
            format!("0x{}", addr_hex.to_lowercase())
        };
        let amount = parse_hex_quantity(data)?;
        Ok(TransferLog { to, amount })
    }
}

/// Split a UniswapV2 `getReserves()` return `(uint112 reserve0, uint112
/// reserve1, uint32)` into `(reserve0, reserve1)`. Each uint112 occupies a
/// 32-byte ABI word, but only the low 28 hex chars (112 bits) are significant —
/// we parse the full words to be safe (the high bits are zero for a uint112).
pub fn parse_two_uint112(result_hex: &str) -> Result<(u128, u128), String> {
    let hex = result_hex
        .strip_prefix("0x")
        .or_else(|| result_hex.strip_prefix("0X"))
        .ok_or_else(|| format!("getReserves result missing 0x prefix: {:?}", result_hex))?;
    // Two full 32-byte words for the two uint112 reserves = 128 hex chars.
    if hex.len() < 128 {
        return Err(format!("getReserves result too short: {:?}", result_hex));
    }
    let r0 = u128::from_str_radix(&hex[0..64], 16)
        .map_err(|e| format!("getReserves reserve0 parse: {}", e))?;
    let r1 = u128::from_str_radix(&hex[64..128], 16)
        .map_err(|e| format!("getReserves reserve1 parse: {}", e))?;
    Ok((r0, r1))
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

/// The production EVM RPC canister on the IC app subnet (candid:service
/// `7hfb6-caaaa-aaaar-qadga-cai`, per the module doc comment at the top of
/// this file).
pub const DEFAULT_EVM_RPC_PRINCIPAL_TEXT: &str = "7hfb6-caaaa-aaaar-qadga-cai";

/// Parses `DEFAULT_EVM_RPC_PRINCIPAL_TEXT`. Split out from `evm_rpc_principal`
/// so `get_evm_rpc_principal` (main.rs) can report the default independently
/// of whether an override is currently set.
pub fn default_evm_rpc_principal() -> Principal {
    Principal::from_text(DEFAULT_EVM_RPC_PRINCIPAL_TEXT).expect("static EVM RPC principal is valid")
}

/// Returns the EVM RPC canister principal.
///
/// Uses the developer-gated `evm_rpc_principal_override` from State when set
/// (enables PocketIC and staging to point at a mock canister).  Falls back to
/// the production canister on the IC app subnet.
pub fn evm_rpc_principal() -> Principal {
    read_state(|s| s.evm_rpc_override()).unwrap_or_else(default_evm_rpc_principal)
}

/// Issue ONE JSON-RPC `request` to a single endpoint. Returns the inner response
/// text on success, or an error string (provider error or IC call error).
async fn single_call(canister: Principal, url: &str, json_payload: &str) -> Result<String, String> {
    let rpc_service = RpcService::Custom(RpcApi {
        url: url.to_string(),
        headers: None,
    });
    let result: Result<(RequestResult,), _> = ic_cdk::api::call::call_with_payment128(
        canister,
        "request",
        (rpc_service, json_payload.to_string(), EVM_RPC_MAX_RESPONSE_BYTES),
        EVM_RPC_CALL_CYCLES,
    )
    .await;
    match result {
        Ok((RequestResult::Ok(text),)) => Ok(text),
        Ok((RequestResult::Err(rpc_err),)) => Err(format!("RPC error from {}: {:?}", url, rpc_err)),
        Err((code, msg)) => Err(format!("call error to {} ({:?}): {}", url, code, msg)),
    }
}

/// The consensus key of a JSON-RPC response: its semantic `result` value, or its
/// `error` value if there is no result. `serde_json::Value` equality is
/// whitespace- and key-order-independent, so two providers that returned the
/// same logical result agree even when their JSON formatting (or the volatile
/// `id` field) differs. An unparseable / shape-less response groups only with
/// itself, so it can never be mistaken for agreement.
fn response_consensus_key(text: &str) -> serde_json::Value {
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(v) => {
            if let Some(r) = v.get("result") {
                r.clone()
            } else if let Some(e) = v.get("error") {
                serde_json::json!({ "__error": e })
            } else {
                serde_json::json!({ "__raw": text })
            }
        }
        Err(_) => serde_json::json!({ "__raw": text }),
    }
}

/// Read a chain's DISTINCT configured RPC endpoints and its effective
/// quorum-provider floor (audit M-04/M-05). The endpoints are already deduped at
/// registration/config time (`chains::admin::dedup_endpoints`); we dedup again
/// here defensively so a hand-poked state cannot smuggle a repeat past the
/// distinct-provider tally. Returns `(distinct_endpoints, floor)`.
fn endpoints_and_floor(chain: ChainId) -> (Vec<String>, u32) {
    read_state(|s| {
        s.multi_chain
            .chain_configs
            .get(&chain)
            .map(|c| {
                let mut seen = std::collections::BTreeSet::new();
                let distinct: Vec<String> = c
                    .rpc_endpoints
                    .iter()
                    .filter(|u| seen.insert((*u).clone()))
                    .cloned()
                    .collect();
                (distinct, effective_min_quorum_providers(c))
            })
            .unwrap_or_else(|| (Vec::new(), crate::chains::config::DEFAULT_MIN_QUORUM_PROVIDERS))
    })
}

/// Send a raw JSON-RPC READ to the EVM RPC canister with MULTI-PROVIDER QUORUM
/// (audit FLAG-1 + M-04/M-05). Queries EVERY DISTINCT configured endpoint and
/// requires at least the chain's quorum-provider FLOOR of DISTINCT providers to
/// return the same semantic result before returning it, so a single malicious /
/// compromised / lagging provider cannot fabricate a balance, block, supply, or
/// receipt the canister credits.
///
/// FAIL-CLOSED FLOOR (M-05, QUORUM-2): if the chain has FEWER distinct configured
/// endpoints than its floor (`DEFAULT_MIN_QUORUM_PROVIDERS` = 3 unless
/// overridden), the read is REFUSED outright — no single- or sub-floor-provider
/// financial read is ever credited. A deployment MUST configure >= floor
/// independent endpoints (an ops action) before the observer / settlement
/// workers can read. This is the runtime gate that makes the quorum non-dormant.
///
/// DISTINCT-PROVIDER TALLY (M-04, QUORUM-1): agreement is counted by DISTINCT
/// provider URL, never by list slot or raw response count, so a duplicated
/// endpoint (already deduped at config time, deduped again here) can never vote
/// twice toward the quorum. The winning value must be returned by
/// `max(floor, strict_majority_of_distinct)` distinct providers.
///
/// On below-floor, no-quorum, or all-fail this returns Err so the caller never
/// credits / advances on disagreement.
///
/// READS ONLY. `eth_sendRawTransaction` is a write and uses
/// `call_evm_rpc_broadcast` (first-Ok: a broadcast lands if ANY provider accepts
/// it; requiring agreement would wrongly drop a tx that only some providers saw).
///
/// This is a thin wrapper over `call_evm_rpc_detailed` that collapses the
/// structured `QuorumError` into one diagnostic string, for callers that only
/// need pass/fail. `eth_get_block_number_at` calls `call_evm_rpc_detailed`
/// directly so it can react differently to "no provider responded at all"
/// (an infrastructure failure) versus "the providers that DID respond
/// disagreed".
async fn call_evm_rpc(chain: ChainId, json_payload: &str) -> Result<String, String> {
    call_evm_rpc_detailed(chain, json_payload)
        .await
        .map_err(|e| e.to_string())
}

/// Structured failure from `call_evm_rpc_detailed`. Distinguishes a
/// configuration problem and an infrastructure failure (every configured
/// provider errored, e.g. every provider returned `TooFewCycles`, so there is
/// nothing to tally at all) from a genuine disagreement (at least one
/// provider responded, but distinct-provider agreement never reached the
/// required floor/majority). Both fail closed identically (never credit /
/// never advance), but an operator investigating a stalled cursor needs to
/// know which one happened: an infrastructure failure is actionable
/// (fix cycles, fix a provider), a disagreement may just mean "try next tick".
#[derive(Debug, Clone, PartialEq, Eq)]
enum QuorumError {
    /// No endpoints configured, or fewer distinct endpoints than the chain's
    /// quorum floor. Caught before any network call is made.
    Configuration(String),
    /// Every configured provider's `request` call errored (network failure,
    /// `TooFewCycles`, or any other per-provider error). No response was
    /// available to tally.
    AllProvidersFailed(String),
    /// At least one provider responded, but distinct-provider agreement never
    /// reached the chain's required floor/majority.
    Disagreement(String),
}

impl std::fmt::Display for QuorumError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QuorumError::Configuration(d)
            | QuorumError::AllProvidersFailed(d)
            | QuorumError::Disagreement(d) => write!(f, "{}", d),
        }
    }
}

/// Same multi-provider quorum read as `call_evm_rpc`, but returns the
/// STRUCTURED `QuorumError` (see its doc comment) instead of a flat string.
async fn call_evm_rpc_detailed(chain: ChainId, json_payload: &str) -> Result<String, QuorumError> {
    let (endpoints, floor) = endpoints_and_floor(chain);
    if endpoints.is_empty() {
        return Err(QuorumError::Configuration(format!(
            "no RPC endpoints configured for chain {:?}",
            chain
        )));
    }
    // M-05 (QUORUM-2): fail closed below the distinct-provider floor. With fewer
    // than `floor` distinct providers there is no way to reach the required
    // cross-provider agreement, so a financial read must never be credited.
    if (endpoints.len() as u32) < floor {
        return Err(QuorumError::Configuration(format!(
            "RPC quorum floor not met for chain {:?}: {} distinct provider(s) configured, need >= {} (configure more endpoints; financial reads fail closed below the floor)",
            chain, endpoints.len(), floor
        )));
    }
    let canister = evm_rpc_principal();

    // Collect EVERY provider's outcome (not just the last error), so a
    // disagreement or all-fail diagnosis can name every provider by URL.
    let mut outcomes: Vec<(String, Result<String, String>)> = Vec::new();
    for url in &endpoints {
        let outcome = single_call(canister, url, json_payload).await;
        if let Err(ref e) = outcome {
            log!(DEBUG, "[evm_rpc] provider read error via {}: {}", url, e);
        }
        outcomes.push((url.clone(), outcome));
    }
    tally_provider_outcomes(chain, &outcomes, floor)
}

/// Pure multi-provider quorum tally (audit M-04/M-05/M-06 lineage), split out
/// from the async network loop above so the decision logic can be exercised
/// with synthetic provider outcomes in unit tests, independent of any live
/// inter-canister call. `outcomes` is `(provider_url, single_call result)` for
/// every distinct configured endpoint, in the same order `call_evm_rpc_detailed`
/// queried them; `floor` is the chain's minimum quorum-provider floor.
fn tally_provider_outcomes(
    chain: ChainId,
    outcomes: &[(String, Result<String, String>)],
    floor: u32,
) -> Result<String, QuorumError> {
    let total = outcomes.len();

    // Collect every Ok response PAIRED with its provider URL, so the tally counts
    // DISTINCT providers (M-04), not list slots or raw response multiplicity.
    let oks: Vec<(&str, &str)> = outcomes
        .iter()
        .filter_map(|(url, r)| r.as_ref().ok().map(|text| (url.as_str(), text.as_str())))
        .collect();
    let errs: Vec<(&str, &str)> = outcomes
        .iter()
        .filter_map(|(url, r)| r.as_ref().err().map(|e| (url.as_str(), e.as_str())))
        .collect();

    if oks.is_empty() {
        let detail = errs
            .iter()
            .map(|(url, e)| format!("{} -> {}", url, e))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(QuorumError::AllProvidersFailed(format!(
            "infrastructure failure: all {} configured provider(s) for chain {:?} errored (nothing to tally, this is not a genuine on-chain disagreement): {}",
            total, chain, detail
        )));
    }

    // Group by semantic consensus key; for each key count the number of DISTINCT
    // provider URLs that returned it (endpoints are deduped, so each URL appears
    // at most once in `oks`, but we count distinctly to be explicit and robust).
    let keyed: Vec<(&str, serde_json::Value)> = oks
        .iter()
        .map(|(url, text)| (*url, response_consensus_key(text)))
        .collect();
    let mut best_idx = 0usize;
    let mut best_count = 0usize;
    for i in 0..keyed.len() {
        let mut providers_for_key = std::collections::BTreeSet::new();
        for (url, key) in &keyed {
            if *key == keyed[i].1 {
                providers_for_key.insert(*url);
            }
        }
        let count = providers_for_key.len();
        if count > best_count {
            best_count = count;
            best_idx = i;
        }
    }

    // Required distinct agreement: at least the floor AND a strict majority of the
    // DISTINCT configured providers (the floor is the audit's primary guard; the
    // majority preserves the original FLAG-1 protection for larger provider sets).
    let majority = total / 2 + 1;
    let needed = (floor as usize).max(majority);
    if best_count >= needed {
        // Return the winning provider's response text (the consensus value).
        Ok(oks[best_idx].1.to_string())
    } else {
        // Genuine disagreement: show WHAT the providers that did respond
        // disagreed about, grouped by distinct value, plus any provider that
        // errored outright (it did not vote either way).
        let mut groups: Vec<(serde_json::Value, Vec<&str>)> = Vec::new();
        for (url, key) in &keyed {
            match groups.iter_mut().find(|(k, _)| k == key) {
                Some(g) => g.1.push(*url),
                None => groups.push((key.clone(), vec![*url])),
            }
        }
        let groups_detail = groups
            .iter()
            .map(|(key, urls)| format!("{} from [{}]", key, urls.join(", ")))
            .collect::<Vec<_>>()
            .join("; ");
        let errs_detail = if errs.is_empty() {
            String::new()
        } else {
            format!(
                "; provider errors: {}",
                errs.iter()
                    .map(|(url, e)| format!("{} -> {}", url, e))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        Err(QuorumError::Disagreement(format!(
            "RPC quorum not reached for chain {:?}: best distinct-provider agreement {}/{} (need {}); response groups: {}{}",
            chain, best_count, total, needed, groups_detail, errs_detail
        )))
    }
}

/// Broadcast a raw JSON-RPC WRITE (`eth_sendRawTransaction`). Returns the first
/// provider's Ok — a broadcast propagates if ANY provider accepts it, so quorum
/// is the wrong model for a write. On all-fail returns the last error.
async fn call_evm_rpc_broadcast(chain: ChainId, json_payload: &str) -> Result<String, String> {
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
        match single_call(canister, url, json_payload).await {
            Ok(text) => return Ok(text),
            Err(e) => last_err = e,
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

/// Consensus-safe probe: does finalized block `n` exist? Issues a RAW
/// `eth_getBlockTransactionCountByNumber(0x{n})` request through
/// `call_evm_rpc_detailed`, i.e. the SAME per-provider raw-request quorum
/// every other read in this file uses (a specific number is byte-identical
/// across IC replicas, unlike a volatile block tag). Returns Ok(Some(number))
/// if present, Ok(None) ONLY for the benign "not yet produced" case (an
/// explicit `result: null`) or a quorum disagreement among providers that DID
/// respond. Every other outcome is Err: every provider erroring outright, a
/// quorum-agreed JSON-RPC error, a missing `result` key, a `result` that is
/// not a valid Ethereum hex quantity, a malformed response carrying both
/// `result` and `error`, or an unparseable body. See `parse_block_probe_response`
/// for the exact decision tree.
///
/// Why a tx-count read instead of the TYPED `eth_getBlockByNumber` candid
/// method (still mirrored below for wire fidelity, but no longer called from
/// here): incident 2026-08-21 found the typed method's own internal 2-of-3
/// threshold consensus costs materially more than `EVM_RPC_CALL_CYCLES`. A
/// live cost query for the exact 3 configured providers measured 3,714,459,200
/// cycles against the 2B attached, establishing underfunding as the cause; the
/// live canary's `Inconsistent` response collapsed the per-provider results
/// without exposing their individual `RpcError` variants, so this is a
/// cost-comparison finding, not a directly-observed `TooFewCycles` on every
/// provider. A raw scalar read answers the same existence question at the
/// SAME per-provider cost every other call in this file already pays, with no
/// per-call cycles budget of its own to get wrong. `"0x0"` (a valid tx count
/// for an empty block) and JSON `null` (a block not yet produced) are both
/// confirmed live against all 3 Conflux providers.
///
/// A one-off Ok(None) is expected when the chain hasn't produced that block
/// yet. A PERSISTENT stream of Ok(None) in the INFO log indicates a real
/// provider or consensus problem and means the burn-watch cursor is stalled;
/// it will not advance until the probe starts returning Ok(Some(...)).
/// Investigate the RPC endpoints and the EVM RPC canister's cycle balance if
/// you see this repeating, and check whether the log line says "quorum
/// disagreement" (providers responded but disagreed) versus an `Err` return
/// (every provider failed outright, see `QuorumError::AllProvidersFailed`).
async fn eth_get_block_number_at(chain: ChainId, n: u64) -> Result<Option<u64>, String> {
    let payload = format!(
        r#"{{"jsonrpc":"2.0","method":"eth_getBlockTransactionCountByNumber","params":["0x{:x}"],"id":{}}}"#,
        n,
        next_rpc_id()
    );
    let quorum_result = call_evm_rpc_detailed(chain, &payload).await;
    block_probe_outcome(chain, n, quorum_result)
}

/// Pure decision logic for `eth_get_block_number_at`, split out from the
/// async network call so the full branch matrix (quorum success,
/// infrastructure failure, disagreement, malformed payload, benign
/// not-yet-produced) can be unit tested with synthetic `QuorumError` /
/// response-text fixtures, without any PocketIC instance or live network
/// call.
fn block_probe_outcome(
    chain: ChainId,
    n: u64,
    quorum_result: Result<String, QuorumError>,
) -> Result<Option<u64>, String> {
    match quorum_result {
        Ok(text) => parse_block_probe_response(chain, n, &text),
        // Configuration and all-providers-failed are both genuine
        // infrastructure problems (nothing to tally, or nothing configured):
        // propagate as Err so the caller (`is_block_final` / `fetch_block_numbers`)
        // logs it distinctly and retries next tick. Fail-closed either way; the
        // cursor is never advanced on this path.
        Err(QuorumError::Configuration(detail)) | Err(QuorumError::AllProvidersFailed(detail)) => {
            Err(format!(
                "eth_getBlockTransactionCountByNumber(0x{:x}) chain={:?}: {}",
                n, chain, detail
            ))
        }
        // Providers that DID respond disagree. Fail-closed (Ok(None)): a block
        // whose existence we cannot agree on must never advance the finalized
        // cursor. This is the "genuine disagreement" case, distinct from an
        // infrastructure failure, logged here rather than propagated as Err
        // since a one-off disagreement is not by itself actionable.
        Err(QuorumError::Disagreement(detail)) => {
            log!(
                INFO,
                "[evm_rpc] eth_getBlockTransactionCountByNumber(0x{:x}) chain={:?} quorum disagreement: {}; treating as not-yet-final (cursor will not advance this tick)",
                n, chain, detail
            );
            Ok(None)
        }
    }
}

/// Parse the quorum-agreed JSON-RPC response text for the block-existence
/// probe.
///
/// A well-formed JSON-RPC response carries EXACTLY ONE of `result`/`error`.
/// `result` present as a valid hex quantity string (even `"0x0"`, a valid tx
/// count for an empty block) means block `n` exists; `result: null` is the
/// documented not-yet-produced signal (benign, confirmed live against all 3
/// Conflux providers). A quorum-agreed `error`, a missing `result` key
/// entirely (distinct from an explicit `null`), a `result` that is not a
/// valid Ethereum hex quantity, a response carrying BOTH `result` and
/// `error`, or an unparseable body are all genuine problems: every one of
/// them returns `Err` so the caller surfaces it loudly (and, via
/// `block_probe_outcome`, never advances the cursor) rather than silently
/// collapsing into the benign not-yet-produced case.
fn parse_block_probe_response(chain: ChainId, n: u64, text: &str) -> Result<Option<u64>, String> {
    let val: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            return Err(format!(
                "eth_getBlockTransactionCountByNumber(0x{:x}) chain={:?}: malformed JSON-RPC response: {} (raw={:?})",
                n, chain, e, text
            ))
        }
    };

    // `.get(...)` (unlike `val["..."]`) distinguishes a MISSING key from an
    // explicit `null` value; a bare `.is_null()` check on the indexed value
    // cannot tell those apart, since serde_json's `Index` returns `Value::Null`
    // for both a missing key and a key whose value literally is `null`.
    let result_field = val.get("result");
    let error_field = val.get("error").filter(|e| !e.is_null());

    if error_field.is_some() && result_field.is_some_and(|r| !r.is_null()) {
        // Malformed: a well-formed response never carries both. Fail closed
        // loudly rather than guessing which field to trust.
        return Err(format!(
            "eth_getBlockTransactionCountByNumber(0x{:x}) chain={:?}: malformed JSON-RPC response carries BOTH result and error: {:?}",
            n, chain, text
        ));
    }

    if let Some(err) = error_field {
        // A JSON-RPC-level error that nonetheless reached quorum (enough
        // providers agreed on the SAME error) is a genuine RPC problem, not
        // the benign not-yet-produced case (which this method signals with
        // `result: null`, per the live Conflux probe evidence). Fail closed
        // AND actionable: return Err so the caller propagates it distinctly
        // rather than silently collapsing it into "not yet produced".
        return Err(format!(
            "eth_getBlockTransactionCountByNumber(0x{:x}) chain={:?}: providers agreed on a JSON-RPC error: {}",
            n, chain, err
        ));
    }

    match result_field {
        None => Err(format!(
            "eth_getBlockTransactionCountByNumber(0x{:x}) chain={:?}: malformed JSON-RPC response missing both result and error: {:?}",
            n, chain, text
        )),
        Some(v) if v.is_null() => Ok(None), // benign: chain has not produced this block yet
        Some(v) => match v.as_str() {
            Some(count_hex) => match parse_hex_quantity(count_hex) {
                // A valid Ethereum hex quantity (the tx count) confirms
                // block `n` exists; the count's actual value is irrelevant
                // to existence, only its validity as a quantity matters.
                Ok(_) => Ok(Some(n)),
                Err(e) => Err(format!(
                    "eth_getBlockTransactionCountByNumber(0x{:x}) chain={:?}: result is not a valid hex quantity: {} (raw={:?})",
                    n, chain, e, text
                )),
            },
            None => Err(format!(
                "eth_getBlockTransactionCountByNumber(0x{:x}) chain={:?}: unexpected result shape (not a string): {:?}",
                n, chain, text
            )),
        },
    }
}

/// Returns `(latest_block, finalized_block)` for the given chain.
///
/// Both elements are the same **consensus-safe** probed height. We do NOT read
/// the volatile chain head (`eth_blockNumber` or a `latest`/`finalized` tag):
/// on a fast-finality chain like Monad those differ across the EVM RPC
/// canister's subnet replicas, so the IC HTTPS-outcall consensus never agrees
/// and the call fails every tick. Instead we probe a SPECIFIC block number
/// `last_observed + MAX_BLOCK_SCAN_WINDOW` via `eth_get_block_number_at` (a
/// fixed, already-final number is byte-identical across replicas):
///   - if that block exists & is final → advance the window up to it
///   - if not (chain hasn't reached it yet) → return the current cursor (no
///     new blocks this tick)
///
/// `last_observed == 0` means the burn-watch cursor is unseeded; the caller
/// (`run_observer`) skips burn-watch entirely in that case (no genesis crawl).
///
/// M-07 (FINAL-1): the cursor advances to `candidate` ONLY when `candidate`
/// satisfies `is_block_final(candidate, finality_depth)` — i.e. block
/// `candidate + finality_depth` also exists, so `candidate` is buried under at
/// least `finality_depth` confirmations. The prior code advanced on mere block
/// EXISTENCE (`is_block_final` was never applied here, unlike `burn_proof.rs`),
/// so the "finalized" cursor it returned could include a still-reorg-able block.
/// Both the burn-watch scan and the settlement mint-confirm gate treat this
/// returned value as final, so advancing it on an unfinalized block let a burn /
/// mint-confirm act on a block that could still reorg out. Now both the probe
/// AND the finality bury-check route through the multi-provider quorum
/// (`eth_get_block_number_at`), so a single lagging provider can neither
/// fabricate nor prematurely finalize the cursor.
pub async fn fetch_block_numbers(chain: ChainId) -> Result<(u64, u64), String> {
    let last_observed = read_state(|s| {
        s.multi_chain
            .last_observed_block
            .get(&chain)
            .copied()
            .unwrap_or(0)
    });
    let finality_depth = read_state(|s| {
        s.multi_chain
            .chain_configs
            .get(&chain)
            .map(|c| c.finality_depth as u64)
    })
    .unwrap_or(1);
    let candidate = last_observed.saturating_add(MAX_BLOCK_SCAN_WINDOW);
    // Advance only when `candidate` is buried under >= finality_depth blocks
    // (M-07). `is_block_final` itself probes `candidate + finality_depth` through
    // the quorum path, so existence of the candidate alone is no longer enough.
    if is_block_final(chain, candidate, finality_depth).await? {
        Ok((candidate, candidate)) // candidate exists AND is final → advance window
    } else {
        Ok((last_observed, last_observed)) // not final yet → nothing new this tick
    }
}

/// True iff block `block + finality_depth` exists & is final on `chain` — i.e.
/// `block` is buried under at least `finality_depth` confirmations. Consensus-safe
/// (probes a SPECIFIC number). On a probe error, propagates Err (caller retries).
pub async fn is_block_final(chain: ChainId, block: u64, finality_depth: u64) -> Result<bool, String> {
    let target = block.saturating_add(finality_depth);
    Ok(eth_get_block_number_at(chain, target).await?.is_some())
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

/// Returns the native balance (in wei) of `address` at a SPECIFIC block number.
/// Like `erc20_total_supply_at`, a fixed block number is byte-identical across
/// the EVM RPC canister's replicas (so HTTPS-outcall consensus is reached) AND,
/// when the number is a finalized block, the read is reorg-safe. The settlement
/// worker uses this to RE-VERIFY a deposit at finality before broadcasting an
/// irreversible mint: deposit DETECTION runs on the volatile `"latest"` balance
/// for liveness (the Gate-4 design), but the mint must only fire against a
/// deposit that is buried and cannot reorg away.
pub async fn get_balance_at_block(chain: ChainId, address: &str, block: u64) -> Result<u128, String> {
    let payload = format!(
        r#"{{"jsonrpc":"2.0","method":"eth_getBalance","params":[{:?},"0x{:x}"],"id":{}}}"#,
        address,
        block,
        next_rpc_id()
    );
    let text = call_evm_rpc(chain, &payload).await?;
    let val: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("eth_getBalance(at block) parse: {}", e))?;
    if let Some(err) = val.get("error") {
        return Err(format!("eth_getBalance(at block) RPC error: {}", err));
    }
    let hex = val["result"]
        .as_str()
        .ok_or_else(|| format!("eth_getBalance(at block): missing result in {:?}", text))?;
    parse_hex_quantity(hex)
}

/// Returns the ERC-20 `totalSupply()` of `contract` at a SPECIFIC block number
/// (e8s for icUSD). Issued via `eth_call` at the fixed block `block`, a
/// specific number is byte-identical across the EVM RPC canister's replicas, so
/// HTTPS-outcall consensus is reached (the same reason `fetch_block_numbers`
/// probes a fixed number, never `latest`). Used by the observer's supply gate
/// to decide whether a burn could have occurred since the last scan.
pub async fn erc20_total_supply_at(
    chain: ChainId,
    contract: &str,
    block: u64,
) -> Result<u128, String> {
    let payload = format!(
        r#"{{"jsonrpc":"2.0","method":"eth_call","params":[{{"to":{:?},"data":{:?}}},"0x{:x}"],"id":{}}}"#,
        contract,
        TOTAL_SUPPLY_SELECTOR,
        block,
        next_rpc_id()
    );
    let text = call_evm_rpc(chain, &payload).await?;
    let val: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("eth_call(totalSupply) parse: {}", e))?;
    if let Some(err) = val.get("error") {
        return Err(format!("eth_call(totalSupply) RPC error: {}", err));
    }
    let hex = val["result"]
        .as_str()
        .ok_or_else(|| format!("eth_call(totalSupply): missing result in {:?}", text))?;
    parse_eth_call_u128(hex)
}

/// Read a generic `eth_call` result hex at a PINNED finalized block (finding
/// #13: a "latest" read returns different bytes per provider and fails the
/// multi-provider quorum; pinning a finalized number is byte-identical across
/// replicas). Shared by the DEX reads below.
async fn eth_call_at_block(chain: ChainId, to: &str, data: &str, block: u64) -> Result<String, String> {
    let payload = format!(
        r#"{{"jsonrpc":"2.0","method":"eth_call","params":[{{"to":{:?},"data":{:?}}},"0x{:x}"],"id":{}}}"#,
        to, data, block, next_rpc_id()
    );
    let text = call_evm_rpc(chain, &payload).await?;
    let val: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("eth_call parse: {}", e))?;
    if let Some(err) = val.get("error") {
        return Err(format!("eth_call RPC error: {}", err));
    }
    val["result"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| format!("eth_call: missing result in {:?}", text))
}

/// UniswapV2 `getReserves()` at a pinned finalized block -> `(reserve0,
/// reserve1)` (finding #13). The reserve ordering follows the pair's `token0()`,
/// which the caller reads separately and orients against.
pub async fn get_reserves(chain: ChainId, pair: &str, block: u64) -> Result<(u128, u128), String> {
    let hex = eth_call_at_block(chain, pair, GET_RESERVES_SELECTOR, block).await?;
    parse_two_uint112(&hex)
}

/// UniswapV2 `token0()` at a pinned block -> the pair's token0 address
/// (0x-prefixed, lowercase). V2 pairs sort tokens by address, so the reserve
/// ordering MUST be read, never assumed.
pub async fn get_pair_token0(chain: ChainId, pair: &str, block: u64) -> Result<String, String> {
    let hex = eth_call_at_block(chain, pair, TOKEN0_SELECTOR, block).await?;
    let raw = hex.strip_prefix("0x").or_else(|| hex.strip_prefix("0X")).unwrap_or(&hex);
    if raw.len() < 40 {
        return Err(format!("token0: result too short: {:?}", hex));
    }
    Ok(format!("0x{}", raw[raw.len() - 40..].to_lowercase()))
}

/// UniswapV2 factory `getPair(tokenA, tokenB)` at a pinned block -> the pair
/// address registered by the factory. Used by config onboarding to ensure the
/// operator-pinned pair is the canonical pool for the configured token path.
pub async fn get_factory_pair(
    chain: ChainId,
    factory: &str,
    token_a: &str,
    token_b: &str,
    block: u64,
) -> Result<String, String> {
    let data = encode_get_pair_calldata(token_a, token_b)?;
    let hex = eth_call_at_block(chain, factory, &data, block).await?;
    parse_eth_call_address(&hex)
}

/// ERC-20 `balanceOf(addr)` at a pinned block (native base units). Used to
/// reconcile the reserve address's on-chain stable custody.
pub async fn erc20_balance_of(chain: ChainId, token: &str, addr: &str, block: u64) -> Result<u128, String> {
    let a = addr.strip_prefix("0x").or_else(|| addr.strip_prefix("0X")).unwrap_or(addr);
    if a.len() != 40 || hex::decode(a).is_err() {
        return Err(format!("erc20_balance_of: invalid address {:?}", addr));
    }
    // selector + 32-byte left-padded address arg.
    let data = format!("{}{:0>64}", BALANCE_OF_SELECTOR, a.to_lowercase());
    let hex = eth_call_at_block(chain, token, &data, block).await?;
    parse_eth_call_u128(&hex)
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
    let max_range = getlogs_max_range_for(chain);
    let mut out = Vec::new();
    let mut start = from_block;
    loop {
        // `to - from <= max_range` per sub-query (the chain's provider cap).
        let chunk_to = start.saturating_add(max_range).min(to_block);
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

/// A transaction receipt plus its logs. `logs` entries are
/// `(address_lowercased, topics, data, log_index)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TxReceiptWithLogs {
    pub tx_hash: Option<String>,
    pub success: bool,
    pub block_number: u64,
    pub logs: Vec<(String, Vec<String>, String, u64)>,
}

/// Pure parser for an `eth_getTransactionReceipt` JSON-RPC response string.
/// Returns Ok(None) if the receipt is null (tx still pending).
pub fn parse_receipt_with_logs(text: &str) -> Result<Option<TxReceiptWithLogs>, String> {
    let val: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| format!("eth_getTransactionReceipt parse: {}", e))?;
    if let Some(err) = val.get("error") {
        return Err(format!("eth_getTransactionReceipt RPC error: {}", err));
    }
    if val["result"].is_null() {
        return Ok(None);
    }
    let res = &val["result"];
    let tx_hash = res["transactionHash"]
        .as_str()
        .map(|hash| hash.to_ascii_lowercase());
    let success = parse_hex_quantity(res["status"].as_str().unwrap_or("0x0"))? == 1;
    let block_number = parse_hex_quantity(
        res["blockNumber"]
            .as_str()
            .ok_or_else(|| format!("receipt missing blockNumber in {:?}", text))?,
    )? as u64;
    let mut logs = Vec::new();
    if let Some(arr) = res["logs"].as_array() {
        for (position, entry) in arr.iter().enumerate() {
            let address = entry["address"].as_str().unwrap_or("").to_lowercase();
            let topics: Vec<String> = entry["topics"]
                .as_array()
                .map(|a| a.iter().filter_map(|t| t.as_str().map(String::from)).collect())
                .unwrap_or_default();
            let data = entry["data"].as_str().unwrap_or("0x").to_string();
            let log_index = match entry["logIndex"].as_str() {
                Some(hex) => parse_hex_quantity(hex)? as u64,
                None => position as u64,
            };
            logs.push((address, topics, data, log_index));
        }
    }
    Ok(Some(TxReceiptWithLogs {
        tx_hash,
        success,
        block_number,
        logs,
    }))
}

/// Fetch a receipt (with logs) for `tx_hash`. Ok(None) = still pending.
pub async fn get_transaction_receipt_with_logs(
    chain: ChainId,
    tx_hash: &str,
) -> Result<Option<TxReceiptWithLogs>, String> {
    let payload = format!(
        r#"{{"jsonrpc":"2.0","method":"eth_getTransactionReceipt","params":[{:?}],"id":{}}}"#,
        tx_hash,
        next_rpc_id()
    );
    let text = call_evm_rpc(chain, &payload).await?;
    parse_receipt_with_logs(&text)
}

/// Broadcasts a signed raw transaction.  Returns the transaction hash on
/// success.
pub async fn send_raw_transaction(chain: ChainId, raw_tx_hex: &str) -> Result<String, String> {
    let payload = format!(
        r#"{{"jsonrpc":"2.0","method":"eth_sendRawTransaction","params":[{:?}],"id":{}}}"#,
        raw_tx_hex,
        next_rpc_id()
    );
    // A broadcast is a write: first-Ok, not quorum (see call_evm_rpc_broadcast).
    let text = call_evm_rpc_broadcast(chain, &payload).await?;
    let val: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("eth_sendRawTransaction parse: {}", e))?;

    if let Some(err) = val.get("error") {
        // IDEMPOTENT-SUCCESS: a broadcast that the node already has is NOT a
        // failure. The IC executes one HTTPS outcall from MANY replicas, so the
        // same signed tx is submitted N times; the first lands and the rest get
        // "already known" / "already exists". Treating that as an error left the
        // op Queued forever (never Inflight, never confirmed) even though the
        // mint landed on-chain. The tx hash is a pure function of the signed
        // bytes, so recover it locally and report success. (A genuine resubmit
        // of a same-nonce tx is likewise safe — it's the identical tx.)
        let msg = err.to_string().to_ascii_lowercase();
        if msg.contains("already known") || msg.contains("already exists") {
            return crate::chains::evm::tx::raw_tx_hash(raw_tx_hex);
        }
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

#[cfg(test)]
mod tests {
    #[test]
    fn getlogs_max_range_is_per_chain() {
        assert_eq!(super::getlogs_max_range_for(crate::chains::config::ChainId(10143)), 100);
        assert_eq!(super::getlogs_max_range_for(crate::chains::config::ChainId(71)), 1000);
        // unknown chain falls back to the conservative Monad cap
        assert_eq!(super::getlogs_max_range_for(crate::chains::config::ChainId(999)), 100);
    }

    // ─── Block-existence probe regression tests (fix/evm-rpc-block-probe-cycles) ─
    //
    // Live incident 2026-08-21: all 3 configured Conflux providers were past the
    // probe target and, queried directly, returned the identical block hash;
    // the backend nevertheless logged "Inconsistent: no block number reached 2
    // distinct-provider agreement" from the TYPED `eth_getBlockByNumber` call.
    // A live cost query for that exact 2-of-3 threshold read measured
    // 3,714,459,200 cycles against the `EVM_RPC_CALL_CYCLES` (2B) attached,
    // establishing underfunding as the cause; the canary's `Inconsistent`
    // response collapsed the per-provider results without exposing their
    // individual `RpcError` variants, so this is a cost-comparison finding,
    // not a directly-observed `TooFewCycles` on every provider. The fix
    // replaces that typed call with a raw `eth_getBlockTransactionCountByNumber`
    // request routed through the already-audited `call_evm_rpc_detailed`
    // quorum. These tests exercise the full decision tree
    // (`tally_provider_outcomes` -> `block_probe_outcome` ->
    // `parse_block_probe_response`) with synthetic provider fixtures,
    // mirroring the ACTUAL Conflux mainnet quorum floor
    // (`CONFLUX_MAINNET_MIN_QUORUM_PROVIDERS` = 2 of 3 configured providers,
    // per `chains::evm::conflux::config`).

    use super::{
        block_probe_outcome, tally_provider_outcomes, ProviderError, QuorumError, RpcError,
        TooFewCyclesRecord,
    };
    use crate::chains::config::ChainId;

    const CHAIN: ChainId = ChainId(1030); // Conflux eSpace mainnet chain id
    const FLOOR: u32 = 2; // CONFLUX_MAINNET_MIN_QUORUM_PROVIDERS

    fn providers() -> [String; 3] {
        [
            "https://evm.confluxrpc.com".to_string(),
            "https://conflux-espace.blockpi.network/v1/rpc/public".to_string(),
            "https://conflux-espace-public.unifra.io".to_string(),
        ]
    }

    fn result_json(id: u64, result: &str) -> String {
        format!(r#"{{"jsonrpc":"2.0","id":{},"result":{}}}"#, id, result)
    }

    fn too_few_cycles_err(url: &str, expected: u64, received: u64) -> String {
        // Mirrors EXACTLY what `single_call` produces for a `TooFewCycles`
        // provider error, so this fixture is a regression check on that format
        // too: `format!("RPC error from {}: {:?}", url, rpc_err)`.
        let rpc_err = RpcError::ProviderError(ProviderError::TooFewCycles(TooFewCyclesRecord {
            expected: candid::Nat::from(expected),
            received: candid::Nat::from(received),
        }));
        format!("RPC error from {}: {:?}", url, rpc_err)
    }

    // (a) quorum of "0x0" (and a nonzero count) at N => Some(N): block exists.
    #[test]
    fn quorum_of_zero_tx_count_confirms_block_exists() {
        let urls = providers();
        let outcomes: Vec<(String, Result<String, String>)> = urls
            .iter()
            .map(|u| (u.clone(), Ok(result_json(1, r#""0x0""#))))
            .collect();
        let tally = tally_provider_outcomes(CHAIN, &outcomes, FLOOR);
        assert!(tally.is_ok(), "expected quorum success, got {:?}", tally);
        let outcome = block_probe_outcome(CHAIN, 154_850_928, tally);
        assert_eq!(outcome, Ok(Some(154_850_928)));
    }

    #[test]
    fn quorum_of_nonzero_tx_count_confirms_block_exists() {
        let urls = providers();
        let outcomes: Vec<(String, Result<String, String>)> = urls
            .iter()
            .map(|u| (u.clone(), Ok(result_json(1, r#""0x5""#))))
            .collect();
        let tally = tally_provider_outcomes(CHAIN, &outcomes, FLOOR);
        let outcome = block_probe_outcome(CHAIN, 42, tally);
        assert_eq!(outcome, Ok(Some(42)));
    }

    // (b) quorum of null => None (future/nonexistent block), benign, fail-closed,
    // cursor unchanged (the caller never advances `last_observed` on Ok(None)).
    #[test]
    fn quorum_of_null_is_benign_not_found() {
        let urls = providers();
        let outcomes: Vec<(String, Result<String, String>)> = urls
            .iter()
            .map(|u| (u.clone(), Ok(result_json(1, "null"))))
            .collect();
        let tally = tally_provider_outcomes(CHAIN, &outcomes, FLOOR);
        assert!(tally.is_ok(), "expected quorum success on agreed null, got {:?}", tally);
        let outcome = block_probe_outcome(CHAIN, 999_999_999, tally);
        assert_eq!(outcome, Ok(None));
    }

    // (c) malformed response or JSON-RPC error => fail closed with an
    // ACTIONABLE Err naming the provider outcome; cursor unchanged (Err,
    // never Ok(Some(_))). A quorum-agreed JSON-RPC error is a genuine RPC
    // problem, not the benign not-yet-produced case (which is signaled
    // ONLY by an explicit `result: null`), so it must not collapse into
    // Ok(None).
    #[test]
    fn malformed_response_fails_closed_with_actionable_error() {
        let err = super::parse_block_probe_response(CHAIN, 7, "not json at all");
        assert!(err.is_err());
        let msg = err.unwrap_err();
        assert!(msg.contains("malformed JSON-RPC response"), "{}", msg);
        assert!(msg.contains("chain="), "{}", msg);
    }

    #[test]
    fn quorum_agreed_json_rpc_error_is_an_actionable_err_not_benign() {
        let urls = providers();
        let outcomes: Vec<(String, Result<String, String>)> = urls
            .iter()
            .map(|u| {
                (
                    u.clone(),
                    Ok(r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"header not found"}}"#.to_string()),
                )
            })
            .collect();
        let tally = tally_provider_outcomes(CHAIN, &outcomes, FLOOR);
        assert!(tally.is_ok(), "providers agreeing on the SAME error still reaches quorum: {:?}", tally);
        let outcome = block_probe_outcome(CHAIN, 7, tally);
        // Fail-closed AND actionable: an agreed JSON-RPC error is NOT the
        // benign not-yet-produced case (that is `result: null` only) and must
        // not be silently swallowed into Ok(None).
        assert!(outcome.is_err(), "{:?}", outcome);
        let msg = outcome.unwrap_err();
        assert!(msg.contains("providers agreed on a JSON-RPC error"), "{}", msg);
        assert!(msg.contains("header not found"), "{}", msg);
    }

    // Missing `result` key entirely (distinct from an explicit `null`) is
    // malformed, not benign: serde_json's `Index` operator collapses "missing
    // key" and "key present with value null" to the same `Value::Null`, so
    // `parse_block_probe_response` must use `.get(...)` rather than indexing
    // to tell them apart. This is a direct regression test for that bug class.
    #[test]
    fn missing_result_key_is_an_error_not_benign_null() {
        let missing = super::parse_block_probe_response(
            CHAIN,
            7,
            r#"{"jsonrpc":"2.0","id":1}"#,
        );
        assert!(missing.is_err(), "{:?}", missing);
        assert!(missing.unwrap_err().contains("missing both result and error"));

        // Contrast: an EXPLICIT null result is still the benign case.
        let explicit_null =
            super::parse_block_probe_response(CHAIN, 7, r#"{"jsonrpc":"2.0","id":1,"result":null}"#);
        assert_eq!(explicit_null, Ok(None));
    }

    // A response is malformed if it carries BOTH result and error.
    #[test]
    fn result_and_error_both_present_is_malformed() {
        let both = super::parse_block_probe_response(
            CHAIN,
            7,
            r#"{"jsonrpc":"2.0","id":1,"result":"0x0","error":{"code":-32000,"message":"weird"}}"#,
        );
        assert!(both.is_err(), "{:?}", both);
        assert!(both.unwrap_err().contains("BOTH result and error"));
    }

    // `result` must be a valid Ethereum hex quantity (per `parse_hex_quantity`)
    // to prove block `n` exists; any string is not sufficient.
    #[test]
    fn non_hex_or_malformed_quantity_result_is_rejected() {
        for bad in ["not-hex", "", "0x", "0xzz", "12345", "0X"] {
            let out = super::parse_block_probe_response(
                CHAIN,
                7,
                &format!(r#"{{"jsonrpc":"2.0","id":1,"result":{:?}}}"#, bad),
            );
            assert!(out.is_err(), "expected Err for result={:?}, got {:?}", bad, out);
            assert!(
                out.unwrap_err().contains("not a valid hex quantity"),
                "wrong error for result={:?}",
                bad
            );
        }
    }

    // A leading-zero-padded hex quantity ("0x00") is a VALID quantity per
    // `parse_hex_quantity` (it tolerates leading zeros, only rejecting a
    // missing prefix or non-hex/empty digits) and so still confirms existence,
    // staying consistent with every other caller of `parse_hex_quantity` in
    // this file.
    #[test]
    fn leading_zero_padded_quantity_is_accepted() {
        let out = super::parse_block_probe_response(CHAIN, 7, r#"{"jsonrpc":"2.0","id":1,"result":"0x00"}"#);
        assert_eq!(out, Ok(Some(7)));
    }

    // A non-string result shape (e.g. a JSON number, not a JSON-RPC-legal hex
    // string) is also rejected.
    #[test]
    fn non_string_result_shape_is_rejected() {
        let out = super::parse_block_probe_response(CHAIN, 7, r#"{"jsonrpc":"2.0","id":1,"result":0}"#);
        assert!(out.is_err(), "{:?}", out);
        assert!(out.unwrap_err().contains("not a string"));
    }

    // (d) partial provider failure: two matching successes + one provider error
    // => 2-of-3 quorum satisfied (Conflux's actual mainnet floor), N accepted.
    #[test]
    fn two_of_three_quorum_accepted_despite_one_provider_error() {
        let urls = providers();
        let outcomes: Vec<(String, Result<String, String>)> = vec![
            (urls[0].clone(), Ok(result_json(1, r#""0x0""#))),
            (urls[1].clone(), Ok(result_json(1, r#""0x0""#))),
            (urls[2].clone(), Err(format!("call error to {} (SysTransient): timeout", urls[2]))),
        ];
        let tally = tally_provider_outcomes(CHAIN, &outcomes, FLOOR);
        assert!(tally.is_ok(), "expected 2-of-3 quorum success, got {:?}", tally);
        let outcome = block_probe_outcome(CHAIN, 154_850_928, tally);
        assert_eq!(outcome, Ok(Some(154_850_928)));
    }

    // (e) all providers TooFewCycles (or otherwise errored) => infrastructure
    // failure diagnosis with per-provider summaries, INCLUDING the TooFewCycles
    // expected/received cycle counts; cursor unchanged (propagated as Err).
    #[test]
    fn all_providers_too_few_cycles_is_an_infra_failure_with_per_provider_detail() {
        let urls = providers();
        let make_outcomes = || -> Vec<(String, Result<String, String>)> {
            urls.iter()
                .map(|u| (u.clone(), Err(too_few_cycles_err(u, 3_714_459_200, 2_000_000_000))))
                .collect()
        };

        let tally = tally_provider_outcomes(CHAIN, &make_outcomes(), FLOOR);
        let quorum_err = tally.expect_err("all-providers-failed must be Err");
        assert!(matches!(quorum_err, QuorumError::AllProvidersFailed(_)));
        let detail = quorum_err.to_string();
        // Actionable: names the failure class...
        assert!(detail.contains("infrastructure failure"), "{}", detail);
        // ...every provider by URL...
        for url in &urls {
            assert!(detail.contains(url.as_str()), "missing {} in: {}", url, detail);
        }
        // ...and the exact TooFewCycles expected/received cycle counts.
        assert!(detail.contains("TooFewCycles"), "{}", detail);
        assert!(detail.contains("3714459200") || detail.contains("3_714_459_200"), "{}", detail);
        assert!(detail.contains("2000000000") || detail.contains("2_000_000_000"), "{}", detail);

        // The probe propagates this as Err (not Ok(None)) so the caller's
        // "fetch_block_numbers failed; will retry" log fires distinctly from
        // the benign not-yet-produced case. Fail-closed either way.
        let outcome = block_probe_outcome(
            CHAIN,
            154_850_928,
            tally_provider_outcomes(CHAIN, &make_outcomes(), FLOOR),
        );
        assert!(outcome.is_err(), "{:?}", outcome);
        let msg = outcome.unwrap_err();
        assert!(msg.contains("infrastructure failure"), "{}", msg);
        assert!(msg.contains("TooFewCycles"), "{}", msg);
    }

    // All-providers-plain-errored (not TooFewCycles specifically) is the SAME
    // infra-failure class, still fail-closed.
    #[test]
    fn all_providers_errored_generically_is_an_infra_failure() {
        let urls = providers();
        let outcomes: Vec<(String, Result<String, String>)> = urls
            .iter()
            .map(|u| (u.clone(), Err(format!("call error to {} (SysFatal): canister trapped", u))))
            .collect();
        let tally = tally_provider_outcomes(CHAIN, &outcomes, FLOOR);
        let quorum_err = tally.expect_err("all-providers-failed must be Err");
        assert!(matches!(quorum_err, QuorumError::AllProvidersFailed(_)));
        assert!(quorum_err.to_string().contains("infrastructure failure"));
    }

    // Configuration failure (below the quorum floor) is distinct from a
    // provider-side infrastructure failure, and also fails closed.
    #[test]
    fn below_floor_is_a_configuration_error_not_all_providers_failed() {
        let outcomes: Vec<(String, Result<String, String>)> =
            vec![("https://only-one.example".to_string(), Ok(result_json(1, r#""0x0""#)))];
        // Only 1 distinct outcome collected but the chain's floor is 2: this
        // exact gate lives in `call_evm_rpc_detailed` (checked before any
        // network call), so `tally_provider_outcomes` itself is never reached
        // with fewer outcomes than endpoints in production. This test instead
        // confirms `QuorumError::Configuration` is a distinct variant from
        // `AllProvidersFailed`, since `eth_get_block_number_at` maps both to an
        // `Err` but an operator reading the log needs the right one.
        let cfg_err = QuorumError::Configuration("no RPC endpoints configured".to_string());
        let all_failed_err = QuorumError::AllProvidersFailed("all 3 providers errored".to_string());
        assert_ne!(cfg_err, all_failed_err);
        // Sanity: the fixture above still reaches quorum on its own (1-of-1),
        // demonstrating `tally_provider_outcomes` doesn't itself re-check floor.
        assert!(tally_provider_outcomes(CHAIN, &outcomes, 1).is_ok());
    }
}
