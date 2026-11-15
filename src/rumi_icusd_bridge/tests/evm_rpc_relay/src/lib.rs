//! TEST-ONLY `evm_rpc_relay` canister.
//!
//! Speaks the raw JSON-RPC escape hatch of the DFINITY EVM RPC canister
//!
//!   request     : (RpcService, text /*json*/, nat64 /*max_response_bytes*/) -> (RequestResult)
//!   requestCost : (RpcService, text, nat64) -> (variant { Ok : nat; Err : RpcError }) query
//!
//! and forwards each `Custom { url, headers }` request as a real IC HTTPS outcall
//! (POST, JSON body). Under PocketIC the outcall is intercepted by the test's
//! outcall pump (`tests/support/outcalls.rs`), which either forwards it to a
//! local anvil node (real EVM execution) or injects a fault.
//!
//! The relay does no consensus, no provider selection and no retries: one call
//! in, one outcall out. That is deliberate. Multi-provider quorum is the bridge
//! canister's job (it calls `request` once per provider URL) and must be tested
//! there.
//!
//! Also exposes `probe_ecdsa`, a harness-only helper proving threshold ECDSA
//! works in the PocketIC build under test.

use candid::{CandidType, Deserialize, Nat};
use ic_cdk::api::call::RejectionCode as IcRejectionCode;
use ic_cdk::api::management_canister::ecdsa::{
    ecdsa_public_key, EcdsaCurve, EcdsaKeyId, EcdsaPublicKeyArgument,
};
use ic_cdk::api::management_canister::http_request::{
    http_request, CanisterHttpRequestArgument, HttpHeader as IcHttpHeader, HttpMethod,
    HttpResponse, TransformArgs, TransformContext,
};

// ---------------------------------------------------------------------------
// Candid types (shapes must match the live EVM RPC canister .did exactly)
// ---------------------------------------------------------------------------

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct HttpHeader {
    pub value: String,
    pub name: String,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct RpcApi {
    pub url: String,
    pub headers: Option<Vec<HttpHeader>>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum L2MainnetService {
    Alchemy,
    Ankr,
    BlockPi,
    PublicNode,
    Llama,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum EthMainnetService {
    Alchemy,
    Ankr,
    BlockPi,
    Cloudflare,
    PublicNode,
    Llama,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum EthSepoliaService {
    Alchemy,
    Ankr,
    BlockPi,
    PublicNode,
    Sepolia,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum RpcService {
    EthSepolia(EthSepoliaService),
    EthMainnet(EthMainnetService),
    ArbitrumOne(L2MainnetService),
    BaseMainnet(L2MainnetService),
    OptimismMainnet(L2MainnetService),
    Provider(u64),
    Custom(RpcApi),
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

// ---------------------------------------------------------------------------
// Outcall plumbing
// ---------------------------------------------------------------------------

const TRANSFORM_NAME: &str = "transform_drop_headers";

/// Upper bound on `max_response_bytes` the real management canister accepts.
const MAX_RESPONSE_BYTES_LIMIT: u64 = 2_000_000;

/// Cycles attached to each outcall. Computed with the IC cost model for the
/// LARGEST subnet size (n = 34) so it always covers the real cost on a smaller
/// subnet. The management canister refunds whatever is not consumed, and the
/// test funds this canister generously.
fn outcall_cycles(request_bytes: u64, max_response_bytes: u64) -> u128 {
    let n: u128 = 34;
    let base = 3_000_000u128 + 60_000u128 * n;
    let per_byte = 400u128 * (request_bytes as u128 + 2 * max_response_bytes as u128);
    (base + per_byte) * n
}

/// Strip response headers so every replica sees the same transformed response.
#[ic_cdk::query(name = "transform_drop_headers")]
fn transform_drop_headers(args: TransformArgs) -> HttpResponse {
    HttpResponse {
        status: args.response.status,
        headers: vec![],
        body: args.response.body,
    }
}

fn map_rejection(code: IcRejectionCode) -> RejectionCode {
    match code {
        IcRejectionCode::NoError => RejectionCode::NoError,
        IcRejectionCode::SysFatal => RejectionCode::SysFatal,
        IcRejectionCode::SysTransient => RejectionCode::SysTransient,
        IcRejectionCode::DestinationInvalid => RejectionCode::DestinationInvalid,
        IcRejectionCode::CanisterReject => RejectionCode::CanisterReject,
        IcRejectionCode::CanisterError => RejectionCode::CanisterError,
        IcRejectionCode::Unknown => RejectionCode::Unknown,
    }
}

fn provider_not_found() -> RpcError {
    RpcError::ProviderError(ProviderError::ProviderNotFound)
}

/// Raw JSON-RPC passthrough, the same escape hatch the bridge uses on the live
/// EVM RPC canister. Only `RpcService::Custom` is served.
#[ic_cdk::update(name = "request")]
async fn request(service: RpcService, json: String, max_response_bytes: u64) -> RequestResult {
    do_request(service, json, max_response_bytes).await
}

/// Harness-only: several `request`s run SEQUENTIALLY inside ONE update call
/// (each parks on its own outcall). Lets the harness prove the outcall pump
/// handles a call that makes many outcalls one after another, which is what the
/// bridge's per-tick observer does.
#[ic_cdk::update(name = "request_seq")]
async fn request_seq(
    service: RpcService,
    jsons: Vec<String>,
    max_response_bytes: u64,
) -> Vec<RequestResult> {
    let mut out = Vec::with_capacity(jsons.len());
    for json in jsons {
        out.push(do_request(service.clone(), json, max_response_bytes).await);
    }
    out
}

async fn do_request(service: RpcService, json: String, max_response_bytes: u64) -> RequestResult {
    let api = match service {
        RpcService::Custom(api) => api,
        _ => return RequestResult::Err(provider_not_found()),
    };

    let mut headers = vec![IcHttpHeader {
        name: "Content-Type".to_string(),
        value: "application/json".to_string(),
    }];
    if let Some(extra) = api.headers {
        for h in extra {
            headers.push(IcHttpHeader {
                name: h.name,
                value: h.value,
            });
        }
    }

    let body = json.into_bytes();
    let cycles = outcall_cycles(body.len() as u64, max_response_bytes);
    let arg = CanisterHttpRequestArgument {
        url: api.url,
        max_response_bytes: Some(max_response_bytes.min(MAX_RESPONSE_BYTES_LIMIT)),
        method: HttpMethod::POST,
        headers,
        body: Some(body),
        transform: Some(TransformContext::from_name(
            TRANSFORM_NAME.to_string(),
            vec![],
        )),
    };

    match http_request(arg, cycles).await {
        Ok((resp,)) => {
            let status: u16 = resp.status.to_string().parse().unwrap_or(0);
            match String::from_utf8(resp.body) {
                Ok(text) if status == 200 => RequestResult::Ok(text),
                Ok(text) => RequestResult::Err(RpcError::HttpOutcallError(
                    HttpOutcallError::InvalidHttpJsonRpcResponse(InvalidHttpJsonRpcRecord {
                        status,
                        body: text,
                        parsing_error: None,
                    }),
                )),
                Err(e) => RequestResult::Err(RpcError::HttpOutcallError(
                    HttpOutcallError::InvalidHttpJsonRpcResponse(InvalidHttpJsonRpcRecord {
                        status,
                        body: String::from_utf8_lossy(e.as_bytes()).into_owned(),
                        parsing_error: Some("response body is not valid UTF-8".to_string()),
                    }),
                )),
            }
        }
        Err((code, message)) => RequestResult::Err(RpcError::HttpOutcallError(
            HttpOutcallError::IcError(IcErrorRecord {
                code: map_rejection(code),
                message,
            }),
        )),
    }
}

/// The live EVM RPC canister quotes the cycles a `request` needs. The relay
/// needs none from the caller (the test funds it), so it always quotes zero for
/// a Custom service and ProviderNotFound otherwise.
#[ic_cdk::query(name = "requestCost")]
fn request_cost(service: RpcService, _json: String, _max_response_bytes: u64) -> RequestCostResult {
    match service {
        RpcService::Custom(_) => RequestCostResult::Ok(Nat::from(0u64)),
        _ => RequestCostResult::Err(provider_not_found()),
    }
}

// ---------------------------------------------------------------------------
// Harness helper
// ---------------------------------------------------------------------------

/// Returns the SEC1 compressed secp256k1 public key for `key_name` at
/// `derivation_path` (relative to this canister). Used by the harness smoke test
/// to prove which threshold-ECDSA key names the PocketIC build under test serves.
#[ic_cdk::update]
async fn probe_ecdsa(key_name: String, derivation_path: Vec<Vec<u8>>) -> Result<Vec<u8>, String> {
    let arg = EcdsaPublicKeyArgument {
        canister_id: None,
        derivation_path,
        key_id: EcdsaKeyId {
            curve: EcdsaCurve::Secp256k1,
            name: key_name,
        },
    };
    match ecdsa_public_key(arg).await {
        Ok((resp,)) => Ok(resp.public_key),
        Err((code, msg)) => Err(format!("{:?}: {}", code, msg)),
    }
}
