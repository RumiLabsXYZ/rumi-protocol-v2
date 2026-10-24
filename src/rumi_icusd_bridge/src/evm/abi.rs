//! Owned by card BB-04 (EVM primitives). Contract ABI: calldata encoding and strict
//! log / return-data decoding for `BridgedIcUSD`.
//!
//! Every decoder here is deliberately strict: it accepts exactly the canonical
//! encoding the Base contract produces and nothing else. Return data and log data come
//! from RPC providers that the canister does not trust individually, so a decoder never
//! "repairs" or skips a malformed field. A rejection is an `AbiError`, never a guess.
//!
//! Pinned values (selectors, topics, role ids) are asserted in the tests below by
//! recomputing keccak from the signature strings, and match
//! `foundry/test/BridgedIcUSD.t.sol` (`test_abi_pin_*`).

use crate::ids::{word_address, word_u64};
use crate::types::ContractAdminAction;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AbiError {
    Malformed(String),
}

fn err<T>(msg: impl Into<String>) -> Result<T, AbiError> {
    Err(AbiError::Malformed(msg.into()))
}

/// A log as returned by `eth_getLogs` / receipts, already hex-decoded.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RawLog {
    pub address: [u8; 20],
    pub topics: Vec<[u8; 32]>,
    pub data: Vec<u8>,
    pub block_number: u64,
    pub block_hash: [u8; 32],
    pub tx_hash: [u8; 32],
    pub log_index: u64,
    pub removed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BridgeOutEvent {
    pub transfer_id: [u8; 32],
    pub from: [u8; 20],
    pub amount_e8s: u64,
    pub sequence: u64,
    pub icp_principal: Vec<u8>,
    pub icp_subaccount: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BridgeMintEvent {
    pub mint_id: [u8; 32],
    pub to: [u8; 20],
    pub amount_e8s: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewCall {
    Version,
    Decimals,
    TotalSupply,
    SupplyCap,
    MintWindowLimit,
    MintWindowSeconds,
    MinBridgeOut,
    MintPaused,
    BridgeOutPaused,
    BridgeOutCount,
    MintConsumed([u8; 32]),
    BridgeOutUsed([u8; 32]),
    HasRole { role: [u8; 32], account: [u8; 20] },
    DefaultAdmin,
}

// ---------------------------------------------------------------------------
// Pinned constants
// ---------------------------------------------------------------------------

const fn hex_nibble(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => panic!("invalid hex digit"),
    }
}

/// Compile-time parse of a 64-digit hex string (no `0x`). A typo fails the build.
const fn hex32(s: &str) -> [u8; 32] {
    let b = s.as_bytes();
    assert!(b.len() == 64, "expected 64 hex digits");
    let mut out = [0u8; 32];
    let mut i = 0;
    while i < 32 {
        out[i] = (hex_nibble(b[2 * i]) << 4) | hex_nibble(b[2 * i + 1]);
        i += 1;
    }
    out
}

/// `keccak256("BridgeOut(bytes32,address,uint256,uint64,bytes,bytes32)")`.
pub const BRIDGE_OUT_TOPIC0: [u8; 32] = hex32("57c1323d2ddfd2271cfa8cd347d9cf1d9c8fdfe66100459f1f35561d847d3a4b");
/// `keccak256("BridgeMint(bytes32,address,uint256)")`.
pub const BRIDGE_MINT_TOPIC0: [u8; 32] = hex32("07a55522f881c2542a34d1528374c51dba3994954e541d288d0cfd4fbc961dcc");

/// `keccak256("MINTER_ROLE")`.
pub const MINTER_ROLE: [u8; 32] = hex32("9f2df0fed2c77648de5860a4cc508cd0818c85b8b8a1ab4ceeef8d981c8956a6");
/// `keccak256("PAUSER_ROLE")`.
pub const PAUSER_ROLE: [u8; 32] = hex32("65d7a28e3265b37a6474929f336521b332c1681b933f6cb9f3376673440d862a");
/// OpenZeppelin `DEFAULT_ADMIN_ROLE` is the zero word.
pub const DEFAULT_ADMIN_ROLE: [u8; 32] = [0u8; 32];

pub const SEL_MINT: [u8; 4] = 0x1e45_8bee_u32.to_be_bytes();
pub const SEL_PAUSE_MINT: [u8; 4] = 0xcd85_cdb5_u32.to_be_bytes();
pub const SEL_UNPAUSE_MINT: [u8; 4] = 0x1a8b_d2da_u32.to_be_bytes();
pub const SEL_PAUSE_BRIDGE_OUT: [u8; 4] = 0x2f1d_448f_u32.to_be_bytes();
pub const SEL_UNPAUSE_BRIDGE_OUT: [u8; 4] = 0x7b45_5e06_u32.to_be_bytes();
pub const SEL_SET_SUPPLY_CAP: [u8; 4] = 0xb6a3_f59a_u32.to_be_bytes();
pub const SEL_SET_MINT_RATE_LIMIT: [u8; 4] = 0x1a5b_7916_u32.to_be_bytes();
pub const SEL_SET_MIN_BRIDGE_OUT: [u8; 4] = 0xccf8_4ef8_u32.to_be_bytes();
pub const SEL_MINT_CONSUMED: [u8; 4] = 0x26c5_b40e_u32.to_be_bytes();
pub const SEL_BRIDGE_OUT_USED: [u8; 4] = 0x3e94_2d93_u32.to_be_bytes();
pub const SEL_BRIDGE_OUT_COUNT: [u8; 4] = 0x53eb_d818_u32.to_be_bytes();
pub const SEL_TOTAL_SUPPLY: [u8; 4] = 0x1816_0ddd_u32.to_be_bytes();
pub const SEL_SUPPLY_CAP: [u8; 4] = 0x8f77_0ad0_u32.to_be_bytes();
pub const SEL_VERSION: [u8; 4] = 0xffa1_ad74_u32.to_be_bytes();
pub const SEL_HAS_ROLE: [u8; 4] = 0x91d1_4854_u32.to_be_bytes();
pub const SEL_DEFAULT_ADMIN: [u8; 4] = 0x84ef_8ffc_u32.to_be_bytes();
pub const SEL_DECIMALS: [u8; 4] = 0x313c_e567_u32.to_be_bytes();
pub const SEL_MINT_PAUSED: [u8; 4] = 0x7e48_31d3_u32.to_be_bytes();
pub const SEL_BRIDGE_OUT_PAUSED: [u8; 4] = 0xd948_e8ec_u32.to_be_bytes();
pub const SEL_MIN_BRIDGE_OUT: [u8; 4] = 0xd88a_9ba8_u32.to_be_bytes();
pub const SEL_MINT_WINDOW_LIMIT: [u8; 4] = 0xfeaf_a875_u32.to_be_bytes();
pub const SEL_MINT_WINDOW_SECONDS: [u8; 4] = 0x2470_4279_u32.to_be_bytes();

/// Exact byte length of a `BridgeOut` log's data: six 32-byte words
/// (amount, sequence, offset, subaccount, principal length, principal bytes).
pub const BRIDGE_OUT_DATA_LEN: usize = 6 * 32;
/// The only offset the contract's `abi.encode` produces for the dynamic `bytes` field.
pub const BRIDGE_OUT_PRINCIPAL_OFFSET: u64 = 0x80;
/// Maximum length of a decoded ABI string return value (`VERSION()` is 25 bytes).
pub const MAX_ABI_STRING_LEN: usize = 1024;

// ---------------------------------------------------------------------------
// Calldata encoding
// ---------------------------------------------------------------------------

fn call(selector: &[u8; 4], words: &[[u8; 32]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + 32 * words.len());
    out.extend_from_slice(selector);
    for w in words {
        out.extend_from_slice(w);
    }
    out
}

/// `mint(address to, uint256 amount, bytes32 mintId)`.
pub fn encode_mint(to: &[u8; 20], amount_e8s: u64, mint_id: &[u8; 32]) -> Vec<u8> {
    call(&SEL_MINT, &[word_address(to), word_u64(amount_e8s), *mint_id])
}

/// Calldata for an admin / pauser action.
pub fn encode_admin_action(a: &ContractAdminAction) -> Vec<u8> {
    match a {
        ContractAdminAction::PauseMint => call(&SEL_PAUSE_MINT, &[]),
        ContractAdminAction::UnpauseMint => call(&SEL_UNPAUSE_MINT, &[]),
        ContractAdminAction::PauseBridgeOut => call(&SEL_PAUSE_BRIDGE_OUT, &[]),
        ContractAdminAction::UnpauseBridgeOut => call(&SEL_UNPAUSE_BRIDGE_OUT, &[]),
        ContractAdminAction::SetSupplyCap { cap_e8s } => call(&SEL_SET_SUPPLY_CAP, &[word_u64(*cap_e8s)]),
        ContractAdminAction::SetMintRateLimit { limit_e8s, window_seconds } => {
            // setMintRateLimit(uint256 limit, uint64 windowSeconds): each argument is
            // one full word (a uint64 is left-padded to 32 bytes in the ABI).
            call(&SEL_SET_MINT_RATE_LIMIT, &[word_u64(*limit_e8s), word_u64(*window_seconds)])
        }
        ContractAdminAction::SetMinBridgeOut { minimum_e8s } => call(&SEL_SET_MIN_BRIDGE_OUT, &[word_u64(*minimum_e8s)]),
    }
}

/// Calldata for a read-only `eth_call`.
pub fn encode_view(v: &ViewCall) -> Vec<u8> {
    match v {
        ViewCall::Version => call(&SEL_VERSION, &[]),
        ViewCall::Decimals => call(&SEL_DECIMALS, &[]),
        ViewCall::TotalSupply => call(&SEL_TOTAL_SUPPLY, &[]),
        ViewCall::SupplyCap => call(&SEL_SUPPLY_CAP, &[]),
        ViewCall::MintWindowLimit => call(&SEL_MINT_WINDOW_LIMIT, &[]),
        ViewCall::MintWindowSeconds => call(&SEL_MINT_WINDOW_SECONDS, &[]),
        ViewCall::MinBridgeOut => call(&SEL_MIN_BRIDGE_OUT, &[]),
        ViewCall::MintPaused => call(&SEL_MINT_PAUSED, &[]),
        ViewCall::BridgeOutPaused => call(&SEL_BRIDGE_OUT_PAUSED, &[]),
        ViewCall::BridgeOutCount => call(&SEL_BRIDGE_OUT_COUNT, &[]),
        ViewCall::MintConsumed(id) => call(&SEL_MINT_CONSUMED, &[*id]),
        ViewCall::BridgeOutUsed(id) => call(&SEL_BRIDGE_OUT_USED, &[*id]),
        ViewCall::HasRole { role, account } => call(&SEL_HAS_ROLE, &[*role, word_address(account)]),
        ViewCall::DefaultAdmin => call(&SEL_DEFAULT_ADMIN, &[]),
    }
}

// ---------------------------------------------------------------------------
// Return-data decoding
// ---------------------------------------------------------------------------

fn is_zero(b: &[u8]) -> bool {
    b.iter().all(|&x| x == 0)
}

fn single_word(ret: &[u8], what: &str) -> Result<[u8; 32], AbiError> {
    <[u8; 32]>::try_from(ret).or_else(|_| err(format!("{what}: return data must be exactly 32 bytes, got {}", ret.len())))
}

/// A `uint256` return value that must fit in `u128`.
pub fn decode_uint_u128(ret: &[u8]) -> Result<u128, AbiError> {
    let w = single_word(ret, "uint")?;
    if !is_zero(&w[..16]) {
        return err("uint value exceeds u128");
    }
    let mut lo = [0u8; 16];
    lo.copy_from_slice(&w[16..]);
    Ok(u128::from_be_bytes(lo))
}

/// A `bool` return value: the word must be exactly 0 or 1.
pub fn decode_bool(ret: &[u8]) -> Result<bool, AbiError> {
    let w = single_word(ret, "bool")?;
    if !is_zero(&w[..31]) || w[31] > 1 {
        return err("bool word must be exactly 0 or 1");
    }
    Ok(w[31] == 1)
}

/// An `address` return value: the upper 12 bytes must be zero.
pub fn decode_address(ret: &[u8]) -> Result<[u8; 20], AbiError> {
    let w = single_word(ret, "address")?;
    if !is_zero(&w[..12]) {
        return err("address word has non-zero upper bytes");
    }
    let mut a = [0u8; 20];
    a.copy_from_slice(&w[12..]);
    Ok(a)
}

/// A single `string` return value in the canonical `abi.encode(string)` layout:
/// `offset (0x20) | length | bytes | zero padding to 32`. Anything else is rejected.
pub fn decode_string(ret: &[u8]) -> Result<String, AbiError> {
    // Bitmask instead of `% 32` / `is_multiple_of` to stay clear of toolchain-dependent lints.
    if ret.len() < 64 || ret.len() & 31 != 0 {
        return err(format!("string: bad return data length {}", ret.len()));
    }
    if ret[..32] != word_u64(0x20) {
        return err("string: offset word must be 0x20");
    }
    let len_word = &ret[32..64];
    if !is_zero(&len_word[..24]) {
        return err("string: length word does not fit u64");
    }
    let len = u64::from_be_bytes(len_word[24..].try_into().expect("8 bytes"));
    if len > MAX_ABI_STRING_LEN as u64 {
        return err(format!("string: length {len} exceeds {MAX_ABI_STRING_LEN}"));
    }
    let len = len as usize;
    let padded = len.div_ceil(32) * 32;
    if ret.len() != 64 + padded {
        return err("string: non-canonical total length");
    }
    if !is_zero(&ret[64 + len..]) {
        return err("string: non-zero padding");
    }
    String::from_utf8(ret[64..64 + len].to_vec()).or_else(|_| err("string: not valid UTF-8"))
}

// ---------------------------------------------------------------------------
// Log decoding
// ---------------------------------------------------------------------------

/// Checks common to every event we decode: not removed, emitted by the bound contract,
/// exactly three topics, the expected topic0, and an address-shaped topic2. Returns
/// `(topic1, topic2 as address)`.
fn check_envelope(log: &RawLog, contract: &[u8; 20], topic0: &[u8; 32], name: &str) -> Result<([u8; 32], [u8; 20]), AbiError> {
    if log.removed {
        return err(format!("{name}: log is marked removed"));
    }
    if &log.address != contract {
        return err(format!("{name}: log not emitted by the bound contract"));
    }
    if log.topics.len() != 3 {
        return err(format!("{name}: expected exactly 3 topics, got {}", log.topics.len()));
    }
    if &log.topics[0] != topic0 {
        return err(format!("{name}: wrong topic0"));
    }
    if !is_zero(&log.topics[2][..12]) {
        return err(format!("{name}: topic2 is not a clean address word"));
    }
    let mut addr = [0u8; 20];
    addr.copy_from_slice(&log.topics[2][12..]);
    Ok((log.topics[1], addr))
}

/// A `uint256` word that must fit in `u64`.
fn word_to_u64(w: &[u8], what: &str) -> Result<u64, AbiError> {
    if !is_zero(&w[..24]) {
        return err(format!("{what} does not fit u64"));
    }
    Ok(u64::from_be_bytes(w[24..32].try_into().expect("8 bytes")))
}

/// Decode a `BridgeOut` log.
///
/// Data layout (asserted byte for byte by the contract tests), 192 bytes:
/// `amount | sequence | offset (0x80) | icpSubaccount | principal length | principal
/// bytes right-padded with zeros`.
pub fn decode_bridge_out(log: &RawLog, contract: &[u8; 20]) -> Result<BridgeOutEvent, AbiError> {
    let (transfer_id, from) = check_envelope(log, contract, &BRIDGE_OUT_TOPIC0, "BridgeOut")?;
    if log.data.len() != BRIDGE_OUT_DATA_LEN {
        return err(format!("BridgeOut: data must be {BRIDGE_OUT_DATA_LEN} bytes, got {}", log.data.len()));
    }
    let d = &log.data;
    let amount_e8s = word_to_u64(&d[0..32], "BridgeOut amount")?;
    if amount_e8s == 0 {
        return err("BridgeOut: zero amount");
    }
    let sequence = word_to_u64(&d[32..64], "BridgeOut sequence")?;
    if sequence == 0 {
        return err("BridgeOut: zero sequence");
    }
    let offset = word_to_u64(&d[64..96], "BridgeOut offset")?;
    if offset != BRIDGE_OUT_PRINCIPAL_OFFSET {
        return err(format!("BridgeOut: principal offset must be 0x80, got {offset:#x}"));
    }
    let mut icp_subaccount = [0u8; 32];
    icp_subaccount.copy_from_slice(&d[96..128]);
    let principal_len = word_to_u64(&d[128..160], "BridgeOut principal length")?;
    if !(1..=29).contains(&principal_len) {
        return err(format!("BridgeOut: principal length {principal_len} outside 1..=29"));
    }
    let n = principal_len as usize;
    let principal_word = &d[160..192];
    if !is_zero(&principal_word[n..]) {
        return err("BridgeOut: non-zero principal padding");
    }
    let icp_principal = principal_word[..n].to_vec();
    if icp_principal == [0x04] {
        return err("BridgeOut: anonymous principal");
    }
    Ok(BridgeOutEvent { transfer_id, from, amount_e8s, sequence, icp_principal, icp_subaccount })
}

/// Decode a `BridgeMint` log (data is the single `amount` word).
pub fn decode_bridge_mint(log: &RawLog, contract: &[u8; 20]) -> Result<BridgeMintEvent, AbiError> {
    let (mint_id, to) = check_envelope(log, contract, &BRIDGE_MINT_TOPIC0, "BridgeMint")?;
    if log.data.len() != 32 {
        return err(format!("BridgeMint: data must be 32 bytes, got {}", log.data.len()));
    }
    let amount_e8s = word_to_u64(&log.data, "BridgeMint amount")?;
    if amount_e8s == 0 {
        return err("BridgeMint: zero amount");
    }
    Ok(BridgeMintEvent { mint_id, to, amount_e8s })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::keccak256;

    const CONTRACT: [u8; 20] = [0xc0; 20];

    fn h(s: &str) -> Vec<u8> {
        hex::decode(s.trim_start_matches("0x")).unwrap()
    }

    fn sel(sig: &str) -> [u8; 4] {
        let k = keccak256(sig.as_bytes());
        [k[0], k[1], k[2], k[3]]
    }

    // ----- pinned constants ---------------------------------------------------

    #[test]
    fn topics_match_keccak_of_signatures() {
        assert_eq!(BRIDGE_OUT_TOPIC0, keccak256(b"BridgeOut(bytes32,address,uint256,uint64,bytes,bytes32)"));
        assert_eq!(BRIDGE_MINT_TOPIC0, keccak256(b"BridgeMint(bytes32,address,uint256)"));
        assert_eq!(
            hex::encode(BRIDGE_OUT_TOPIC0),
            "57c1323d2ddfd2271cfa8cd347d9cf1d9c8fdfe66100459f1f35561d847d3a4b"
        );
        assert_eq!(
            hex::encode(BRIDGE_MINT_TOPIC0),
            "07a55522f881c2542a34d1528374c51dba3994954e541d288d0cfd4fbc961dcc"
        );
    }

    #[test]
    fn role_ids_match_keccak() {
        assert_eq!(MINTER_ROLE, keccak256(b"MINTER_ROLE"));
        assert_eq!(PAUSER_ROLE, keccak256(b"PAUSER_ROLE"));
        assert_eq!(DEFAULT_ADMIN_ROLE, [0u8; 32]);
        assert_eq!(
            hex::encode(MINTER_ROLE),
            "9f2df0fed2c77648de5860a4cc508cd0818c85b8b8a1ab4ceeef8d981c8956a6"
        );
        assert_eq!(
            hex::encode(PAUSER_ROLE),
            "65d7a28e3265b37a6474929f336521b332c1681b933f6cb9f3376673440d862a"
        );
    }

    #[test]
    fn selectors_match_keccak_of_signatures() {
        let all: &[(&str, [u8; 4], u32)] = &[
            ("mint(address,uint256,bytes32)", SEL_MINT, 0x1e458bee),
            ("pauseMint()", SEL_PAUSE_MINT, 0xcd85cdb5),
            ("unpauseMint()", SEL_UNPAUSE_MINT, 0x1a8bd2da),
            ("pauseBridgeOut()", SEL_PAUSE_BRIDGE_OUT, 0x2f1d448f),
            ("unpauseBridgeOut()", SEL_UNPAUSE_BRIDGE_OUT, 0x7b455e06),
            ("setSupplyCap(uint256)", SEL_SET_SUPPLY_CAP, 0xb6a3f59a),
            ("setMintRateLimit(uint256,uint64)", SEL_SET_MINT_RATE_LIMIT, 0x1a5b7916),
            ("setMinBridgeOut(uint256)", SEL_SET_MIN_BRIDGE_OUT, 0xccf84ef8),
            ("mintConsumed(bytes32)", SEL_MINT_CONSUMED, 0x26c5b40e),
            ("bridgeOutUsed(bytes32)", SEL_BRIDGE_OUT_USED, 0x3e942d93),
            ("bridgeOutCount()", SEL_BRIDGE_OUT_COUNT, 0x53ebd818),
            ("totalSupply()", SEL_TOTAL_SUPPLY, 0x18160ddd),
            ("supplyCap()", SEL_SUPPLY_CAP, 0x8f770ad0),
            ("VERSION()", SEL_VERSION, 0xffa1ad74),
            ("hasRole(bytes32,address)", SEL_HAS_ROLE, 0x91d14854),
            ("defaultAdmin()", SEL_DEFAULT_ADMIN, 0x84ef8ffc),
            ("decimals()", SEL_DECIMALS, 0x313ce567),
            ("mintPaused()", SEL_MINT_PAUSED, 0x7e4831d3),
            ("bridgeOutPaused()", SEL_BRIDGE_OUT_PAUSED, 0xd948e8ec),
            ("minBridgeOut()", SEL_MIN_BRIDGE_OUT, 0xd88a9ba8),
            ("mintWindowLimit()", SEL_MINT_WINDOW_LIMIT, 0xfeafa875),
            ("mintWindowSeconds()", SEL_MINT_WINDOW_SECONDS, 0x24704279),
        ];
        for (sig, constant, pinned) in all {
            assert_eq!(*constant, sel(sig), "selector constant for {sig}");
            assert_eq!(*constant, pinned.to_be_bytes(), "pinned literal for {sig}");
        }
        // No two constants collide.
        let mut seen = std::collections::BTreeSet::new();
        for (sig, constant, _) in all {
            assert!(seen.insert(*constant), "duplicate selector for {sig}");
        }
    }

    // ----- encoders (expected hex produced by `cast calldata`) ----------------

    const ADDR_CAFE: [u8; 20] = {
        let mut a = [0u8; 20];
        a[18] = 0xca;
        a[19] = 0xfe;
        a
    };

    #[test]
    fn encode_mint_matches_cast() {
        // cast calldata 'mint(address,uint256,bytes32)' 0x000000000000000000000000000000000000cafe 2500000000 0x1111..11
        let expected = h("0x1e458bee000000000000000000000000000000000000000000000000000000000000cafe000000000000000000000000000000000000000000000000000000009502f9001111111111111111111111111111111111111111111111111111111111111111");
        assert_eq!(encode_mint(&ADDR_CAFE, 2_500_000_000, &[0x11; 32]), expected);
        assert_eq!(expected.len(), 4 + 96);
    }

    #[test]
    fn encode_mint_max_amount_fills_low_8_bytes_only() {
        let data = encode_mint(&[0xff; 20], u64::MAX, &[0u8; 32]);
        assert_eq!(&data[4..16], &[0u8; 12], "address word is left padded");
        assert_eq!(&data[16..36], &[0xff; 20]);
        assert_eq!(&data[36..60], &[0u8; 24], "amount word is left padded");
        assert_eq!(&data[60..68], &[0xff; 8]);
    }

    #[test]
    fn encode_admin_actions_match_cast() {
        assert_eq!(encode_admin_action(&ContractAdminAction::PauseMint), h("0xcd85cdb5"));
        assert_eq!(encode_admin_action(&ContractAdminAction::UnpauseMint), h("0x1a8bd2da"));
        assert_eq!(encode_admin_action(&ContractAdminAction::PauseBridgeOut), h("0x2f1d448f"));
        assert_eq!(encode_admin_action(&ContractAdminAction::UnpauseBridgeOut), h("0x7b455e06"));
        // cast calldata 'setSupplyCap(uint256)' 1000000000000
        assert_eq!(
            encode_admin_action(&ContractAdminAction::SetSupplyCap { cap_e8s: 1_000_000_000_000 }),
            h("0xb6a3f59a000000000000000000000000000000000000000000000000000000e8d4a51000")
        );
        // cast calldata 'setMintRateLimit(uint256,uint64)' 10000000000000 86400
        assert_eq!(
            encode_admin_action(&ContractAdminAction::SetMintRateLimit {
                limit_e8s: 10_000_000_000_000,
                window_seconds: 86_400
            }),
            h("0x1a5b7916000000000000000000000000000000000000000000000000000009184e72a0000000000000000000000000000000000000000000000000000000000000015180")
        );
        // cast calldata 'setMinBridgeOut(uint256)' 1000000
        assert_eq!(
            encode_admin_action(&ContractAdminAction::SetMinBridgeOut { minimum_e8s: 1_000_000 }),
            h("0xccf84ef800000000000000000000000000000000000000000000000000000000000f4240")
        );
    }

    #[test]
    fn encode_views_match_cast() {
        let cases: &[(ViewCall, &str)] = &[
            (ViewCall::Version, "0xffa1ad74"),
            (ViewCall::Decimals, "0x313ce567"),
            (ViewCall::TotalSupply, "0x18160ddd"),
            (ViewCall::SupplyCap, "0x8f770ad0"),
            (ViewCall::MintWindowLimit, "0xfeafa875"),
            (ViewCall::MintWindowSeconds, "0x24704279"),
            (ViewCall::MinBridgeOut, "0xd88a9ba8"),
            (ViewCall::MintPaused, "0x7e4831d3"),
            (ViewCall::BridgeOutPaused, "0xd948e8ec"),
            (ViewCall::BridgeOutCount, "0x53ebd818"),
            (ViewCall::DefaultAdmin, "0x84ef8ffc"),
            // cast calldata 'mintConsumed(bytes32)' 0x2222..22
            (
                ViewCall::MintConsumed([0x22; 32]),
                "0x26c5b40e2222222222222222222222222222222222222222222222222222222222222222",
            ),
            // cast calldata 'bridgeOutUsed(bytes32)' 0x3333..33
            (
                ViewCall::BridgeOutUsed([0x33; 32]),
                "0x3e942d933333333333333333333333333333333333333333333333333333333333333333",
            ),
            // cast calldata 'hasRole(bytes32,address)' MINTER_ROLE 0x2c7536E3605D9C16a7a3D7b1898e529396a65c23
            (
                ViewCall::HasRole {
                    role: MINTER_ROLE,
                    account: crate::ids::parse_address("0x2c7536E3605D9C16a7a3D7b1898e529396a65c23").unwrap(),
                },
                "0x91d148549f2df0fed2c77648de5860a4cc508cd0818c85b8b8a1ab4ceeef8d981c8956a60000000000000000000000002c7536e3605d9c16a7a3d7b1898e529396a65c23",
            ),
        ];
        for (call, expected) in cases {
            assert_eq!(encode_view(call), h(expected), "{call:?}");
        }
    }

    // ----- return decoders ----------------------------------------------------

    fn word_with_last(b: u8) -> Vec<u8> {
        let mut w = vec![0u8; 32];
        w[31] = b;
        w
    }

    #[test]
    fn decode_uint_accepts_exact_word_and_u128_max() {
        assert_eq!(decode_uint_u128(&word_with_last(7)).unwrap(), 7);
        assert_eq!(decode_uint_u128(&[0u8; 32]).unwrap(), 0);
        let mut w = vec![0u8; 32];
        w[16..].copy_from_slice(&u128::MAX.to_be_bytes());
        assert_eq!(decode_uint_u128(&w).unwrap(), u128::MAX);
    }

    #[test]
    fn decode_uint_rejects_bad_lengths_and_overflow() {
        assert!(decode_uint_u128(&[]).is_err());
        assert!(decode_uint_u128(&[0u8; 31]).is_err());
        assert!(decode_uint_u128(&[0u8; 33]).is_err());
        assert!(decode_uint_u128(&[0u8; 64]).is_err());
        let mut w = vec![0u8; 32];
        w[15] = 1; // 2^128
        assert!(decode_uint_u128(&w).is_err());
        let mut w = vec![0u8; 32];
        w[0] = 0x80;
        assert!(decode_uint_u128(&w).is_err());
    }

    #[test]
    fn decode_bool_is_strict() {
        assert!(!decode_bool(&word_with_last(0)).unwrap());
        assert!(decode_bool(&word_with_last(1)).unwrap());
        assert!(decode_bool(&word_with_last(2)).is_err());
        assert!(decode_bool(&word_with_last(0xff)).is_err());
        let mut w = word_with_last(1);
        w[0] = 1;
        assert!(decode_bool(&w).is_err(), "dirty upper byte");
        let mut w = word_with_last(0);
        w[30] = 1;
        assert!(decode_bool(&w).is_err(), "value 256");
        assert!(decode_bool(&[]).is_err());
        assert!(decode_bool(&[0u8; 31]).is_err());
        assert!(decode_bool(&[0u8; 33]).is_err());
    }

    #[test]
    fn decode_address_is_strict() {
        let mut w = vec![0u8; 32];
        w[12..].copy_from_slice(&[0xab; 20]);
        assert_eq!(decode_address(&w).unwrap(), [0xab; 20]);
        for i in 0..12 {
            let mut d = w.clone();
            d[i] = 1;
            assert!(decode_address(&d).is_err(), "dirty byte {i}");
        }
        assert!(decode_address(&w[..31]).is_err());
        let mut long = w.clone();
        long.push(0);
        assert!(decode_address(&long).is_err());
    }

    fn abi_string(s: &str) -> Vec<u8> {
        let mut out = word_u64(0x20).to_vec();
        out.extend_from_slice(&word_u64(s.len() as u64));
        out.extend_from_slice(s.as_bytes());
        out.resize(64 + s.len().div_ceil(32) * 32, 0);
        out
    }

    #[test]
    fn decode_string_matches_cast_vector() {
        // cast abi-encode 'f(string)' 'rumi-base-icusd-bridge-v1'
        let enc = h("0x0000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000000001972756d692d626173652d69637573642d6272696467652d763100000000000000");
        assert_eq!(decode_string(&enc).unwrap(), crate::ids::CONTRACT_VERSION);
        assert_eq!(abi_string(crate::ids::CONTRACT_VERSION), enc, "test helper agrees with cast");
        // cast abi-encode 'f(string)' ''
        let empty = h("0x00000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000000");
        assert_eq!(decode_string(&empty).unwrap(), "");
    }

    #[test]
    fn decode_string_boundary_lengths() {
        for len in [1usize, 31, 32, 33, 64, 1023, 1024] {
            let s = "a".repeat(len);
            assert_eq!(decode_string(&abi_string(&s)).unwrap(), s, "len {len}");
        }
        assert!(decode_string(&abi_string(&"a".repeat(1025))).is_err());
    }

    #[test]
    fn decode_string_rejects_non_canonical_encodings() {
        let good = abi_string("hello");
        assert!(decode_string(&good).is_ok());
        // Wrong offset.
        let mut bad = good.clone();
        bad[31] = 0x40;
        assert!(decode_string(&bad).is_err());
        // Dirty offset upper byte.
        let mut bad = good.clone();
        bad[0] = 1;
        assert!(decode_string(&bad).is_err());
        // Length word with high bits set.
        let mut bad = good.clone();
        bad[32] = 1;
        assert!(decode_string(&bad).is_err());
        // Length larger than the data present.
        let mut bad = good.clone();
        bad[63] = 64;
        assert!(decode_string(&bad).is_err());
        // Non-zero padding.
        let mut bad = good.clone();
        bad[64 + 5] = 1;
        assert!(decode_string(&bad).is_err());
        // Trailing extra word.
        let mut bad = good.clone();
        bad.extend_from_slice(&[0u8; 32]);
        assert!(decode_string(&bad).is_err());
        // Missing padding.
        assert!(decode_string(&good[..good.len() - 1]).is_err());
        // Too short.
        assert!(decode_string(&good[..63]).is_err());
        assert!(decode_string(&[]).is_err());
        // Not UTF-8.
        let mut bad = abi_string("a");
        bad[64] = 0xff;
        assert!(decode_string(&bad).is_err());
    }

    // ----- BridgeOut ----------------------------------------------------------

    /// The exact `data` the contract tests assert: amount 2500000000, sequence 1,
    /// principal 0x00000000000000020101, subaccount bytes32(0xabc). Also produced by
    /// `cast abi-encode 'f(uint256,uint64,bytes,bytes32)' 2500000000 1 0x00000000000000020101 0x...0abc`.
    const CONTRACT_DATA_HEX: &str = "000000000000000000000000000000000000000000000000000000009502f900\
                                     0000000000000000000000000000000000000000000000000000000000000001\
                                     0000000000000000000000000000000000000000000000000000000000000080\
                                     0000000000000000000000000000000000000000000000000000000000000abc\
                                     000000000000000000000000000000000000000000000000000000000000000a\
                                     0000000000000002010100000000000000000000000000000000000000000000";

    const TRANSFER_ID: [u8; 32] = [0x7a; 32];
    const SENDER: [u8; 20] = [0x5e; 20];

    fn out_log() -> RawLog {
        let mut t2 = [0u8; 32];
        t2[12..].copy_from_slice(&SENDER);
        RawLog {
            address: CONTRACT,
            topics: vec![BRIDGE_OUT_TOPIC0, TRANSFER_ID, t2],
            data: h(CONTRACT_DATA_HEX),
            block_number: 100,
            block_hash: [1; 32],
            tx_hash: [2; 32],
            log_index: 3,
            removed: false,
        }
    }

    #[test]
    fn bridge_out_contract_vector_decodes() {
        let ev = decode_bridge_out(&out_log(), &CONTRACT).unwrap();
        assert_eq!(ev.transfer_id, TRANSFER_ID);
        assert_eq!(ev.from, SENDER);
        assert_eq!(ev.amount_e8s, 2_500_000_000);
        assert_eq!(ev.sequence, 1);
        assert_eq!(ev.icp_principal, h("0x00000000000000020101"));
        let mut sub = [0u8; 32];
        sub[30] = 0x0a;
        sub[31] = 0xbc;
        assert_eq!(ev.icp_subaccount, sub);
    }

    #[test]
    fn bridge_out_contract_vector_is_192_bytes_as_documented() {
        assert_eq!(out_log().data.len(), BRIDGE_OUT_DATA_LEN);
    }

    #[test]
    fn bridge_out_29_byte_principal_and_max_values() {
        let mut log = out_log();
        let mut d = vec![0u8; 192];
        d[24..32].copy_from_slice(&u64::MAX.to_be_bytes());
        d[56..64].copy_from_slice(&u64::MAX.to_be_bytes());
        d[88..96].copy_from_slice(&0x80u64.to_be_bytes());
        d[96..128].copy_from_slice(&[0xee; 32]);
        d[152..160].copy_from_slice(&29u64.to_be_bytes());
        for i in 0..29 {
            d[160 + i] = 0xa0 + i as u8;
        }
        log.data = d;
        let ev = decode_bridge_out(&log, &CONTRACT).unwrap();
        assert_eq!(ev.amount_e8s, u64::MAX);
        assert_eq!(ev.sequence, u64::MAX);
        assert_eq!(ev.icp_principal.len(), 29);
        assert_eq!(ev.icp_principal[28], 0xa0 + 28);
        assert_eq!(ev.icp_subaccount, [0xee; 32]);
    }

    fn mutate_out(f: impl FnOnce(&mut RawLog)) -> RawLog {
        let mut l = out_log();
        f(&mut l);
        l
    }

    #[test]
    fn bridge_out_rejects_envelope_violations() {
        let bad: Vec<(&str, RawLog)> = vec![
            ("removed", mutate_out(|l| l.removed = true)),
            ("wrong address", mutate_out(|l| l.address = [0xc1; 20])),
            ("wrong topic0", mutate_out(|l| l.topics[0] = BRIDGE_MINT_TOPIC0)),
            ("zero topic0", mutate_out(|l| l.topics[0] = [0u8; 32])),
            ("two topics", mutate_out(|l| l.topics.truncate(2))),
            ("one topic", mutate_out(|l| l.topics.truncate(1))),
            ("no topics", mutate_out(|l| l.topics.clear())),
            ("four topics", mutate_out(|l| l.topics.push([9u8; 32]))),
            ("dirty topic2 first byte", mutate_out(|l| l.topics[2][0] = 1)),
            ("dirty topic2 byte 11", mutate_out(|l| l.topics[2][11] = 1)),
        ];
        for (name, log) in bad {
            assert!(decode_bridge_out(&log, &CONTRACT).is_err(), "{name} must be rejected");
        }
    }

    #[test]
    fn bridge_out_rejects_wrong_expected_contract() {
        assert!(decode_bridge_out(&out_log(), &[0xc1; 20]).is_err());
    }

    #[test]
    fn bridge_out_rejects_data_violations() {
        // Wrong total length.
        assert!(decode_bridge_out(&mutate_out(|l| l.data.truncate(191)), &CONTRACT).is_err(), "short by one");
        assert!(decode_bridge_out(&mutate_out(|l| l.data.truncate(160)), &CONTRACT).is_err(), "no principal word");
        assert!(decode_bridge_out(&mutate_out(|l| l.data.clear()), &CONTRACT).is_err(), "empty");
        assert!(decode_bridge_out(&mutate_out(|l| l.data.push(0)), &CONTRACT).is_err(), "long by one");
        assert!(decode_bridge_out(&mutate_out(|l| l.data.extend_from_slice(&[0u8; 32])), &CONTRACT).is_err(), "long by a word");

        // Amount: zero, 2^64, dirty upper bytes.
        assert!(decode_bridge_out(&mutate_out(|l| l.data[24..32].fill(0)), &CONTRACT).is_err(), "zero amount");
        assert!(decode_bridge_out(&mutate_out(|l| l.data[23] = 1), &CONTRACT).is_err(), "amount 2^64");
        assert!(decode_bridge_out(&mutate_out(|l| l.data[0] = 0x80), &CONTRACT).is_err(), "amount high bit");

        // Sequence: zero, 2^64.
        assert!(decode_bridge_out(&mutate_out(|l| l.data[56..64].fill(0)), &CONTRACT).is_err(), "zero sequence");
        assert!(decode_bridge_out(&mutate_out(|l| l.data[55] = 1), &CONTRACT).is_err(), "sequence 2^64");

        // Offset: 0xa0, 0x60, 0, dirty upper bytes.
        assert!(decode_bridge_out(&mutate_out(|l| l.data[95] = 0xa0), &CONTRACT).is_err(), "offset 0xa0");
        assert!(decode_bridge_out(&mutate_out(|l| l.data[95] = 0x60), &CONTRACT).is_err(), "offset 0x60");
        assert!(decode_bridge_out(&mutate_out(|l| l.data[95] = 0), &CONTRACT).is_err(), "offset 0");
        assert!(decode_bridge_out(&mutate_out(|l| l.data[64] = 1), &CONTRACT).is_err(), "offset high byte");

        // Principal length: 0, 30, 32, 2^64 + 10.
        assert!(decode_bridge_out(&mutate_out(|l| l.data[159] = 0), &CONTRACT).is_err(), "length 0");
        assert!(decode_bridge_out(&mutate_out(|l| l.data[159] = 30), &CONTRACT).is_err(), "length 30");
        assert!(decode_bridge_out(&mutate_out(|l| l.data[159] = 32), &CONTRACT).is_err(), "length 32");
        assert!(decode_bridge_out(&mutate_out(|l| l.data[151] = 1), &CONTRACT).is_err(), "length 2^64 + 10");

        // Non-zero padding after the principal bytes (first and last padding byte).
        assert!(decode_bridge_out(&mutate_out(|l| l.data[160 + 10] = 1), &CONTRACT).is_err(), "padding byte 10");
        assert!(decode_bridge_out(&mutate_out(|l| l.data[191] = 1), &CONTRACT).is_err(), "padding byte 31");
    }

    #[test]
    fn bridge_out_rejects_anonymous_principal() {
        let log = mutate_out(|l| {
            l.data[159] = 1; // length 1
            l.data[160..192].fill(0);
            l.data[160] = 0x04;
        });
        assert!(decode_bridge_out(&log, &CONTRACT).is_err());
        // A different single byte principal is structurally fine at this layer.
        let ok = mutate_out(|l| {
            l.data[159] = 1;
            l.data[160..192].fill(0);
            l.data[160] = 0x05;
        });
        assert_eq!(decode_bridge_out(&ok, &CONTRACT).unwrap().icp_principal, vec![0x05]);
    }

    #[test]
    fn bridge_out_subaccount_is_unconstrained() {
        for fill in [0u8, 0xff] {
            let log = mutate_out(|l| l.data[96..128].fill(fill));
            assert_eq!(decode_bridge_out(&log, &CONTRACT).unwrap().icp_subaccount, [fill; 32]);
        }
    }

    // ----- BridgeMint ---------------------------------------------------------

    const MINT_ID: [u8; 32] = [0x33; 32];
    const RECIPIENT: [u8; 20] = [0x77; 20];

    fn mint_log() -> RawLog {
        let mut t2 = [0u8; 32];
        t2[12..].copy_from_slice(&RECIPIENT);
        // From the contract test: data == hex"...9502f900" (2_500_000_000).
        RawLog {
            address: CONTRACT,
            topics: vec![BRIDGE_MINT_TOPIC0, MINT_ID, t2],
            data: h("0x000000000000000000000000000000000000000000000000000000009502f900"),
            block_number: 5,
            block_hash: [1; 32],
            tx_hash: [2; 32],
            log_index: 0,
            removed: false,
        }
    }

    fn mutate_mint(f: impl FnOnce(&mut RawLog)) -> RawLog {
        let mut l = mint_log();
        f(&mut l);
        l
    }

    #[test]
    fn bridge_mint_decodes_contract_vector() {
        let ev = decode_bridge_mint(&mint_log(), &CONTRACT).unwrap();
        assert_eq!(ev, BridgeMintEvent { mint_id: MINT_ID, to: RECIPIENT, amount_e8s: 2_500_000_000 });
        let max = mutate_mint(|l| l.data[24..32].copy_from_slice(&u64::MAX.to_be_bytes()));
        assert_eq!(decode_bridge_mint(&max, &CONTRACT).unwrap().amount_e8s, u64::MAX);
    }

    #[test]
    fn bridge_mint_rejects_violations() {
        let bad: Vec<(&str, RawLog)> = vec![
            ("removed", mutate_mint(|l| l.removed = true)),
            ("wrong address", mutate_mint(|l| l.address = [0xc1; 20])),
            ("wrong topic0", mutate_mint(|l| l.topics[0] = BRIDGE_OUT_TOPIC0)),
            ("two topics", mutate_mint(|l| l.topics.truncate(2))),
            ("four topics", mutate_mint(|l| l.topics.push([1u8; 32]))),
            ("dirty topic2", mutate_mint(|l| l.topics[2][3] = 1)),
            ("data 31 bytes", mutate_mint(|l| l.data.truncate(31))),
            ("data 33 bytes", mutate_mint(|l| l.data.push(0))),
            ("data 64 bytes", mutate_mint(|l| l.data.extend_from_slice(&[0u8; 32]))),
            ("empty data", mutate_mint(|l| l.data.clear())),
            ("zero amount", mutate_mint(|l| l.data.fill(0))),
            ("amount 2^64", mutate_mint(|l| {
                l.data.fill(0);
                l.data[23] = 1;
            })),
            ("amount high bit", mutate_mint(|l| l.data[0] = 0x80)),
        ];
        for (name, log) in bad {
            assert!(decode_bridge_mint(&log, &CONTRACT).is_err(), "{name} must be rejected");
        }
        assert!(decode_bridge_mint(&mint_log(), &[0xc1; 20]).is_err());
    }

    #[test]
    fn a_bridge_out_log_never_decodes_as_a_mint_and_vice_versa() {
        assert!(decode_bridge_mint(&out_log(), &CONTRACT).is_err());
        assert!(decode_bridge_out(&mint_log(), &CONTRACT).is_err());
    }
}
