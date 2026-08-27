//! PocketIC compatibility proof for the developer-gated RPC endpoint-set
//! digest. The wire response commits to live stored configuration without ever
//! returning credential-bearing endpoint URLs.

use candid::{decode_one, encode_args, encode_one, Principal};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use rumi_protocol_backend::chains::config::{
    ChainId, ChainRpcEndpointDigestError, ChainRpcEndpointSetDigestV1, GasStrategy,
    RegisterChainArg,
};
use rumi_protocol_backend::{InitArg, ProtocolArg, ProtocolError};

const CFX: ChainId = ChainId(1030);
const SECRET_BEARING_URL: &str = "https://provider.example/v1/super-secret-token";

fn developer() -> Principal {
    Principal::from_slice(&[0x44; 29])
}

fn outsider() -> Principal {
    Principal::from_slice(&[0x55; 29])
}

fn backend_wasm() -> Vec<u8> {
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let path = std::env::var_os("RUMI_PROTOCOL_BACKEND_WASM")
        .map(std::path::PathBuf::from)
        .map(|candidate| {
            if candidate.is_absolute() {
                candidate
            } else {
                workspace_root.join(candidate)
            }
        })
        .unwrap_or_else(|| {
            workspace_root.join("target/wasm32-unknown-unknown/release/rumi_protocol_backend.wasm")
        });
    std::fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "read freshly built release backend Wasm at {}: {error}",
            path.display()
        )
    })
}

fn boot() -> (PocketIc, Principal) {
    let pic = PocketIcBuilder::new().with_nns_subnet().build();
    let cid = pic.create_canister();
    pic.add_cycles(cid, 10_000_000_000_000);
    let init = ProtocolArg::Init(InitArg {
        xrc_principal: Principal::management_canister(),
        icusd_ledger_principal: Principal::from_slice(&[0x11; 29]),
        icp_ledger_principal: Principal::from_slice(&[0x22; 29]),
        fee_e8s: 10_000,
        developer_principal: developer(),
        treasury_principal: None,
        stability_pool_principal: None,
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    });
    pic.install_canister(cid, backend_wasm(), encode_args((init,)).unwrap(), None);
    (pic, cid)
}

fn register_chain(pic: &PocketIc, cid: Principal) {
    let arg = RegisterChainArg {
        chain_id: CFX,
        display_name: "Conflux eSpace mainnet".into(),
        rpc_endpoints: vec![
            SECRET_BEARING_URL.into(),
            "https://provider-b.example/v1".into(),
            SECRET_BEARING_URL.into(),
        ],
        finality_depth: 400,
        gas_strategy: GasStrategy::EvmEip1559 {
            max_priority_fee_gwei: 1,
            max_fee_gwei_ceiling: 100,
        },
        chain_native_decimals: 18,
        min_quorum_providers: Some(2),
    };
    let reply = pic
        .update_call(cid, developer(), "register_chain", encode_one(arg).unwrap())
        .expect("register_chain call");
    match reply {
        WasmResult::Reply(bytes) => {
            let result: Result<(), ProtocolError> = decode_one(&bytes).expect("register decode");
            result.expect("developer registration must succeed");
        }
        WasmResult::Reject(message) => panic!("register rejected: {message}"),
    }
}

/// Production recovery must use this side-effect-free replicated update export.
fn replicated_digest(
    pic: &PocketIc,
    cid: Principal,
    caller: Principal,
    chain: ChainId,
) -> (
    Vec<u8>,
    Result<ChainRpcEndpointSetDigestV1, ChainRpcEndpointDigestError>,
) {
    let reply = pic
        .update_call(
            cid,
            caller,
            "get_chain_rpc_endpoint_set_digest",
            encode_one(chain).unwrap(),
        )
        .expect("replicated digest call");
    match reply {
        WasmResult::Reply(bytes) => {
            let decoded = decode_one(&bytes).expect("digest decode");
            (bytes, decoded)
        }
        WasmResult::Reject(message) => panic!("replicated digest call rejected: {message}"),
    }
}

#[test]
fn digest_method_is_exported_only_for_replicated_update_execution() {
    let (pic, cid) = boot();

    let query_attempt = pic.query_call(
        cid,
        developer(),
        "get_chain_rpc_endpoint_set_digest",
        encode_one(CFX).unwrap(),
    );
    match query_attempt {
        Err(error) => {
            let message = format!("{error:?}");
            assert!(
                message.contains("CanisterMethodNotFound")
                    && message.contains("no query method 'get_chain_rpc_endpoint_set_digest'"),
                "unexpected rejection for query-only regression guard: {message}"
            );
        }
        Ok(WasmResult::Reject(message)) => assert!(
            message.contains("CanisterMethodNotFound")
                || message.contains("no query method 'get_chain_rpc_endpoint_set_digest'"),
            "unexpected canister rejection for query-only regression guard: {message}"
        ),
        Ok(WasmResult::Reply(_)) => {
            panic!("digest must not be exported on the uncertified query path")
        }
    }
}

#[test]
fn digest_update_is_developer_gated_and_has_a_missing_chain_error() {
    let (pic, cid) = boot();
    register_chain(&pic, cid);

    assert_eq!(
        replicated_digest(&pic, cid, outsider(), CFX).1,
        Err(ChainRpcEndpointDigestError::Unauthorized)
    );
    assert_eq!(
        replicated_digest(&pic, cid, developer(), ChainId(999_999)).1,
        Err(ChainRpcEndpointDigestError::ChainNotRegistered(ChainId(
            999_999
        )))
    );
}

#[test]
fn successful_digest_projects_count_and_quorum_without_raw_url_leakage() {
    let (pic, cid) = boot();
    register_chain(&pic, cid);

    let (wire, result) = replicated_digest(&pic, cid, developer(), CFX);
    let digest = result.expect("registered developer update must succeed");
    assert_eq!(digest.chain_id, CFX);
    assert_eq!(digest.endpoint_count, 2, "stored duplicates are collapsed");
    assert_eq!(digest.effective_min_quorum_providers, 2);
    assert_eq!(digest.digest_sha256.len(), 64);

    assert!(
        !wire
            .windows(SECRET_BEARING_URL.len())
            .any(|window| window == SECRET_BEARING_URL.as_bytes()),
        "the raw Candid reply must not contain endpoint URL bytes"
    );
    assert!(
        !format!("{digest:?}").contains("provider.example"),
        "the typed response must not expose endpoint URL text"
    );
}
