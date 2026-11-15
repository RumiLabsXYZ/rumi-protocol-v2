//! Deploy and drive foundry's `BridgedIcUSD` on a local anvil node.
//!
//! Everything is hand-rolled ABI encoding (all constructor args are static) so
//! the harness needs no ABI crate. Selectors and event topics are asserted
//! against both `keccak256` and (when available) `cast sig` in the unit tests at
//! the bottom of this file.

use super::anvil::Anvil;
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

pub const VERSION_STRING: &str = "rumi-base-icusd-bridge-v1";

// Function selectors (verified against `cast sig` by `selectors_match`).
pub const SEL_BRIDGE_OUT: [u8; 4] = [0x80, 0x34, 0x51, 0xee]; // bridgeOut(uint256,bytes,bytes32,bytes32)
pub const SEL_BALANCE_OF: [u8; 4] = [0x70, 0xa0, 0x82, 0x31]; // balanceOf(address)
pub const SEL_TOTAL_SUPPLY: [u8; 4] = [0x18, 0x16, 0x0d, 0xdd]; // totalSupply()
pub const SEL_BRIDGE_OUT_COUNT: [u8; 4] = [0x53, 0xeb, 0xd8, 0x18]; // bridgeOutCount()
pub const SEL_MINT_CONSUMED: [u8; 4] = [0x26, 0xc5, 0xb4, 0x0e]; // mintConsumed(bytes32)
pub const SEL_MINT: [u8; 4] = [0x1e, 0x45, 0x8b, 0xee]; // mint(address,uint256,bytes32)
pub const SEL_VERSION: [u8; 4] = [0xff, 0xa1, 0xad, 0x74]; // VERSION()
pub const SEL_COMPUTE_TRANSFER_ID: [u8; 4] = [0xae, 0xe7, 0x8e, 0x6a]; // computeTransferId(address,bytes32)

/// topic0 of `BridgeMint(bytes32 indexed mintId, address indexed to, uint256 amount)`.
pub const TOPIC_BRIDGE_MINT: &str =
    "0x07a55522f881c2542a34d1528374c51dba3994954e541d288d0cfd4fbc961dcc";
/// topic0 of `BridgeOut(bytes32 indexed transferId, address indexed from, uint256 amount,
/// uint64 sequence, bytes icpPrincipal, bytes32 icpSubaccount)`.
pub const TOPIC_BRIDGE_OUT: &str =
    "0x57c1323d2ddfd2271cfa8cd347d9cf1d9c8fdfe66100459f1f35561d847d3a4b";

// ---------------------------------------------------------------------------
// ABI word helpers
// ---------------------------------------------------------------------------

pub fn word_u128(v: u128) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[16..].copy_from_slice(&v.to_be_bytes());
    w
}

pub fn word_u64(v: u64) -> [u8; 32] {
    word_u128(v as u128)
}

/// Left-pad a 20-byte address (hex, with or without 0x) into a word.
pub fn word_addr(addr: &str) -> [u8; 32] {
    let raw = hex::decode(addr.trim_start_matches("0x"))
        .unwrap_or_else(|e| panic!("bad address {addr}: {e}"));
    assert_eq!(raw.len(), 20, "address {addr} is not 20 bytes");
    let mut w = [0u8; 32];
    w[12..].copy_from_slice(&raw);
    w
}

/// Deterministic bytes32 from a small integer (handy for mint ids / nonces).
pub fn bytes32_from_u64(v: u64) -> [u8; 32] {
    word_u64(v)
}

fn pad32(len: usize) -> usize {
    len.div_ceil(32) * 32
}

// ---------------------------------------------------------------------------
// Calldata encoders
// ---------------------------------------------------------------------------

/// `bridgeOut(uint256 amount, bytes icpPrincipal, bytes32 icpSubaccount, bytes32 clientNonce)`.
/// Head = amount | offset(0x80) | subaccount | nonce, tail = len | data padded to 32.
pub fn encode_bridge_out(
    amount: u128,
    principal: &[u8],
    subaccount: [u8; 32],
    client_nonce: [u8; 32],
) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&SEL_BRIDGE_OUT);
    out.extend_from_slice(&word_u128(amount));
    out.extend_from_slice(&word_u64(0x80));
    out.extend_from_slice(&subaccount);
    out.extend_from_slice(&client_nonce);
    out.extend_from_slice(&word_u64(principal.len() as u64));
    out.extend_from_slice(principal);
    out.extend(std::iter::repeat(0u8).take(pad32(principal.len()) - principal.len()));
    out
}

/// `mint(address to, uint256 amount, bytes32 mintId)`.
pub fn encode_mint(to: &str, amount: u128, mint_id: [u8; 32]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&SEL_MINT);
    out.extend_from_slice(&word_addr(to));
    out.extend_from_slice(&word_u128(amount));
    out.extend_from_slice(&mint_id);
    out
}

fn encode_call_addr(sel: [u8; 4], addr: &str) -> Vec<u8> {
    let mut out = sel.to_vec();
    out.extend_from_slice(&word_addr(addr));
    out
}

// ---------------------------------------------------------------------------
// Artifact loading
// ---------------------------------------------------------------------------

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR for the integration test = <root>/src/rumi_icusd_bridge.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn forge_bin() -> String {
    if let Ok(p) = std::env::var("FORGE_BIN") {
        return p;
    }
    format!(
        "{}/.foundry/bin/forge",
        std::env::var("HOME").expect("HOME not set")
    )
}

/// Load the creation bytecode from `foundry/out/BridgedIcUSD.sol/BridgedIcUSD.json`
/// (override the file with `BRIDGED_ICUSD_ARTIFACT`). If the artifact is missing
/// it runs `forge build --offline` inside `foundry/` (purely local).
pub fn load_creation_bytecode() -> Vec<u8> {
    let foundry_dir = workspace_root().join("foundry");
    let artifact = std::env::var("BRIDGED_ICUSD_ARTIFACT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| foundry_dir.join("out/BridgedIcUSD.sol/BridgedIcUSD.json"));
    if !artifact.exists() {
        let status = Command::new(forge_bin())
            .args(["build", "--offline"])
            .current_dir(&foundry_dir)
            .env("FOUNDRY_DISABLE_NIGHTLY_WARNING", "1")
            .status()
            .unwrap_or_else(|e| panic!("could not run forge build: {e}"));
        assert!(status.success(), "forge build --offline failed");
    }
    let text = std::fs::read_to_string(&artifact)
        .unwrap_or_else(|e| panic!("read {}: {e}", artifact.display()));
    let json: Value = serde_json::from_str(&text).expect("artifact is JSON");
    let obj = json["bytecode"]["object"]
        .as_str()
        .expect("bytecode.object");
    hex::decode(obj.trim_start_matches("0x")).expect("bytecode hex")
}

// ---------------------------------------------------------------------------
// Deployment
// ---------------------------------------------------------------------------

/// Constructor arguments. `admin` and `minter` must differ (the contract
/// enforces it).
#[derive(Clone, Debug)]
pub struct DeployArgs {
    pub admin: String,
    pub initial_admin_delay: u64,
    pub minter: String,
    /// Zero address when `None`.
    pub guardian: Option<String>,
    pub supply_cap: u128,
    pub mint_window_limit: u128,
    pub mint_window_seconds: u64,
    pub min_bridge_out: u128,
}

impl DeployArgs {
    /// Permissive defaults for harness tests: a huge cap and mint window, a
    /// one-day window length, and a minimum bridge-out of 1 base unit.
    pub fn permissive(admin: &str, minter: &str) -> DeployArgs {
        DeployArgs {
            admin: admin.to_string(),
            initial_admin_delay: 3 * 24 * 3600,
            minter: minter.to_string(),
            guardian: None,
            supply_cap: 1_000_000_000_00000000, // 1e9 icUSD
            mint_window_limit: 1_000_000_000_00000000,
            mint_window_seconds: 24 * 3600,
            min_bridge_out: 1,
        }
    }

    /// The 8 static ABI words of the constructor.
    pub fn encode(&self) -> Vec<u8> {
        let zero = "0x0000000000000000000000000000000000000000".to_string();
        let guardian = self.guardian.clone().unwrap_or(zero);
        let mut out = Vec::new();
        out.extend_from_slice(&word_addr(&self.admin));
        out.extend_from_slice(&word_u64(self.initial_admin_delay));
        out.extend_from_slice(&word_addr(&self.minter));
        out.extend_from_slice(&word_addr(&guardian));
        out.extend_from_slice(&word_u128(self.supply_cap));
        out.extend_from_slice(&word_u128(self.mint_window_limit));
        out.extend_from_slice(&word_u64(self.mint_window_seconds));
        out.extend_from_slice(&word_u128(self.min_bridge_out));
        assert_eq!(out.len(), 8 * 32);
        out
    }
}

/// A decoded `BridgeOut` log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BridgeOutEvent {
    pub transfer_id: [u8; 32],
    pub from: String,
    pub amount: u128,
    pub sequence: u64,
    pub icp_principal: Vec<u8>,
    pub icp_subaccount: [u8; 32],
}

/// A deployed `BridgedIcUSD`.
#[derive(Clone, Debug)]
pub struct BridgedIcUsd {
    /// Lowercase 0x-prefixed contract address.
    pub address: String,
    pub deploy_tx: String,
}

impl BridgedIcUsd {
    /// Deploy from `deployer` (an anvil unlocked account). The deployer keeps no
    /// authority unless it is also passed as `admin`.
    pub fn deploy(anvil: &Anvil, deployer: &str, args: &DeployArgs) -> BridgedIcUsd {
        let mut data = load_creation_bytecode();
        data.extend_from_slice(&args.encode());
        let tx = anvil.send_tx_from_unlocked(deployer, None, &data, 0);
        let receipt = anvil
            .receipt(&tx)
            .expect("deploy receipt (anvil automines)");
        assert_eq!(
            receipt["status"], "0x1",
            "BridgedIcUSD deployment reverted: {receipt}"
        );
        let address = receipt["contractAddress"]
            .as_str()
            .expect("contractAddress in deploy receipt")
            .to_lowercase();
        BridgedIcUsd {
            address,
            deploy_tx: tx,
        }
    }

    // ----- views -----

    /// `VERSION()` decoded from the ABI string return.
    pub fn version(&self, anvil: &Anvil) -> String {
        decode_abi_string(&anvil.eth_call(&self.address, &SEL_VERSION))
    }

    pub fn balance_of(&self, anvil: &Anvil, who: &str) -> u128 {
        decode_u128(&anvil.eth_call(&self.address, &encode_call_addr(SEL_BALANCE_OF, who)))
    }

    pub fn total_supply(&self, anvil: &Anvil) -> u128 {
        decode_u128(&anvil.eth_call(&self.address, &SEL_TOTAL_SUPPLY))
    }

    pub fn bridge_out_count(&self, anvil: &Anvil) -> u64 {
        decode_u128(&anvil.eth_call(&self.address, &SEL_BRIDGE_OUT_COUNT)) as u64
    }

    pub fn mint_consumed(&self, anvil: &Anvil, mint_id: [u8; 32]) -> bool {
        let mut data = SEL_MINT_CONSUMED.to_vec();
        data.extend_from_slice(&mint_id);
        decode_u128(&anvil.eth_call(&self.address, &data)) != 0
    }

    /// `computeTransferId(from, clientNonce)`.
    pub fn compute_transfer_id(
        &self,
        anvil: &Anvil,
        from: &str,
        client_nonce: [u8; 32],
    ) -> [u8; 32] {
        let mut data = SEL_COMPUTE_TRANSFER_ID.to_vec();
        data.extend_from_slice(&word_addr(from));
        data.extend_from_slice(&client_nonce);
        let out = anvil.eth_call(&self.address, &data);
        out.try_into().expect("bytes32 return")
    }

    // ----- transactions (from anvil unlocked accounts) -----

    /// Holder-side `bridgeOut`, sent from an unlocked account. A reverting call
    /// is mined with a failed receipt (see `Anvil::send_tx_from_unlocked`); this
    /// only panics if the node refuses the transaction outright.
    pub fn bridge_out(
        &self,
        anvil: &Anvil,
        from_unlocked: &str,
        amount: u128,
        principal_bytes: &[u8],
        subaccount: [u8; 32],
        client_nonce: [u8; 32],
    ) -> String {
        self.try_bridge_out(
            anvil,
            from_unlocked,
            amount,
            principal_bytes,
            subaccount,
            client_nonce,
        )
        .unwrap_or_else(|e| panic!("bridgeOut from {from_unlocked} failed: {e}"))
    }

    pub fn try_bridge_out(
        &self,
        anvil: &Anvil,
        from_unlocked: &str,
        amount: u128,
        principal_bytes: &[u8],
        subaccount: [u8; 32],
        client_nonce: [u8; 32],
    ) -> Result<String, String> {
        let data = encode_bridge_out(amount, principal_bytes, subaccount, client_nonce);
        anvil.try_send_tx_from_unlocked(from_unlocked, Some(&self.address), &data, 0, None)
    }

    /// Raw `mint` sent from an unlocked account. For harness self-tests only:
    /// the real bridge mints with a canister-signed transaction, not an unlocked
    /// anvil account.
    pub fn mint_from_unlocked(
        &self,
        anvil: &Anvil,
        minter: &str,
        to: &str,
        amount: u128,
        mint_id: [u8; 32],
    ) -> String {
        let data = encode_mint(to, amount, mint_id);
        anvil.send_tx_from_unlocked(minter, Some(&self.address), &data, 0)
    }
}

// ---------------------------------------------------------------------------
// Return / log decoding
// ---------------------------------------------------------------------------

pub fn decode_u128(ret: &[u8]) -> u128 {
    assert!(
        ret.len() >= 32,
        "return data shorter than one word: {} bytes",
        ret.len()
    );
    assert!(
        ret[..16].iter().all(|b| *b == 0),
        "value does not fit in u128"
    );
    u128::from_be_bytes(ret[16..32].try_into().unwrap())
}

/// Decode a single ABI-encoded `string` return value.
pub fn decode_abi_string(ret: &[u8]) -> String {
    let offset = decode_u128(&ret[0..32]) as usize;
    let len = decode_u128(&ret[offset..offset + 32]) as usize;
    String::from_utf8(ret[offset + 32..offset + 32 + len].to_vec()).expect("utf8 string")
}

fn log_topics(log: &Value) -> Vec<String> {
    log["topics"]
        .as_array()
        .expect("topics array")
        .iter()
        .map(|t| t.as_str().unwrap().to_lowercase())
        .collect()
}

fn log_data(log: &Value) -> Vec<u8> {
    hex::decode(log["data"].as_str().expect("data").trim_start_matches("0x")).expect("data hex")
}

/// Logs of a receipt whose first topic equals `topic0` and whose emitter is `address`.
pub fn logs_with_topic<'a>(receipt: &'a Value, address: &str, topic0: &str) -> Vec<&'a Value> {
    receipt["logs"]
        .as_array()
        .expect("logs array")
        .iter()
        .filter(|l| {
            l["address"]
                .as_str()
                .map(|a| a.eq_ignore_ascii_case(address))
                .unwrap_or(false)
                && log_topics(l).first().map(|t| t == topic0).unwrap_or(false)
        })
        .collect()
}

/// Decode a `BridgeOut` log (topics: id, from; data: amount, sequence, offset,
/// subaccount, then the dynamic `bytes`).
pub fn decode_bridge_out(log: &Value) -> BridgeOutEvent {
    let topics = log_topics(log);
    assert_eq!(topics[0], TOPIC_BRIDGE_OUT, "not a BridgeOut log");
    let data = log_data(log);
    let amount = decode_u128(&data[0..32]);
    let sequence = decode_u128(&data[32..64]) as u64;
    let offset = decode_u128(&data[64..96]) as usize;
    let icp_subaccount: [u8; 32] = data[96..128].try_into().unwrap();
    let len = decode_u128(&data[offset..offset + 32]) as usize;
    let icp_principal = data[offset + 32..offset + 32 + len].to_vec();
    BridgeOutEvent {
        transfer_id: hex::decode(topics[1].trim_start_matches("0x"))
            .unwrap()
            .try_into()
            .unwrap(),
        from: format!("0x{}", &topics[2][26..]),
        amount,
        sequence,
        icp_principal,
        icp_subaccount,
    }
}

/// Decode a `BridgeMint` log into `(mintId, to, amount)`.
pub fn decode_bridge_mint(log: &Value) -> ([u8; 32], String, u128) {
    let topics = log_topics(log);
    assert_eq!(topics[0], TOPIC_BRIDGE_MINT, "not a BridgeMint log");
    let id: [u8; 32] = hex::decode(topics[1].trim_start_matches("0x"))
        .unwrap()
        .try_into()
        .unwrap();
    let to = format!("0x{}", &topics[2][26..]);
    (id, to, decode_u128(&log_data(log)[0..32]))
}

// ---------------------------------------------------------------------------
// Unit tests (run inside every integration test binary that includes `mod support;`)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use sha3::{Digest, Keccak256};

    fn keccak(s: &str) -> [u8; 32] {
        let mut h = Keccak256::new();
        h.update(s.as_bytes());
        h.finalize().into()
    }

    fn selector(sig: &str) -> [u8; 4] {
        keccak(sig)[..4].try_into().unwrap()
    }

    #[test]
    fn selectors_match() {
        let table: [(&str, [u8; 4]); 8] = [
            ("bridgeOut(uint256,bytes,bytes32,bytes32)", SEL_BRIDGE_OUT),
            ("balanceOf(address)", SEL_BALANCE_OF),
            ("totalSupply()", SEL_TOTAL_SUPPLY),
            ("bridgeOutCount()", SEL_BRIDGE_OUT_COUNT),
            ("mintConsumed(bytes32)", SEL_MINT_CONSUMED),
            ("mint(address,uint256,bytes32)", SEL_MINT),
            ("VERSION()", SEL_VERSION),
            (
                "computeTransferId(address,bytes32)",
                SEL_COMPUTE_TRANSFER_ID,
            ),
        ];
        let cast = format!(
            "{}/.foundry/bin/cast",
            std::env::var("HOME").unwrap_or_default()
        );
        for (sig, want) in table {
            assert_eq!(selector(sig), want, "keccak selector mismatch for {sig}");
            // Cross-check with `cast sig` when foundry is installed (offline, local).
            if let Ok(out) = Command::new(&cast)
                .args(["sig", sig])
                .env("FOUNDRY_DISABLE_NIGHTLY_WARNING", "1")
                .output()
            {
                if out.status.success() {
                    let s = String::from_utf8_lossy(&out.stdout);
                    let hex_sel = s
                        .lines()
                        .map(str::trim)
                        .find(|l| l.starts_with("0x"))
                        .expect("cast sig output");
                    assert_eq!(
                        hex_sel,
                        format!("0x{}", hex::encode(want)),
                        "cast sig mismatch for {sig}"
                    );
                }
            }
        }
    }

    #[test]
    fn event_topics_match() {
        assert_eq!(
            format!(
                "0x{}",
                hex::encode(keccak("BridgeMint(bytes32,address,uint256)"))
            ),
            TOPIC_BRIDGE_MINT
        );
        assert_eq!(
            format!(
                "0x{}",
                hex::encode(keccak(
                    "BridgeOut(bytes32,address,uint256,uint64,bytes,bytes32)"
                ))
            ),
            TOPIC_BRIDGE_OUT
        );
    }

    #[test]
    fn bridge_out_encoding_layout() {
        let principal = [0xaa; 10];
        let data = encode_bridge_out(5, &principal, [1u8; 32], [2u8; 32]);
        // selector + 4 head words + length word + one padded data word
        assert_eq!(data.len(), 4 + 4 * 32 + 32 + 32);
        assert_eq!(
            &data[4 + 32..4 + 64],
            &word_u64(0x80)[..],
            "bytes offset must point past the 4 head words"
        );
        assert_eq!(decode_u128(&data[4 + 4 * 32..4 + 5 * 32]), 10);
        assert!(
            data[4 + 5 * 32 + 10..].iter().all(|b| *b == 0),
            "padding must be zero"
        );
        // A 29-byte principal pads to exactly one word, a 32-byte-multiple adds none.
        assert_eq!(
            encode_bridge_out(1, &[1u8; 29], [0; 32], [0; 32]).len(),
            4 + 4 * 32 + 32 + 32
        );
        assert_eq!(
            encode_bridge_out(1, &[1u8; 1], [0; 32], [0; 32]).len(),
            4 + 4 * 32 + 32 + 32
        );
    }

    #[test]
    fn constructor_is_eight_words() {
        let a = DeployArgs::permissive(
            "0x1111111111111111111111111111111111111111",
            "0x2222222222222222222222222222222222222222",
        );
        assert_eq!(a.encode().len(), 256);
    }
}
