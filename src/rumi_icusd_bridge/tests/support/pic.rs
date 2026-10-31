//! PocketIC construction and canister-install helpers.
//!
//! The PocketIC server binary comes from `POCKET_IC_BIN` (required, and it must
//! be an absolute path: the server is spawned from the cargo target directory).
//! Wasm modules are read from disk at runtime (never `include_bytes!`), so a
//! test binary never goes stale against a rebuilt canister:
//!
//! * `RELAY_WASM`  : the `evm_rpc_relay` wasm   (default: `<target>/wasm32-unknown-unknown/release/evm_rpc_relay.wasm`)
//! * `BRIDGE_WASM` : the bridge canister wasm   (default: `<target>/wasm32-unknown-unknown/release/rumi_icusd_bridge.wasm`)
//!
//! `<target>` is `$CARGO_TARGET_DIR` when set, else `<workspace>/target`.

use candid::Principal;
use pocket_ic::{PocketIc, PocketIcBuilder};
use std::path::PathBuf;

/// Cycles every harness canister is funded with (100T).
pub const DEFAULT_CYCLES: u128 = 100_000_000_000_000;

pub fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn target_dir() -> PathBuf {
    std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| workspace_root().join("target"))
}

/// Build a PocketIC instance with an II subnet (which hosts the threshold-ECDSA
/// keys) and an application subnet (where harness canisters are created),
/// mirroring the working setup of `conflux_espace_happy_path_pic.rs` and
/// `phase1b_supply_gate_pic.rs` in the backend tests.
///
/// Panics with a clear message if `POCKET_IC_BIN` is unset or not a file.
pub fn build_pic() -> PocketIc {
    let bin = std::env::var("POCKET_IC_BIN")
        .expect("POCKET_IC_BIN must be set to the absolute path of the pocket-ic server binary");
    assert!(
        std::path::Path::new(&bin).is_file(),
        "POCKET_IC_BIN={bin} is not a file (it must be an absolute path to the pocket-ic server)"
    );
    PocketIcBuilder::new()
        .with_ii_subnet()
        .with_application_subnet()
        .build()
}

/// Read a wasm from `$env_var` (if set) or `default_path`.
pub fn read_wasm(env_var: &str, default_path: PathBuf) -> Vec<u8> {
    let path = std::env::var(env_var)
        .map(PathBuf::from)
        .unwrap_or(default_path);
    std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read wasm {} ({e}). Build it first, or set {env_var}.",
            path.display()
        )
    })
}

pub fn relay_wasm() -> Vec<u8> {
    read_wasm(
        "RELAY_WASM",
        target_dir().join("wasm32-unknown-unknown/release/evm_rpc_relay.wasm"),
    )
}

pub fn bridge_wasm() -> Vec<u8> {
    read_wasm(
        "BRIDGE_WASM",
        target_dir().join("wasm32-unknown-unknown/release/rumi_icusd_bridge.wasm"),
    )
}

/// Create a canister on the application subnet and fund it.
pub fn create_funded_canister(pic: &PocketIc, cycles: u128) -> Principal {
    let id = pic.create_canister();
    pic.add_cycles(id, cycles);
    id
}

/// Create a funded canister and install `wasm` with the Candid-encoded `init_arg`.
pub fn install_with_cycles(
    pic: &PocketIc,
    wasm: Vec<u8>,
    init_arg: Vec<u8>,
    cycles: u128,
) -> Principal {
    let id = create_funded_canister(pic, cycles);
    pic.install_canister(id, wasm, init_arg, None);
    id
}

/// Install `wasm` at a specific canister id (for example the production EVM RPC
/// canister id, so the bridge's compiled-in default principal resolves to the
/// relay). PocketIC creates a fresh subnet when the id belongs to no existing
/// subnet range, and returns an error if the id is already in use.
pub fn install_at_id(
    pic: &PocketIc,
    id: Principal,
    wasm: Vec<u8>,
    init_arg: Vec<u8>,
    cycles: u128,
) -> Principal {
    let created = pic
        .create_canister_with_id(None, None, id)
        .unwrap_or_else(|e| panic!("create_canister_with_id({id}) failed: {e}"));
    pic.add_cycles(created, cycles);
    pic.install_canister(created, wasm, init_arg, None);
    created
}

/// Deploy the `evm_rpc_relay` test canister (empty init arg).
pub fn deploy_relay(pic: &PocketIc) -> Principal {
    install_with_cycles(
        pic,
        relay_wasm(),
        candid::encode_args(()).expect("encode empty init"),
        DEFAULT_CYCLES,
    )
}

/// A fixed non-anonymous test principal (the "user" in harness tests).
pub fn test_principal(tag: u8) -> Principal {
    Principal::from_slice(&[tag, 0xab, 0xcd, 0x01])
}
