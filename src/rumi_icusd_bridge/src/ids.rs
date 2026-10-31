//! Identifiers, ledger memos, subaccounts and threshold-ECDSA derivation paths.
//!
//! Every identifier that crosses a trust boundary carries an explicit domain tag so
//! a value produced for one purpose can never be accepted for another:
//!
//! - Mint ids for deposits and for held-claim re-mints use different domains, each built
//!   from the chain slug in `chains.rs`.
//! - Bridge-out transfer ids are computed by the Base contract under its own domain
//!   (`rumi.icusd.base.bridge-out.v1`), so they cannot collide with mint ids.
//! - Ledger memos carry a purpose prefix.
//! - Derivation paths start with a bridge-specific root and a purpose label. They are
//!   also bound to this canister's principal by the IC, so they can never equal any
//!   address derived by `rumi_protocol_backend`.

use crate::chains;
use sha2::{Digest as _, Sha256};
use sha3::Keccak256;

/// Contract version tag the canister requires before binding (`VERSION()`).
pub const CONTRACT_VERSION: &str = "rumi-base-icusd-bridge-v1";

/// Domain string the contract hashes into bridge-out transfer ids. The token is compiled
/// with this exact string, so it stays Base-named under every chain slug.
pub const BRIDGE_OUT_DOMAIN: &str = "rumi.icusd.base.bridge-out.v1";

/// Version segment of every threshold-ECDSA derivation path.
pub const KEY_PATH_VERSION: &[u8] = b"v1";

/// Memo prefixes (8 bytes each) for ledger operations.
pub const PULL_MEMO_PREFIX: &[u8; 8] = b"RBRG-DEP";
pub const REFUND_MEMO_PREFIX: &[u8; 8] = b"RBRG-RFD";
pub const SWEEP_MEMO_PREFIX: &[u8; 8] = b"RBRG-FEE";

// ---------------------------------------------------------------------------
// Slug templates
//
// Four off-chain strings (plus the init-time salt label) carry the chain slug from the
// static table in `chains.rs`. The Base slug reproduces the strings the live Base instance
// already signs under, byte for byte (`tests::base_slug_reproduces_live_strings`). The
// templates take the slug as an argument; the id functions below look it up from the chain
// id, so no caller can pick a slug that disagrees with the chain it hashes.
// ---------------------------------------------------------------------------

/// Domain string hashed into deposit mint ids: `rumi.icusd.{slug}.mint.v1`.
pub fn mint_domain(slug: &str) -> String {
    format!("rumi.icusd.{slug}.mint.v1")
}

/// Domain string hashed into held-claim re-mint ids: `rumi.icusd.{slug}.remint.v1`.
pub fn remint_domain(slug: &str) -> String {
    format!("rumi.icusd.{slug}.remint.v1")
}

/// Label hashed (SHA-256) into the escrow subaccount: `rumi-icusd-{slug}-bridge:escrow:v1`.
pub fn escrow_label(slug: &str) -> String {
    format!("rumi-icusd-{slug}-bridge:escrow:v1")
}

/// Root segment of every threshold-ECDSA derivation path: `rumi-icusd-{slug}-bridge`.
pub fn key_root(slug: &str) -> String {
    format!("rumi-icusd-{slug}-bridge")
}

/// Label hashed into the per-instance salt at install: `rumi-icusd-{slug}-bridge:instance-salt:v1`.
pub fn instance_salt_label(slug: &str) -> String {
    format!("rumi-icusd-{slug}-bridge:instance-salt:v1")
}

/// The slug of the chain this bridge instance serves, from the static chain table.
///
/// Panics on a chain id with no profile. It must never fall back to a default slug (for
/// example `base`): that would silently hash another chain's ids, escrow and keys under the
/// Base strings. The panic is unreachable in a running canister because the chain id comes
/// only from the stored config, and both entry points guarantee it has a profile: install
/// admits only validated ids (`state::validate_init`, covered by
/// `chains::tests::every_chain_install_admits_has_a_profile`), and `post_upgrade` refuses
/// a stored id that has none (`chains::validate_stored_chain`).
pub fn chain_slug(chain_id: u64) -> &'static str {
    match chains::profile(chain_id) {
        Some(p) => p.slug,
        None => panic!("no bridge chain profile for chain id {chain_id}"),
    }
}

/// The 32-byte escrow subaccount of the bridge canister on `chain_id`.
pub fn escrow_subaccount(chain_id: u64) -> [u8; 32] {
    Sha256::digest(escrow_label(chain_slug(chain_id)).as_bytes()).into()
}

pub fn keccak256(data: &[u8]) -> [u8; 32] {
    Keccak256::digest(data).into()
}

/// ABI word for a `uint256` holding a `u64`.
pub fn word_u64(v: u64) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[24..].copy_from_slice(&v.to_be_bytes());
    w
}

/// ABI word for a `uint256` holding a `u128`.
pub fn word_u128(v: u128) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[16..].copy_from_slice(&v.to_be_bytes());
    w
}

/// ABI word for an `address`.
pub fn word_address(a: &[u8; 20]) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[12..].copy_from_slice(a);
    w
}

fn abi_hash(words: &[[u8; 32]]) -> [u8; 32] {
    let mut buf = Vec::with_capacity(words.len() * 32);
    for w in words {
        buf.extend_from_slice(w);
    }
    keccak256(&buf)
}

/// `keccak256(abi.encode(keccak256(mint_domain(slug)), chain_id, contract, instance_salt, deposit_id))`.
pub fn mint_id(chain_id: u64, contract: &[u8; 20], instance_salt: &[u8; 32], deposit_id: u64) -> [u8; 32] {
    abi_hash(&[
        keccak256(mint_domain(chain_slug(chain_id)).as_bytes()),
        word_u64(chain_id),
        word_address(contract),
        *instance_salt,
        word_u64(deposit_id),
    ])
}

/// `keccak256(abi.encode(keccak256(remint_domain(slug)), chain_id, contract, instance_salt, transfer_id))`.
pub fn remint_id(chain_id: u64, contract: &[u8; 20], instance_salt: &[u8; 32], transfer_id: &[u8; 32]) -> [u8; 32] {
    abi_hash(&[
        keccak256(remint_domain(chain_slug(chain_id)).as_bytes()),
        word_u64(chain_id),
        word_address(contract),
        *instance_salt,
        *transfer_id,
    ])
}

/// The transfer id the contract assigns: `keccak256(abi.encode(keccak256(BRIDGE_OUT_DOMAIN),
/// chainid, contract, from, clientNonce))`. Used by tests and by the frontend vector check.
pub fn bridge_out_transfer_id(chain_id: u64, contract: &[u8; 20], from: &[u8; 20], client_nonce: &[u8; 32]) -> [u8; 32] {
    abi_hash(&[
        keccak256(BRIDGE_OUT_DOMAIN.as_bytes()),
        word_u64(chain_id),
        word_address(contract),
        word_address(from),
        *client_nonce,
    ])
}

fn prefixed_memo(prefix: &[u8; 8], id: u64) -> Vec<u8> {
    let mut m = Vec::with_capacity(16);
    m.extend_from_slice(prefix);
    m.extend_from_slice(&id.to_be_bytes());
    m
}

pub fn pull_memo(deposit_id: u64) -> Vec<u8> {
    prefixed_memo(PULL_MEMO_PREFIX, deposit_id)
}

pub fn refund_memo(deposit_id: u64) -> Vec<u8> {
    prefixed_memo(REFUND_MEMO_PREFIX, deposit_id)
}

pub fn sweep_memo(sweep_id: u64) -> Vec<u8> {
    prefixed_memo(SWEEP_MEMO_PREFIX, sweep_id)
}

/// Release memo is the 32-byte transfer id itself.
pub fn release_memo(transfer_id: &[u8; 32]) -> Vec<u8> {
    transfer_id.to_vec()
}

/// Which signing identity a Base transaction uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize, candid::CandidType)]
pub enum LaneKind {
    /// Holds `MINTER_ROLE` on the Base contract.
    Minter,
    /// Holds `DEFAULT_ADMIN_ROLE` on the Base contract.
    Admin,
}

impl LaneKind {
    pub fn label(&self) -> &'static [u8] {
        match self {
            LaneKind::Minter => b"minter",
            LaneKind::Admin => b"admin",
        }
    }
}

/// `[key_root(slug), "v1", purpose, chain_id (8 bytes BE)]`.
pub fn derivation_path(lane: LaneKind, chain_id: u64) -> Vec<Vec<u8>> {
    vec![
        key_root(chain_slug(chain_id)).into_bytes(),
        KEY_PATH_VERSION.to_vec(),
        lane.label().to_vec(),
        chain_id.to_be_bytes().to_vec(),
    ]
}

/// Lowercase `0x`-prefixed hex.
pub fn hex0x(bytes: &[u8]) -> String {
    format!("0x{}", hex::encode(bytes))
}

/// EIP-55 mixed-case checksum rendering of a 20-byte address (`0x` + 40 hex chars).
///
/// Purely presentational: every RPC parameter, stored value and hashed input in this
/// crate uses lowercase `hex0x`, never this form.
pub fn to_checksum(address: &[u8; 20]) -> String {
    let lower = hex::encode(address);
    let digest = keccak256(lower.as_bytes());
    let mut out = String::with_capacity(42);
    out.push_str("0x");
    for (i, c) in lower.chars().enumerate() {
        let byte = digest[i / 2];
        let nibble = if i % 2 == 0 { byte >> 4 } else { byte & 0x0f };
        if c.is_ascii_alphabetic() && nibble >= 8 {
            out.push(c.to_ascii_uppercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// Parse a `0x`-prefixed 20-byte address.
///
/// EIP-55 rule: hex that is entirely lowercase or entirely uppercase (digits carry no
/// case) is accepted as unchecksummed. Hex that mixes cases is a claim of checksum
/// correctness and must match the EIP-55 checksum exactly, so a typo in a pasted
/// address is rejected instead of silently naming a different account.
pub fn parse_address(s: &str) -> Result<[u8; 20], String> {
    let raw = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).ok_or("address must start with 0x")?;
    if raw.len() != 40 {
        return Err(format!("address must be 20 bytes, got {} hex chars", raw.len()));
    }
    let bytes = hex::decode(raw).map_err(|e| format!("invalid address hex: {e}"))?;
    let mut out = [0u8; 20];
    out.copy_from_slice(&bytes);
    let has_lower = raw.bytes().any(|b| (b'a'..=b'f').contains(&b));
    let has_upper = raw.bytes().any(|b| (b'A'..=b'F').contains(&b));
    if has_lower && has_upper && to_checksum(&out)[2..] != *raw {
        return Err("address checksum does not match (EIP-55): check for a typo, or use all lowercase".to_string());
    }
    Ok(out)
}

/// Parse a `0x`-prefixed 32-byte value.
pub fn parse_bytes32(s: &str) -> Result<[u8; 32], String> {
    let raw = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).ok_or("value must start with 0x")?;
    if raw.len() != 64 {
        return Err(format!("value must be 32 bytes, got {} hex chars", raw.len()));
    }
    let bytes = hex::decode(raw).map_err(|e| format!("invalid hex: {e}"))?;
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(s: &str) -> [u8; 32] {
        parse_bytes32(s).unwrap()
    }

    #[test]
    fn bridge_out_domain_matches_contract_constant() {
        // Pinned by foundry/test/BridgedIcUSD.t.sol (BRIDGE_OUT_DOMAIN).
        assert_eq!(
            keccak256(BRIDGE_OUT_DOMAIN.as_bytes()),
            h("0xbf6555f205691a2cd4c2403b214c0d516fdb66c929882e857755b63e90f1d4f1")
        );
    }

    #[test]
    fn transfer_id_matches_contract_vector() {
        // foundry/test/BridgedIcUSD.t.sol test_transfer_id_pinned_vector.
        let contract = parse_address("0x00000000000000000000000000000000000B45E1").unwrap();
        let from = parse_address("0x00000000000000000000000000000000000A11CE").unwrap();
        let mut nonce = [0u8; 32];
        nonce[31] = 1;
        assert_eq!(
            bridge_out_transfer_id(8453, &contract, &from, &nonce),
            h("0xf2f74fbfe0e8d729020d775d796ee6ff86aa29273d4873986667fde1c136336c")
        );
    }

    #[test]
    fn mint_and_remint_ids_are_domain_separated() {
        let contract = [7u8; 20];
        let salt = [9u8; 32];
        let a = mint_id(8453, &contract, &salt, 1);
        let b = mint_id(8453, &contract, &salt, 2);
        let c = mint_id(84532, &contract, &salt, 1);
        let d = mint_id(8453, &[8u8; 20], &salt, 1);
        let e = mint_id(8453, &contract, &[1u8; 32], 1);
        let mut tid = [0u8; 32];
        tid[24..].copy_from_slice(&1u64.to_be_bytes());
        let r = remint_id(8453, &contract, &salt, &tid);
        let all = [a, b, c, d, e, r];
        for i in 0..all.len() {
            for j in (i + 1)..all.len() {
                assert_ne!(all[i], all[j], "ids {i} and {j} collide");
            }
        }
        // A re-mint id for a transfer id equal to the deposit-id word still differs.
        assert_ne!(r, a);
    }

    #[test]
    fn memos_are_prefixed_and_distinct() {
        assert_eq!(pull_memo(5).len(), 16);
        assert_ne!(pull_memo(5), refund_memo(5));
        assert_ne!(pull_memo(5), sweep_memo(5));
        assert_eq!(&pull_memo(0x0102)[..8], b"RBRG-DEP");
        assert_eq!(&pull_memo(0x0102)[8..], &0x0102u64.to_be_bytes());
        assert_eq!(release_memo(&[3u8; 32]).len(), 32);
    }

    #[test]
    fn derivation_paths_are_purpose_and_chain_separated() {
        assert_ne!(derivation_path(LaneKind::Minter, 8453), derivation_path(LaneKind::Admin, 8453));
        assert_ne!(derivation_path(LaneKind::Minter, 8453), derivation_path(LaneKind::Minter, 84532));
        let p = derivation_path(LaneKind::Minter, 8453);
        assert_eq!(p[0], b"rumi-icusd-base-bridge".to_vec());
        assert_eq!(p[3], 8453u64.to_be_bytes().to_vec());
    }

    #[test]
    fn escrow_subaccount_is_stable() {
        // Changing the label would orphan escrow. Pin it.
        assert_eq!(escrow_subaccount(8453), <[u8; 32]>::from(Sha256::digest(b"rumi-icusd-base-bridge:escrow:v1")));
        assert_ne!(escrow_subaccount(8453), [0u8; 32]);
    }

    /// Golden ids captured from the code at `1701ebf7`, before the slug templates existed
    /// (old `MINT_DOMAIN` / `REMINT_DOMAIN` constants), for the fixed inputs below, and
    /// reproduced independently with a separate Keccak-256 implementation.
    const GOLDEN: [(u64, &str, &str); 3] = [
        (8453, "0xe832e5b43d7498bf7fd52efb4dcf017a4e0870414fb03447b3b6371283160e02", "0x2dc1718344e885e506ccc7f9591c66a98e58294875982b51161c2757bc10e5f8"),
        (84532, "0x437ecba535fd69ab9e2409a8d8dd4cd45e3d6a859aa098e5bcfba6cb06a8a71f", "0x7b18bc8905b7aa4e969029b4b75f0e290b337b19e2820ed6803c6593353c3116"),
        (31337, "0x2f10e7ab851dca2a36bc24fb72c30b49db31df520021de08c2c7d56fe26aa8b0", "0xc86b92345dcf0173ede638eca1765bbe59bf3f50165f2c5282cdd048cbab9c50"),
    ];

    fn golden_inputs() -> ([u8; 20], [u8; 32], [u8; 32], u64) {
        let contract: [u8; 20] = core::array::from_fn(|i| 0xb0 + i as u8);
        let salt: [u8; 32] = core::array::from_fn(|i| 0x01 + i as u8);
        let transfer_id: [u8; 32] = core::array::from_fn(|i| 0xa0 + i as u8);
        (contract, salt, transfer_id, 42)
    }

    #[test]
    fn base_slug_reproduces_live_strings() {
        let (contract, salt, transfer_id, deposit_id) = golden_inputs();
        for (chain, golden_mint, golden_remint) in GOLDEN {
            let slug = chain_slug(chain);
            assert_eq!(slug, "base", "chain {chain}");

            // Domains: literals copied from the pre-change constants.
            assert_eq!(mint_domain(slug), "rumi.icusd.base.mint.v1", "chain {chain}");
            assert_eq!(remint_domain(slug), "rumi.icusd.base.remint.v1", "chain {chain}");
            assert_eq!(BRIDGE_OUT_DOMAIN, "rumi.icusd.base.bridge-out.v1");
            assert_eq!(CONTRACT_VERSION, "rumi-base-icusd-bridge-v1");

            // Escrow subaccount: SHA-256 of the pre-change label.
            assert_eq!(escrow_label(slug), "rumi-icusd-base-bridge:escrow:v1", "chain {chain}");
            assert_eq!(escrow_subaccount(chain), <[u8; 32]>::from(Sha256::digest(b"rumi-icusd-base-bridge:escrow:v1")), "chain {chain}");

            // Key paths: the literal vector, byte for byte.
            let chain_be = chain.to_be_bytes().to_vec();
            assert_eq!(chain_be.len(), 8);
            assert_eq!(key_root(slug), "rumi-icusd-base-bridge", "chain {chain}");
            assert_eq!(
                derivation_path(LaneKind::Minter, chain),
                vec![b"rumi-icusd-base-bridge".to_vec(), b"v1".to_vec(), b"minter".to_vec(), chain_be.clone()],
                "chain {chain}"
            );
            assert_eq!(
                derivation_path(LaneKind::Admin, chain),
                vec![b"rumi-icusd-base-bridge".to_vec(), b"v1".to_vec(), b"admin".to_vec(), chain_be],
                "chain {chain}"
            );

            // Install salt label.
            assert_eq!(instance_salt_label(slug), "rumi-icusd-base-bridge:instance-salt:v1", "chain {chain}");

            // Mint and re-mint ids equal values captured from the unchanged code.
            assert_eq!(hex0x(&mint_id(chain, &contract, &salt, deposit_id)), golden_mint, "mint id, chain {chain}");
            assert_eq!(hex0x(&remint_id(chain, &contract, &salt, &transfer_id)), golden_remint, "re-mint id, chain {chain}");
        }
    }

    #[test]
    fn other_slugs_never_reproduce_a_base_string() {
        // Slugs of the chains the groundwork plan names, passed straight to the templates to
        // prove a different slug moves every string. (`non_base_rows_resolve_to_their_own_slug`
        // checks the table rows reach these slugs through the chain id.)
        let base_strings = [
            mint_domain("base"),
            remint_domain("base"),
            escrow_label("base"),
            key_root("base"),
            instance_salt_label("base"),
            BRIDGE_OUT_DOMAIN.to_string(),
            CONTRACT_VERSION.to_string(),
        ];
        let templates: [fn(&str) -> String; 5] = [mint_domain, remint_domain, escrow_label, key_root, instance_salt_label];
        for slug in ["eth", "arb", "rhood", "hyper", "monad"] {
            for t in templates {
                let s = t(slug);
                assert!(s.contains(slug), "{s} must carry its slug");
                for b in &base_strings {
                    assert_ne!(&s, b, "slug {slug} reproduced a Base string");
                }
            }
            assert_ne!(Sha256::digest(escrow_label(slug).as_bytes())[..], Sha256::digest(escrow_label("base").as_bytes())[..]);
            assert_ne!(keccak256(mint_domain(slug).as_bytes()), keccak256(mint_domain("base").as_bytes()));
            assert_ne!(keccak256(remint_domain(slug).as_bytes()), keccak256(remint_domain("base").as_bytes()));
            assert_ne!(key_root(slug).into_bytes(), b"rumi-icusd-base-bridge".to_vec());
        }
    }

    #[test]
    fn non_base_rows_resolve_to_their_own_slug() {
        // The table now describes the other chains (install still refuses them). Their ids
        // must hash under their own slug, never under `base`.
        let (contract, salt, transfer_id, deposit_id) = golden_inputs();
        let base_mint = mint_id(8453, &contract, &salt, deposit_id);
        for (chain, slug) in [
            (1u64, "eth"),
            (11155111, "eth"),
            (42161, "arb"),
            (421614, "arb"),
            (4663, "rhood"),
            (46630, "rhood"),
            (999, "hyper"),
            (998, "hyper"),
            (143, "monad"),
            (10143, "monad"),
        ] {
            assert_eq!(chain_slug(chain), slug, "chain {chain}");
            assert_eq!(escrow_subaccount(chain), <[u8; 32]>::from(Sha256::digest(format!("rumi-icusd-{slug}-bridge:escrow:v1").as_bytes())), "chain {chain}");
            assert_ne!(escrow_subaccount(chain), escrow_subaccount(8453), "chain {chain}");
            assert_ne!(mint_id(chain, &contract, &salt, deposit_id), base_mint, "chain {chain}");
            assert_eq!(derivation_path(LaneKind::Minter, chain)[0], format!("rumi-icusd-{slug}-bridge").into_bytes(), "chain {chain}");
            assert_ne!(remint_id(chain, &contract, &salt, &transfer_id), remint_id(8453, &contract, &salt, &transfer_id), "chain {chain}");
        }
    }

    #[test]
    fn an_unknown_chain_id_panics_instead_of_falling_back_to_base() {
        // Ids with no row at all: the unset value, near misses of Base ids, an id the groundwork
        // plan does not name (Conflux), and the extremes.
        for chain in [0u64, 10, 1030, 8454, 84531, 31338, 11155112, u64::MAX] {
            let msg = std::panic::catch_unwind(|| chain_slug(chain)).expect_err("must panic").downcast::<String>().expect("string panic message");
            assert_eq!(*msg, format!("no bridge chain profile for chain id {chain}"));
            assert!(std::panic::catch_unwind(|| escrow_subaccount(chain)).is_err());
            assert!(std::panic::catch_unwind(|| derivation_path(LaneKind::Minter, chain)).is_err());
            assert!(std::panic::catch_unwind(|| mint_id(chain, &[0; 20], &[0; 32], 1)).is_err());
            assert!(std::panic::catch_unwind(|| remint_id(chain, &[0; 20], &[0; 32], &[0; 32])).is_err());
        }
    }

    #[test]
    fn address_parsing_is_strict() {
        assert!(parse_address("0x0000000000000000000000000000000000000001").is_ok());
        assert!(parse_address("0000000000000000000000000000000000000001").is_err());
        assert!(parse_address("0x00000000000000000000000000000000000001").is_err());
        assert!(parse_address("0xzz00000000000000000000000000000000000001").is_err());
    }

    /// The official test vectors from EIP-55.
    const EIP55_VECTORS: [&str; 4] = [
        "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed",
        "0xfB6916095ca1df60bB79Ce92cE3Ea74c37c5d359",
        "0xdbF03B407c01E7cD3CBea99509d93f8DDDC8C6FB",
        "0xD1220A0cf47c7B9Be7A2E6BA89F429762e7b9aDb",
    ];

    #[test]
    fn eip55_vectors_render_and_parse() {
        for v in EIP55_VECTORS {
            let parsed = parse_address(v).unwrap_or_else(|e| panic!("{v} must parse: {e}"));
            assert_eq!(to_checksum(&parsed), v, "to_checksum must reproduce the EIP-55 vector");
            assert_eq!(hex0x(&parsed), v.to_ascii_lowercase(), "hex0x stays lowercase");
        }
    }

    #[test]
    fn unchecksummed_all_lower_and_all_upper_are_accepted() {
        for v in EIP55_VECTORS {
            let lower = v.to_ascii_lowercase();
            let upper = format!("0x{}", v[2..].to_ascii_uppercase());
            let checksummed = parse_address(v).unwrap();
            assert_eq!(parse_address(&lower).unwrap(), checksummed);
            assert_eq!(parse_address(&upper).unwrap(), checksummed);
            // The `0X` prefix form is still accepted.
            assert_eq!(parse_address(&format!("0X{}", &lower[2..])).unwrap(), checksummed);
        }
        // Digits-only addresses carry no case at all.
        assert!(parse_address("0x1111111111111111111111111111111111111111").is_ok());
    }

    #[test]
    fn mixed_case_with_a_wrong_checksum_is_rejected() {
        for v in EIP55_VECTORS {
            let body: Vec<char> = v[2..].chars().collect();
            // Flip the case of every letter, one at a time: each single flip must be caught.
            let mut flipped_any = false;
            for (i, c) in body.iter().enumerate() {
                if !c.is_ascii_alphabetic() {
                    continue;
                }
                flipped_any = true;
                let mut mutated = body.clone();
                mutated[i] = if c.is_ascii_uppercase() { c.to_ascii_lowercase() } else { c.to_ascii_uppercase() };
                let text = format!("0x{}", mutated.into_iter().collect::<String>());
                let err = parse_address(&text).expect_err(&format!("{text} has a one-character case flip"));
                assert!(err.contains("checksum does not match"), "unexpected error: {err}");
            }
            assert!(flipped_any);
        }
    }

    #[test]
    fn checksum_error_does_not_leak_into_other_malformations() {
        // Length and prefix errors keep their own messages.
        assert!(parse_address("0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAe").unwrap_err().contains("20 bytes"));
        assert!(parse_address("5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed").unwrap_err().contains("start with 0x"));
        assert!(parse_address("0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAeg").unwrap_err().contains("invalid address hex"));
    }

    #[test]
    fn checksum_helper_is_deterministic_and_idempotent_through_parse() {
        let addr = [0xabu8; 20];
        let c = to_checksum(&addr);
        assert!(c.starts_with("0x") && c.len() == 42);
        assert_eq!(parse_address(&c).unwrap(), addr);
        assert_eq!(c.to_ascii_lowercase(), hex0x(&addr));
    }
}
