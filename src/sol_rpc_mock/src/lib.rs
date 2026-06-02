//! Mock SOL RPC canister for the Solana M2 end-to-end PocketIC test (Task 9).
//!
//! Speaks the REAL SOL RPC canister `jsonRequest` escape-hatch interface so the
//! production backend wrapper (`chains/solana/sol_rpc.rs`) can talk to it
//! unchanged via `set_sol_rpc_principal`. The Candid arg/return types are
//! mirrored (minimally) from `sol_rpc.rs` so encode/decode line up byte-for-byte:
//!
//!   jsonRequest : (RpcSources, opt RpcConfig, text /*json payload*/)
//!               -> (MultiRequestResult)
//!
//! It parses the JSON-RPC `method` out of the `payload` string (and, for
//! `getAccountInfo`, branches on the `encoding` param) and returns
//! `MultiRequestResult::Consistent(RequestResult::Ok("<canned json>"))` shaped
//! exactly as the wrapper's parsers expect (verified against `sol_rpc.rs`):
//!
//!   - getBalance      -> {"result":{"value":<lamports>}}        (parse_balance_lamports)
//!   - getAccountInfo (encoding jsonParsed, the SPL-mint supply read)
//!                     -> {"result":{"value":{"data":{"parsed":{"info":{"supply":"<e8s>"}}}}}}
//!                                                                (parse_mint_supply_jsonparsed)
//!   - getAccountInfo (encoding base64, the durable-NONCE account read)
//!                     -> {"result":{"value":{"data":["<b64 of 80-byte nonce acct>","base64"]}}}
//!                                                                (parse_account_data_base64 +
//!                                                                 parse_nonce_account_blockhash)
//!   - sendTransaction -> {"result":"<signature base58>"}        (parse_send_transaction_signature)
//!   - getTransaction  -> {"result":{"slot":<u64>,"meta":{"err":null}}} when confirmed
//!                        / {"result":null} when not found        (parse_get_transaction)
//!   - getSlot         -> {"result":<u64>}                        (parse_slot)
//!   - getLatestBlockhash -> {"result":{"value":{"blockhash":"<b58>"}}} (parse_latest_blockhash)
//!
//! Behavior is fully scripted by the test via the `set_*` test-control endpoints,
//! backed by a `thread_local! RefCell<Script>` (same pattern as monad_rpc_mock).
//!
//! Build:
//!   cargo build --target wasm32-unknown-unknown --release --package sol_rpc_mock

use base64::Engine;
use candid::{CandidType, Deserialize, Reserved};
use std::cell::RefCell;
use std::collections::HashMap;

// ─── Candid types mirrored from chains/solana/sol_rpc.rs ─────────────────────
//
// These must encode/decode identically to the backend's local defs. The request
// side (`RpcSources`, `SolanaCluster`, `ConsensusStrategy`, `RpcConfig`) is what
// the backend ENCODES and the mock must DECODE; the response side
// (`MultiRequestResult` / `RequestResult` / `RpcError`) is what the mock ENCODES
// and the backend must DECODE. The unused error arms (`ProviderError`,
// `HttpOutcallError`, `Inconsistent`) are typed `candid::Reserved` exactly like
// the backend's mirror, so the whole type table is structurally identical even
// though the mock only ever returns `Consistent(Ok(..))`.

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum SolanaCluster {
    Mainnet,
    Devnet,
    Testnet,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum RpcSources {
    Default(SolanaCluster),
    // `Custom(vec RpcSource)` exists in the .did but is unused (and unused by the
    // backend wrapper, which always sends `Default(Devnet)`).
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum ConsensusStrategy {
    Equality,
    Threshold { total: Option<u8>, min: u8 },
}

/// Field names match `sol_rpc.rs` exactly (camelCase) so candid hashes line up.
#[derive(CandidType, Deserialize, Clone, Debug)]
#[allow(non_snake_case)]
pub struct RpcConfig {
    pub responseSizeEstimate: Option<u64>,
    pub responseConsensus: Option<ConsensusStrategy>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
}

/// Mirrors `sol_rpc.rs`'s `RpcError` (variant NAMES + ORDER identical). The
/// `ProviderError` / `HttpOutcallError` payloads are `Reserved` (never emitted by
/// the mock; present only so the type table matches the backend's decoder).
#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum RpcError {
    JsonRpcError(JsonRpcError),
    ProviderError(Reserved),
    ValidationError(String),
    HttpOutcallError(Reserved),
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum RequestResult {
    Ok(String),
    Err(RpcError),
}

/// The `Inconsistent` arm's per-provider vector is `Reserved` (mirrors
/// `sol_rpc.rs`): the mock always returns `Consistent`, but the type must define
/// both arms in the same order so the backend's `Decode!` subtype-check passes.
#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum MultiRequestResult {
    Consistent(RequestResult),
    Inconsistent(Reserved),
}

// ─── Durable-nonce account layout (mirrors sol_rpc.rs) ───────────────────────
//
// The settlement worker's `sign_mint` / `sign_withdrawal` read the durable nonce
// via `get_durable_nonce(nonce_addr)`, which issues a base64 `getAccountInfo` and
// parses an 80-byte System nonce account:
//   version: u32 LE          [0..4]
//   state:   u32 LE          [4..8]   (1 = Initialized)
//   authority: Pubkey        [8..40]
//   durable_nonce/blockhash: [40..72]
//   fee_calculator.lamports_per_signature: u64 LE [72..80]
// The mock pre-supplies a valid Initialized account so the FULL signing path
// works WITHOUT running the real `solana_bootstrap_nonce`.

const NONCE_STATE_SIZE: usize = 80;
const NONCE_STATE_INITIALIZED: u32 = 1;

/// Build an 80-byte Initialized nonce account with the given 32-byte blockhash.
/// authority + fee_calculator are left zeroed (the backend reads only `state` and
/// the `[40..72]` blockhash).
fn build_nonce_account(blockhash: [u8; 32]) -> Vec<u8> {
    let mut buf = vec![0u8; NONCE_STATE_SIZE];
    // version = 0 (buf[0..4] already zero).
    buf[4..8].copy_from_slice(&NONCE_STATE_INITIALIZED.to_le_bytes());
    // authority [8..40] left zero (unread by the backend).
    buf[40..72].copy_from_slice(&blockhash);
    // fee_calculator [72..80] left zero.
    buf
}

// ─── Scripted state ──────────────────────────────────────────────────────────

struct Script {
    /// pubkey (base58) -> balance in lamports.
    balances: HashMap<String, u64>,
    /// SPL-mint on-chain supply (e8s) returned by the jsonParsed `getAccountInfo`.
    /// `Some` => `result.value...supply` present; `None` => `result.value` is null
    /// (mint not found). Defaults to `Some(0)` so the observer's supply gate
    /// parses cleanly (a 0 supply equals a fresh, un-minted mint).
    mint_supply: Option<u64>,
    /// The 32-byte durable-nonce blockhash returned (inside an Initialized 80-byte
    /// account) for EVERY base64 `getAccountInfo` read. Defaults to a non-zero
    /// placeholder so signing works out of the box.
    nonce_blockhash: [u8; 32],
    /// Whether the nonce account exists. When false the base64 `getAccountInfo`
    /// returns `result.value: null` (account not found) so a test can exercise the
    /// "nonce not bootstrapped" error. Defaults true.
    nonce_exists: bool,
    /// Whether `getTransaction` reports the tx as confirmed. true =>
    /// `{"result":{"slot":<slot>,"meta":{"err":null}}}` (Confirmed); false =>
    /// `{"result":null}` (NotFound). Defaults true so the happy path settles.
    tx_confirmed: bool,
    /// The slot reported in a confirmed `getTransaction` (and as the bare `getSlot`
    /// result). Defaults to a plausible non-zero slot.
    slot: u64,
    /// The base58 signature `sendTransaction` returns. The backend computes the
    /// signature LOCALLY and confirms by ITS own value, so this need not match;
    /// it is a plausible 64-byte base58 placeholder.
    send_signature: String,
    /// The base58 blockhash returned by `getLatestBlockhash` (completeness; the
    /// happy path uses the durable nonce, not this).
    latest_blockhash_b58: String,
}

impl Default for Script {
    fn default() -> Self {
        Script {
            balances: HashMap::new(),
            mint_supply: Some(0),
            // A non-zero, recognizable default blockhash (bytes 1..=32).
            nonce_blockhash: {
                let mut h = [0u8; 32];
                for (i, b) in h.iter_mut().enumerate() {
                    *b = (i as u8) + 1;
                }
                h
            },
            nonce_exists: true,
            tx_confirmed: true,
            slot: 300_000_000,
            // A plausible 64-byte base58 signature (the base58 of 64 0x11 bytes).
            // The exact value is irrelevant: the backend tracks by its own locally
            // computed signature and confirms by that, not by this string.
            send_signature:
                "4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQff4P3bkLKi2NJ1JJJ1JJJ1JJJ1JJJ1JJJ1JJJ1JJJ1JJJ1JJJ1JJJ1"
                    .to_string(),
            latest_blockhash_b58: "11111111111111111111111111111111".to_string(),
        }
    }
}

thread_local! {
    static SCRIPT: RefCell<Script> = RefCell::new(Script::default());
}

// ─── Init ────────────────────────────────────────────────────────────────────

#[ic_cdk_macros::init]
fn init() {
    // Defaults are set in `Script::default()`; nothing extra to seed.
}

// ─── The real SOL RPC `jsonRequest` escape-hatch ─────────────────────────────

/// Mirrors the SOL RPC canister's generic `jsonRequest` method. The backend
/// wrapper calls this via `call_with_payment128(canister, "jsonRequest",
/// (RpcSources, Option<RpcConfig>, String), cycles)`. We ignore the `sources` /
/// `config` (the test configures the cluster; the mock serves all methods),
/// parse the JSON-RPC `method`, and return a canned `Consistent(Ok(..))`.
#[ic_cdk_macros::update]
#[allow(non_snake_case)]
fn jsonRequest(_sources: RpcSources, _config: Option<RpcConfig>, payload: String) -> MultiRequestResult {
    let parsed: serde_json::Value = match serde_json::from_str(&payload) {
        Ok(v) => v,
        Err(e) => {
            return MultiRequestResult::Consistent(RequestResult::Err(RpcError::JsonRpcError(
                JsonRpcError {
                    code: -32700,
                    message: format!("mock: invalid json payload: {e}"),
                },
            )))
        }
    };
    let method = parsed["method"].as_str().unwrap_or("");
    let id = parsed["id"].clone();
    let params = &parsed["params"];

    let response_json: String = SCRIPT.with(|s| {
        let script = s.borrow();
        match method {
            "getBalance" => {
                // params = [pubkey, {commitment}]. Return result.value = lamports.
                let pubkey = params.get(0).and_then(|v| v.as_str()).unwrap_or("");
                let bal = script.balances.get(pubkey).copied().unwrap_or(0);
                format!(
                    r#"{{"jsonrpc":"2.0","id":{},"result":{{"value":{}}}}}"#,
                    id, bal
                )
            }
            "getAccountInfo" => {
                // params = [pubkey, {encoding, commitment}]. Branch on `encoding`:
                //   - "jsonParsed" => the SPL-mint supply read.
                //   - "base64"     => the durable-nonce account read.
                let encoding = params
                    .get(1)
                    .and_then(|cfg| cfg.get("encoding"))
                    .and_then(|e| e.as_str())
                    .unwrap_or("");
                match encoding {
                    "jsonParsed" => match script.mint_supply {
                        Some(supply) => format!(
                            r#"{{"jsonrpc":"2.0","id":{},"result":{{"value":{{"data":{{"parsed":{{"info":{{"supply":"{}"}}}}}}}}}}}}"#,
                            id, supply
                        ),
                        // Null value => "mint account not found".
                        None => format!(
                            r#"{{"jsonrpc":"2.0","id":{},"result":{{"value":null}}}}"#,
                            id
                        ),
                    },
                    // Default to the base64 (nonce) shape for "base64" (and any
                    // other/absent encoding) so a durable-nonce read always works.
                    _ => {
                        if !script.nonce_exists {
                            format!(
                                r#"{{"jsonrpc":"2.0","id":{},"result":{{"value":null}}}}"#,
                                id
                            )
                        } else {
                            let acct = build_nonce_account(script.nonce_blockhash);
                            let b64 = base64::engine::general_purpose::STANDARD.encode(&acct);
                            format!(
                                r#"{{"jsonrpc":"2.0","id":{},"result":{{"value":{{"data":["{}","base64"]}}}}}}"#,
                                id, b64
                            )
                        }
                    }
                }
            }
            "sendTransaction" => {
                // params = [b64tx, {encoding, skipPreflight}]. result = signature.
                format!(
                    r#"{{"jsonrpc":"2.0","id":{},"result":{:?}}}"#,
                    id, script.send_signature
                )
            }
            "getTransaction" => {
                // params = [sig, {encoding, commitment, maxSupportedTransactionVersion}].
                // Confirmed => non-null result with meta.err == null; otherwise null.
                if script.tx_confirmed {
                    format!(
                        r#"{{"jsonrpc":"2.0","id":{},"result":{{"slot":{},"meta":{{"err":null}}}}}}"#,
                        id, script.slot
                    )
                } else {
                    format!(r#"{{"jsonrpc":"2.0","id":{},"result":null}}"#, id)
                }
            }
            "getSlot" => {
                // result is a BARE u64 (not nested under result.value).
                format!(r#"{{"jsonrpc":"2.0","id":{},"result":{}}}"#, id, script.slot)
            }
            "getLatestBlockhash" => {
                format!(
                    r#"{{"jsonrpc":"2.0","id":{},"result":{{"value":{{"blockhash":{:?}}}}}}}"#,
                    id, script.latest_blockhash_b58
                )
            }
            other => {
                // Unknown method: a JSON-RPC error so an unexpected wrapper call is
                // loud rather than silently mis-parsed.
                format!(
                    r#"{{"jsonrpc":"2.0","id":{},"error":{{"code":-32601,"message":"mock: unsupported method {}"}}}}"#,
                    id, other
                )
            }
        }
    });

    MultiRequestResult::Consistent(RequestResult::Ok(response_json))
}

// ─── Test-control endpoints (called by the PocketIC test) ────────────────────

/// Set a pubkey's lamport balance (used for the custody-deposit detection AND the
/// settlement hot-wallet gas gate).
#[ic_cdk_macros::update]
fn set_balance(pubkey: String, lamports: u64) {
    SCRIPT.with(|s| {
        s.borrow_mut().balances.insert(pubkey, lamports);
    });
}

/// Set the SPL-mint on-chain supply (e8s) returned by the jsonParsed
/// `getAccountInfo`. The observer's M2 supply gate reads this for drop detection.
#[ic_cdk_macros::update]
fn set_mint_supply(supply_e8s: u64) {
    SCRIPT.with(|s| {
        s.borrow_mut().mint_supply = Some(supply_e8s);
    });
}

/// Make the SPL-mint `getAccountInfo` (jsonParsed) return a null value
/// (mint-not-found), for tests of the unset-mint path.
#[ic_cdk_macros::update]
fn clear_mint_supply() {
    SCRIPT.with(|s| {
        s.borrow_mut().mint_supply = None;
    });
}

/// Set the 32-byte durable-nonce blockhash returned (inside an Initialized 80-byte
/// account) for the base64 `getAccountInfo` read. `bytes` must be exactly 32 bytes.
#[ic_cdk_macros::update]
fn set_nonce_blockhash(bytes: Vec<u8>) {
    let mut h = [0u8; 32];
    let n = bytes.len().min(32);
    h[..n].copy_from_slice(&bytes[..n]);
    SCRIPT.with(|s| {
        s.borrow_mut().nonce_blockhash = h;
    });
}

/// Toggle whether the durable-nonce account exists. When `false`, the base64
/// `getAccountInfo` returns `result.value: null` (not bootstrapped). Default true.
#[ic_cdk_macros::update]
fn set_nonce_exists(exists: bool) {
    SCRIPT.with(|s| {
        s.borrow_mut().nonce_exists = exists;
    });
}

/// Toggle whether `getTransaction` reports the tx confirmed (true => Confirmed at
/// `slot`; false => NotFound). Lets a test hold a submitted op Inflight, then
/// flip it confirmed. Default true.
#[ic_cdk_macros::update]
fn set_tx_confirmed(confirmed: bool) {
    SCRIPT.with(|s| {
        s.borrow_mut().tx_confirmed = confirmed;
    });
}

/// Set the slot reported by `getSlot` and in a confirmed `getTransaction`.
#[ic_cdk_macros::update]
fn set_slot(slot: u64) {
    SCRIPT.with(|s| {
        s.borrow_mut().slot = slot;
    });
}
