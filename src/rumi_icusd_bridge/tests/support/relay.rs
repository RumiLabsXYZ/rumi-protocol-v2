//! Client-side Candid mirror of the `evm_rpc_relay` test canister (see
//! `tests/evm_rpc_relay/evm_rpc_relay.did`) plus thin call helpers.
//!
//! The types match the live EVM RPC canister's `request` escape hatch, so the
//! same shapes also describe what the bridge canister will exchange with it.

use super::outcalls::{call_with_outcalls, OutcallRouter};
use candid::{decode_one, encode_args, CandidType, Deserialize, Nat, Principal};
use pocket_ic::{PocketIc, WasmResult};

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct HttpHeader {
    pub value: String,
    pub name: String,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct RpcApi {
    pub url: String,
    pub headers: Option<Vec<HttpHeader>>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum L2MainnetService {
    Alchemy,
    Ankr,
    BlockPi,
    PublicNode,
    Llama,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum EthMainnetService {
    Alchemy,
    Ankr,
    BlockPi,
    Cloudflare,
    PublicNode,
    Llama,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum EthSepoliaService {
    Alchemy,
    Ankr,
    BlockPi,
    PublicNode,
    Sepolia,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum RpcService {
    EthSepolia(EthSepoliaService),
    EthMainnet(EthMainnetService),
    ArbitrumOne(L2MainnetService),
    BaseMainnet(L2MainnetService),
    OptimismMainnet(L2MainnetService),
    Provider(u64),
    Custom(RpcApi),
}

impl RpcService {
    /// `Custom { url, headers: None }`.
    pub fn custom(url: &str) -> RpcService {
        RpcService::Custom(RpcApi {
            url: url.to_string(),
            headers: None,
        })
    }
}

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

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct TooFewCyclesRecord {
    pub expected: Nat,
    pub received: Nat,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ProviderError {
    TooFewCycles(TooFewCyclesRecord),
    MissingRequiredProvider,
    ProviderNotFound,
    NoPermission,
    InvalidRpcConfig(String),
}

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
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ValidationError {
    Custom(String),
    InvalidHex(String),
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum RpcError {
    ProviderError(ProviderError),
    HttpOutcallError(HttpOutcallError),
    JsonRpcError(JsonRpcError),
    ValidationError(ValidationError),
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum RequestResult {
    Ok(String),
    Err(RpcError),
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum RequestCostResult {
    Ok(Nat),
    Err(RpcError),
}

/// `relay.request(service, json, max_response_bytes)` driven through the
/// outcall pump (the call parks on an HTTPS outcall until the router answers).
pub fn request(
    pic: &PocketIc,
    relay: Principal,
    router: &mut dyn OutcallRouter,
    service: RpcService,
    json: &str,
    max_response_bytes: u64,
) -> RequestResult {
    let arg =
        encode_args((service, json.to_string(), max_response_bytes)).expect("encode request args");
    let bytes = call_with_outcalls(pic, relay, Principal::anonymous(), "request", arg, router)
        .unwrap_or_else(|e| panic!("relay.request failed: {e}"));
    decode_one(&bytes).expect("decode RequestResult")
}

/// `relay.request_seq(service, jsons, max_response_bytes)`: several requests run
/// sequentially inside ONE update call (one outcall each).
pub fn request_seq(
    pic: &PocketIc,
    relay: Principal,
    router: &mut dyn OutcallRouter,
    service: RpcService,
    jsons: Vec<String>,
    max_response_bytes: u64,
) -> Vec<RequestResult> {
    let arg = encode_args((service, jsons, max_response_bytes)).expect("encode request_seq args");
    let bytes = call_with_outcalls(
        pic,
        relay,
        Principal::anonymous(),
        "request_seq",
        arg,
        router,
    )
    .unwrap_or_else(|e| panic!("relay.request_seq failed: {e}"));
    decode_one(&bytes).expect("decode Vec<RequestResult>")
}

/// `relay.requestCost(..)` (a query; no outcall).
pub fn request_cost(
    pic: &PocketIc,
    relay: Principal,
    service: RpcService,
    json: &str,
    max_response_bytes: u64,
) -> RequestCostResult {
    let arg = encode_args((service, json.to_string(), max_response_bytes)).expect("encode");
    match pic
        .query_call(relay, Principal::anonymous(), "requestCost", arg)
        .expect("requestCost query")
    {
        WasmResult::Reply(b) => decode_one(&b).expect("decode RequestCostResult"),
        WasmResult::Reject(m) => panic!("requestCost rejected: {m}"),
    }
}

/// `relay.probe_ecdsa(key_name, derivation_path)`: `Ok(sec1_pubkey)` or `Err(text)`.
pub fn probe_ecdsa(
    pic: &PocketIc,
    relay: Principal,
    key_name: &str,
    derivation_path: Vec<Vec<u8>>,
) -> Result<Vec<u8>, String> {
    let arg = encode_args((key_name.to_string(), derivation_path)).expect("encode");
    match pic
        .update_call(relay, Principal::anonymous(), "probe_ecdsa", arg)
        .expect("probe_ecdsa call")
    {
        WasmResult::Reply(b) => decode_one(&b).expect("decode probe_ecdsa result"),
        WasmResult::Reject(m) => Err(format!("rejected: {m}")),
    }
}

/// A JSON-RPC request body string.
pub fn jsonrpc(method: &str, params: serde_json::Value) -> String {
    serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string()
}
