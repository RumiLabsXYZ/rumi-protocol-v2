//! EIP-1559 (type-0x02) transaction builder, calldata helpers, and tECDSA
//! signing wrapper for Monad.
//!
//! # EIP-1559 encoding
//! Unsigned (signing payload): `0x02 || rlp([chain_id, nonce,
//!   max_priority_fee_per_gas, max_fee_per_gas, gas_limit, to, value, data,
//!   access_list])`.
//! Signed: same list extended with `[y_parity, r, s]`.
//! `signing_hash` = keccak256 of the unsigned payload.
//!
//! Integer fields are minimal big-endian (no leading zeros; zero = 0x80).
//! `to` is exactly 20 bytes.  `r` and `s` are minimal big-endian integers
//! (leading zero bytes stripped) — this is the classic RLP gotcha.

use alloy_rlp::Encodable;
use sha3::{Digest, Keccak256};

use ic_cdk::api::management_canister::ecdsa::{
    sign_with_ecdsa, EcdsaCurve, EcdsaKeyId, SignWithEcdsaArgument,
};

use crate::chains::monad::config::monad_ecdsa_key_name;

// ─── public types ────────────────────────────────────────────────────────────

/// All fields required to build an EIP-1559 (type-0x02) transaction.
pub struct Eip1559Fields {
    pub chain_id: u64,
    pub nonce: u64,
    pub max_priority_fee_per_gas: u128,
    pub max_fee_per_gas: u128,
    pub gas_limit: u64,
    /// "0x…" hex-encoded 20-byte Ethereum address.
    pub to: String,
    pub value: u128,
    pub data: Vec<u8>,
}

/// The per-op-kind shape of a Monad settlement transaction (what varies between
/// a mint and a native withdrawal: `to`, `value`, calldata, gas_limit).
pub enum MonadTxKind<'a> {
    /// `mint(address,uint256,uint64)` on the icUSD EVM contract.
    Mint { contract: &'a str, recipient: &'a str, amount_e8s: u128, vault_id: u64 },
    /// A native MON transfer (`amount_wei` carried in the EIP-1559 `value`).
    NativeWithdrawal { recipient: &'a str, amount_wei: u128 },
}

/// Single source of truth for the per-op-kind EIP-1559 field shape (gas_limit,
/// calldata, to, value). Both the settlement worker (`build_tx_plan`) and
/// `MonadAdapter::sign_*` call this so a submit and a replace-by-fee resubmit
/// build byte-identical transactions (only nonce + fees differ). Two
/// independent builders that MUST stay identical are a latent double-mint
/// hazard: if one drifts (e.g. a gas_limit change), a replace-by-fee resubmit
/// stops replacing and becomes a second on-chain mint.
///
/// - Mint: `to` = icUSD contract, `value` = 0, calldata = `encode_mint_calldata`,
///   gas_limit 120_000.
/// - Native withdrawal: `to` = recipient, `value` = amount (wei), empty data,
///   gas_limit 21_000.
///
/// Returns `Err` if any address string is malformed (defense-in-depth: the
/// boundary validation at `set_chain_contract`/`open_chain_vault`/withdraw+close
/// makes this unreachable in practice, but a malformed address must never trap
/// the settlement worker after the re-entrancy guard is held).
pub fn build_eip1559_fields(
    chain_id: u64,
    kind: MonadTxKind,
    nonce: u64,
    prio: u128,
    max_fee: u128,
) -> Result<Eip1559Fields, String> {
    match kind {
        MonadTxKind::Mint { contract, recipient, amount_e8s, vault_id } => {
            let data = encode_mint_calldata(recipient, amount_e8s, vault_id)?;
            Ok(Eip1559Fields {
                chain_id,
                nonce,
                max_priority_fee_per_gas: prio,
                max_fee_per_gas: max_fee,
                gas_limit: 120_000,
                to: contract.to_string(),
                value: 0,
                data,
            })
        }
        MonadTxKind::NativeWithdrawal { recipient, amount_wei } => Ok(Eip1559Fields {
            chain_id,
            nonce,
            max_priority_fee_per_gas: prio,
            max_fee_per_gas: max_fee,
            gas_limit: 21_000,
            to: recipient.to_string(),
            value: amount_wei,
            data: vec![],
        }),
    }
}

// ─── calldata helpers ─────────────────────────────────────────────────────────

/// Build calldata for `mint(address,uint256,uint64)`.
/// Signature string: `"mint(address,uint256,uint64)"`.
/// Layout: 4-byte selector || word(address) || word(amount) || word(vault_id).
///
/// Returns `Err` if `to` is not a valid 20-byte hex address.
pub fn encode_mint_calldata(to: &str, amount_e8s: u128, vault_id: u64) -> Result<Vec<u8>, String> {
    let selector = keccak_selector("mint(address,uint256,uint64)");
    let mut out = Vec::with_capacity(4 + 96);
    out.extend_from_slice(&selector);
    out.extend_from_slice(&abi_word_address(to)?);
    out.extend_from_slice(&abi_word_u128(amount_e8s));
    out.extend_from_slice(&abi_word_u128(vault_id as u128));
    Ok(out)
}

/// Build calldata for `transfer(address,uint256)`.
/// Signature string: `"transfer(address,uint256)"`.
/// Layout: 4-byte selector || word(address) || word(amount).
///
/// Returns `Err` if `to` is not a valid 20-byte hex address.
pub fn encode_transfer_calldata(to: &str, amount: u128) -> Result<Vec<u8>, String> {
    let selector = keccak_selector("transfer(address,uint256)");
    let mut out = Vec::with_capacity(4 + 64);
    out.extend_from_slice(&selector);
    out.extend_from_slice(&abi_word_address(to)?);
    out.extend_from_slice(&abi_word_u128(amount));
    Ok(out)
}

// ─── EIP-1559 encoding ───────────────────────────────────────────────────────

/// Compute the signing hash: keccak256(`0x02 || rlp([...fields..., access_list])`).
///
/// Returns `Err` if `fields.to` is not a valid 20-byte hex address.
pub fn signing_hash(fields: &Eip1559Fields) -> Result<[u8; 32], String> {
    let payload = rlp_encode_eip1559(fields, None)?;
    Ok(Keccak256::digest(&payload).into())
}

/// Assemble the final signed EIP-1559 transaction bytes:
/// `0x02 || rlp([...fields..., access_list, y_parity, r, s])`.
///
/// Returns `Err` if `y_parity` is not 0 or 1, or if `fields.to` is malformed.
pub fn assemble_signed_tx(
    fields: &Eip1559Fields,
    r: &[u8; 32],
    s: &[u8; 32],
    y_parity: u8,
) -> Result<Vec<u8>, String> {
    if y_parity > 1 {
        return Err(format!("y_parity must be 0 or 1, got {y_parity}"));
    }
    rlp_encode_eip1559(fields, Some((r, s, y_parity)))
}

/// Determine which `y_parity` (0 or 1) matches `expected_addr` for the given
/// (hash, r, s).  Returns `Err` if neither parity recovers the expected address.
pub fn recover_y_parity(
    hash: &[u8; 32],
    r: &[u8; 32],
    s: &[u8; 32],
    expected_addr: &str,
) -> Result<u8, String> {
    use k256::ecdsa::{RecoveryId, Signature, VerifyingKey};
    use super::tecdsa::evm_address_from_pubkey;

    let sig = Signature::from_scalars(*r, *s)
        .map_err(|e| format!("invalid (r,s): {e}"))?;

    for parity in 0u8..=1 {
        let rid = RecoveryId::new(parity == 1, false);
        let Ok(vk) = VerifyingKey::recover_from_prehash(hash, &sig, rid) else {
            continue;
        };
        let pk_bytes = vk.to_encoded_point(false).as_bytes().to_vec();
        let Ok(addr) = evm_address_from_pubkey(&pk_bytes) else {
            continue;
        };
        if addr.to_lowercase() == expected_addr.to_lowercase() {
            return Ok(parity);
        }
    }
    Err(format!(
        "neither y_parity=0 nor y_parity=1 recovers address {expected_addr}"
    ))
}

/// High-level helper: compute signing hash, call tECDSA management canister,
/// split the 64-byte compact signature into (r, s), recover y_parity, assemble
/// the signed transaction, and return it as `"0x…"` hex.
pub async fn sign_eip1559(
    fields: &Eip1559Fields,
    derivation_path: Vec<Vec<u8>>,
    signer_addr: &str,
) -> Result<String, String> {
    let hash = signing_hash(fields)?;

    let key_id = EcdsaKeyId { curve: EcdsaCurve::Secp256k1, name: monad_ecdsa_key_name() };
    let arg = SignWithEcdsaArgument {
        message_hash: hash.to_vec(),
        derivation_path,
        key_id,
    };

    let (res,) = sign_with_ecdsa(arg)
        .await
        .map_err(|(code, msg)| format!("sign_with_ecdsa failed: {code:?}: {msg}"))?;

    if res.signature.len() != 64 {
        return Err(format!(
            "expected 64-byte compact signature, got {}",
            res.signature.len()
        ));
    }
    let r: [u8; 32] = res.signature[..32].try_into().unwrap();
    let s: [u8; 32] = res.signature[32..].try_into().unwrap();

    let y_parity = recover_y_parity(&hash, &r, &s, signer_addr)?;
    let signed = assemble_signed_tx(fields, &r, &s, y_parity)?;
    Ok(format!("0x{}", hex::encode(signed)))
}

// ─── internal RLP helpers ────────────────────────────────────────────────────

/// Encode the full EIP-1559 transaction, with or without signature.
/// Returns `0x02 || rlp_list([chain_id, nonce, max_priority_fee,
///   max_fee, gas_limit, to, value, data, access_list, (y_parity, r, s)?])`.
///
/// Returns `Err` if `fields.to` is not a valid 20-byte hex address.
fn rlp_encode_eip1559(
    fields: &Eip1559Fields,
    sig: Option<(&[u8; 32], &[u8; 32], u8)>,
) -> Result<Vec<u8>, String> {
    let to_bytes = parse_address(&fields.to)?;

    let mut payload = Vec::new();
    fields.chain_id.encode(&mut payload);
    fields.nonce.encode(&mut payload);
    fields.max_priority_fee_per_gas.encode(&mut payload);
    fields.max_fee_per_gas.encode(&mut payload);
    fields.gas_limit.encode(&mut payload);
    // `to`: 20-byte string (not an integer — not minimized).
    encode_20_byte_string(&to_bytes, &mut payload);
    fields.value.encode(&mut payload);
    // `data`: byte string — [u8] implements Encodable as an RLP byte string.
    fields.data.as_slice().encode(&mut payload);
    // `access_list`: empty list.
    payload.push(0xc0u8);

    if let Some((r, s, y_parity)) = sig {
        // y_parity: 0 => 0x80, 1 => 0x01.
        (y_parity as u64).encode(&mut payload);
        // r and s: minimal big-endian integers (strip leading zeros).
        encode_minimal_uint(r, &mut payload);
        encode_minimal_uint(s, &mut payload);
    }

    let mut out = Vec::new();
    out.push(0x02u8);
    write_rlp_list_header(payload.len(), &mut out);
    out.extend_from_slice(&payload);
    Ok(out)
}

/// Encode a 20-byte address as an RLP byte string (NOT an integer).
/// Header = 0x80 + 20 = 0x94, followed by 20 bytes.
fn encode_20_byte_string(bytes: &[u8; 20], out: &mut Vec<u8>) {
    out.push(0x94u8); // 0x80 + 20
    out.extend_from_slice(bytes);
}

/// Encode a 32-byte value as an RLP minimal big-endian integer.
/// Leading zero bytes are stripped.  Zero encodes as 0x80 (empty string).
fn encode_minimal_uint(bytes: &[u8; 32], out: &mut Vec<u8>) {
    let stripped = strip_leading_zeros(bytes);
    if stripped.is_empty() {
        out.push(0x80u8); // zero
    } else if stripped.len() == 1 && stripped[0] < 0x80 {
        out.push(stripped[0]);
    } else {
        // Length ≤ 32 so always fits in a single-byte string header.
        out.push(0x80 + stripped.len() as u8);
        out.extend_from_slice(stripped);
    }
}

fn strip_leading_zeros(b: &[u8; 32]) -> &[u8] {
    let first = b.iter().position(|&x| x != 0).unwrap_or(32);
    &b[first..]
}

/// Write an RLP list header for `payload_len` bytes of list content.
fn write_rlp_list_header(payload_len: usize, out: &mut Vec<u8>) {
    if payload_len <= 55 {
        out.push(0xc0 + payload_len as u8);
    } else {
        let lb = minimal_be_bytes(payload_len);
        out.push(0xf7 + lb.len() as u8);
        out.extend_from_slice(&lb);
    }
}

/// Minimal big-endian byte encoding of a usize (used for RLP length prefixes).
fn minimal_be_bytes(mut n: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    while n > 0 {
        bytes.push(n as u8);
        n >>= 8;
    }
    bytes.reverse();
    bytes
}

// ─── ABI / calldata helpers ───────────────────────────────────────────────────

/// First 4 bytes of keccak256(function_signature_string).
fn keccak_selector(sig: &str) -> [u8; 4] {
    let hash = Keccak256::digest(sig.as_bytes());
    [hash[0], hash[1], hash[2], hash[3]]
}

/// ABI-encode an Ethereum address as a 32-byte left-padded word.
/// Strips the "0x" prefix, pads to 32 bytes.
///
/// Returns `Err` if `addr` is not valid hex or not exactly 20 bytes.
fn abi_word_address(addr: &str) -> Result<[u8; 32], String> {
    let addr_str = addr.strip_prefix("0x").or_else(|| addr.strip_prefix("0X")).unwrap_or(addr);
    let addr_bytes = hex::decode(addr_str)
        .map_err(|e| format!("abi_word_address: invalid hex in '{}': {}", addr, e))?;
    if addr_bytes.len() != 20 {
        return Err(format!(
            "abi_word_address: '{}' is {} bytes, expected 20",
            addr,
            addr_bytes.len()
        ));
    }
    let mut word = [0u8; 32];
    word[12..].copy_from_slice(&addr_bytes);
    Ok(word)
}

/// ABI-encode a u128 as a 32-byte big-endian word.
fn abi_word_u128(n: u128) -> [u8; 32] {
    let mut word = [0u8; 32];
    word[16..].copy_from_slice(&n.to_be_bytes());
    word
}

/// Parse a "0x…" hex address into 20 bytes.
///
/// Returns `Err` if `addr` is not valid hex or not exactly 20 bytes.
fn parse_address(addr: &str) -> Result<[u8; 20], String> {
    let hex_str = addr.strip_prefix("0x").or_else(|| addr.strip_prefix("0X")).unwrap_or(addr);
    let bytes = hex::decode(hex_str)
        .map_err(|e| format!("parse_address: invalid hex in '{}': {}", addr, e))?;
    bytes.try_into().map_err(|v: Vec<u8>| {
        format!("parse_address: '{}' is {} bytes, expected 20", addr, v.len())
    })
}

// ─── test-only re-exports of private helpers ─────────────────────────────────
//
// `abi_word_address` and `parse_address` are private (module-internal). To
// let `tests_tx` assert their error-return behaviour without changing their
// visibility, we expose thin wrappers under `#[cfg(test)]`.

#[cfg(test)]
pub fn abi_word_address_test(addr: &str) -> Result<[u8; 32], String> {
    abi_word_address(addr)
}

#[cfg(test)]
pub fn parse_address_test(addr: &str) -> Result<[u8; 20], String> {
    parse_address(addr)
}
