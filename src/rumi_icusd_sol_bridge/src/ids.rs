//! Identifiers, ledger memos, subaccounts and threshold-Ed25519 derivation paths for the
//! Solana bridge instance (`docs/coordination/multichain/chain-solana.md` sections 2 and 5,
//! design `2026-10-30-multichain-icusd-groundwork-design.md` section 5.1).
//!
//! Every identifier that crosses a trust boundary carries an explicit domain tag, so a
//! value produced for one purpose can never be accepted for another:
//!
//! | Id | Domain | Preimage fields (each length-prefixed) |
//! |----|--------|-----------------------------------------|
//! | mint id | `rumi.icusd.sol.mint.v1` | genesis hash, mint key, instance salt, ICP escrow-pull block index, recipient key, amount |
//! | re-mint id | `rumi.icusd.sol.remint.v1` | genesis hash, mint key, instance salt, return id of the held claim |
//! | return id | `rumi.icusd.sol.return.v1` | genesis hash, transaction signature |
//! | burn id | `rumi.icusd.sol.burn.v1` | genesis hash, mint key, instance salt, count, then each covered return id in order |
//! | refund id | `rumi.icusd.sol.refund.v1` | genesis hash, mint key, instance salt, signature of the stray transaction |
//!
//! `rumi.icusd.sol.burn.v1` is the domain the note adds to the profile (section 2.3). The
//! **refund id domain `rumi.icusd.sol.refund.v1` is newly proposed by BB-47**: the note
//! names a refund id (`rumi1|f|<refund_id>`, section 2.4) but no domain or preimage. Its
//! preimage is keyed by the stray transaction's signature only, so there is exactly one
//! refund per stray; the amount and recipient are properties of that transaction and are
//! deliberately not hashed (two different amounts for one stray must not yield two ids).
//!
//! # Hashing
//!
//! `sha256(domain_bytes || (u32_be(len(field)) || field)*)`. The note says "domain, then
//! length-prefixed ..." without fixing the prefix width; BB-47 fixes it at `u32` big-endian
//! and pins it with golden vectors. The domain is the one raw element: the five domains are
//! a closed set and none is a prefix of another (`domains_are_prefix_free`), so the first
//! bytes of a preimage identify its domain unambiguously. Integers are 8-byte big-endian.
//!
//! Derivation paths for the four threshold-Ed25519 roles are
//! `[rumi-icusd-sol-bridge, v1, role, genesis_hash_32]`. They are bound to this canister's
//! principal by the IC, so they can never equal any address `rumi_protocol_backend` derives
//! (design statement, not re-verified here).

use sha2::{Digest as _, Sha256};

/// Mint id domain (section 2.1).
pub const MINT_DOMAIN: &str = "rumi.icusd.sol.mint.v1";
/// Re-mint of a held claim (section 2.1).
pub const REMINT_DOMAIN: &str = "rumi.icusd.sol.remint.v1";
/// Burn of custodied returns (section 2.3; new domain to add to the profile).
pub const BURN_DOMAIN: &str = "rumi.icusd.sol.burn.v1";
/// Return id (section 2.2).
pub const RETURN_DOMAIN: &str = "rumi.icusd.sol.return.v1";
/// Refund of strays. NEWLY PROPOSED by BB-47 (section 2.4 names no domain).
pub const REFUND_DOMAIN: &str = "rumi.icusd.sol.refund.v1";

/// Every domain, for the prefix-freeness and disjointness tests.
pub const ALL_DOMAINS: [&str; 5] = [MINT_DOMAIN, REMINT_DOMAIN, BURN_DOMAIN, RETURN_DOMAIN, REFUND_DOMAIN];

/// Root segment of every derivation path.
pub const KEY_ROOT: &[u8] = b"rumi-icusd-sol-bridge";
/// Version segment of every derivation path.
pub const KEY_PATH_VERSION: &[u8] = b"v1";
/// Label hashed (SHA-256) into the escrow subaccount.
pub const ESCROW_LABEL: &str = "rumi-icusd-sol-bridge:escrow:v1";
/// Label hashed into the per-instance salt at install.
pub const INSTANCE_SALT_LABEL: &str = "rumi-icusd-sol-bridge:instance-salt:v1";
/// Version string `bridge_version` reports. (Base reports its contract `VERSION()`; Solana has
/// no program under the memo-plus-durable-nonce option, so this names the bridge itself.)
pub const BRIDGE_VERSION: &str = "rumi-sol-icusd-bridge-v1";

/// ICP ledger memo prefixes (8 bytes each) for escrow operations (`RSOL-*`, design 5.1).
pub const PULL_MEMO_PREFIX: &[u8; 8] = b"RSOL-DEP";
pub const REFUND_MEMO_PREFIX: &[u8; 8] = b"RSOL-RFD";
pub const SWEEP_MEMO_PREFIX: &[u8; 8] = b"RSOL-FEE";

/// 32-byte hash (cluster genesis hash, instance salt, ids, blockhash and nonce values).
pub type Hash32 = [u8; 32];

/// The four threshold-Ed25519 roles (section 2, "Keys"). Roles stay separate keys (least
/// privilege), so mint and burn are two-signer transactions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize, candid::CandidType)]
pub enum KeyRole {
    /// Pays fees and ATA rent, and is the durable-nonce authority.
    FeePayer,
    /// Mint authority of the escrow mint. Never set to `None`.
    MintAuthority,
    /// Owns the return associated token account.
    ReturnOwner,
    /// The mint account's own keypair; used once, at mint creation.
    MintAccount,
}

impl KeyRole {
    pub const ALL: [KeyRole; 4] = [KeyRole::FeePayer, KeyRole::MintAuthority, KeyRole::ReturnOwner, KeyRole::MintAccount];

    /// The path element for this role.
    pub fn label(&self) -> &'static [u8] {
        match self {
            KeyRole::FeePayer => b"fee-payer",
            KeyRole::MintAuthority => b"mint-authority",
            KeyRole::ReturnOwner => b"return-owner",
            KeyRole::MintAccount => b"mint-account",
        }
    }
}

/// `[rumi-icusd-sol-bridge, v1, role, genesis_hash_32]`. Network identity is a path element,
/// so a devnet key can never equal a mainnet key even if canister separation failed.
pub fn derivation_path(role: KeyRole, genesis_hash: &Hash32) -> Vec<Vec<u8>> {
    vec![KEY_ROOT.to_vec(), KEY_PATH_VERSION.to_vec(), role.label().to_vec(), genesis_hash.to_vec()]
}

/// The 32-byte escrow subaccount of the bridge canister: `sha256("rumi-icusd-sol-bridge:escrow:v1")`.
pub fn escrow_subaccount() -> [u8; 32] {
    Sha256::digest(ESCROW_LABEL.as_bytes()).into()
}

/// `sha256(INSTANCE_SALT_LABEL || canister id || install time (u64 big-endian))`, the same
/// shape the Base instance uses. A replacement instance gets a new salt.
pub fn instance_salt(canister_id: &[u8], install_time_ns: u64) -> Hash32 {
    let mut h = Sha256::new();
    h.update(INSTANCE_SALT_LABEL.as_bytes());
    h.update(canister_id);
    h.update(install_time_ns.to_be_bytes());
    h.finalize().into()
}

/// Incremental builder for `domain || (u32_be(len) || field)*`.
struct IdHasher(Sha256);

impl IdHasher {
    fn new(domain: &str) -> Self {
        let mut h = Sha256::new();
        h.update(domain.as_bytes());
        IdHasher(h)
    }

    fn field(&mut self, bytes: &[u8]) -> &mut Self {
        // Every field is far below 4 GiB; `try_from` keeps the width explicit.
        let len = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
        self.0.update(len.to_be_bytes());
        self.0.update(bytes);
        self
    }

    fn u64_field(&mut self, v: u64) -> &mut Self {
        self.field(&v.to_be_bytes())
    }

    fn finish(self) -> Hash32 {
        self.0.finalize().into()
    }
}

/// Mint id for a deposit (section 2.1): the idempotency key a Solana mint is reconciled by.
pub fn mint_id(
    genesis_hash: &Hash32,
    mint: &[u8; 32],
    instance_salt: &Hash32,
    escrow_pull_block_index: u64,
    recipient: &[u8; 32],
    amount: u64,
) -> Hash32 {
    let mut h = IdHasher::new(MINT_DOMAIN);
    h.field(genesis_hash).field(mint).field(instance_salt).u64_field(escrow_pull_block_index).field(recipient).u64_field(amount);
    h.finish()
}

/// Re-mint id for a held claim. Shape mirrors the Base re-mint id (keyed by the claim's id).
/// The note (section 2.1) names only the domain; the preimage is BB-47's reading of it.
pub fn remint_id(genesis_hash: &Hash32, mint: &[u8; 32], instance_salt: &Hash32, return_id: &Hash32) -> Hash32 {
    let mut h = IdHasher::new(REMINT_DOMAIN);
    h.field(genesis_hash).field(mint).field(instance_salt).field(return_id);
    h.finish()
}

/// Return id: the dedup key of a finalized valid return (section 2.2). `signature` is the
/// 64-byte first signature of the user's return transaction.
pub fn return_id(genesis_hash: &Hash32, signature: &[u8; 64]) -> Hash32 {
    let mut h = IdHasher::new(RETURN_DOMAIN);
    h.field(genesis_hash).field(signature);
    h.finish()
}

/// Most returns one burn id may cover. v1 burns one return per transaction; the cap only bounds
/// the quadratic duplicate check and is raised together with the batching change.
pub const MAX_COVERED_RETURNS: usize = 16;

/// Why an id could not be built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdError {
    /// A burn must cover at least one return.
    EmptyReturnList,
    /// One return cannot be covered twice by the same burn.
    DuplicateReturnId,
    /// More than [`MAX_COVERED_RETURNS`] returns.
    TooManyReturns,
}

/// Burn id (section 2.3): the ordered list of covered return ids, so the memo `rumi1|b|<id>`
/// pins exactly which returns a burn settles. v1 burns one return per transaction; the list
/// form is kept so batching later needs no new domain.
pub fn burn_id(
    genesis_hash: &Hash32,
    mint: &[u8; 32],
    instance_salt: &Hash32,
    covered_return_ids: &[Hash32],
) -> Result<Hash32, IdError> {
    if covered_return_ids.is_empty() {
        return Err(IdError::EmptyReturnList);
    }
    if covered_return_ids.len() > MAX_COVERED_RETURNS {
        return Err(IdError::TooManyReturns);
    }
    for (i, id) in covered_return_ids.iter().enumerate() {
        if covered_return_ids[..i].contains(id) {
            return Err(IdError::DuplicateReturnId);
        }
    }
    let mut h = IdHasher::new(BURN_DOMAIN);
    h.field(genesis_hash).field(mint).field(instance_salt).u64_field(covered_return_ids.len() as u64);
    for id in covered_return_ids {
        h.field(id);
    }
    Ok(h.finish())
}

/// Refund id for a stray (section 2.4). NEWLY PROPOSED domain and preimage (see module docs).
pub fn refund_id(genesis_hash: &Hash32, mint: &[u8; 32], instance_salt: &Hash32, stray_signature: &[u8; 64]) -> Hash32 {
    let mut h = IdHasher::new(REFUND_DOMAIN);
    h.field(genesis_hash).field(mint).field(instance_salt).field(stray_signature);
    h.finish()
}

fn prefixed_memo(prefix: &[u8; 8], id: u64) -> Vec<u8> {
    let mut m = Vec::with_capacity(16);
    m.extend_from_slice(prefix);
    m.extend_from_slice(&id.to_be_bytes());
    m
}

/// ICP ledger memo of an escrow pull (deposit).
pub fn pull_memo(deposit_id: u64) -> Vec<u8> {
    prefixed_memo(PULL_MEMO_PREFIX, deposit_id)
}

/// ICP ledger memo of a deposit refund.
pub fn refund_ledger_memo(deposit_id: u64) -> Vec<u8> {
    prefixed_memo(REFUND_MEMO_PREFIX, deposit_id)
}

/// ICP ledger memo of a fee sweep.
pub fn sweep_memo(sweep_id: u64) -> Vec<u8> {
    prefixed_memo(SWEEP_MEMO_PREFIX, sweep_id)
}

/// Release memo is the 32-byte return id itself.
pub fn release_memo(return_id: &Hash32) -> Vec<u8> {
    return_id.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GENESIS: Hash32 = [0x01; 32];
    const MINT: [u8; 32] = [0x44; 32];
    const SALT: Hash32 = [0x05; 32];
    const RECIPIENT: [u8; 32] = [0x77; 32];
    const SIG: [u8; 64] = [0x9a; 64];
    const SIG2: [u8; 64] = [0x9b; 64];

    // Every pinned hex below was produced by an independent implementation (Python
    // `hashlib.sha256` over the documented preimage `domain || (u32_be(len) || field)*`,
    // `tools/golden_reference.py --check`), not by this module. A change here is a change of
    // identity.
    const MINT_ID: &str = "fc12c4627d4e95f60c3956fb9d0e33736b275f48d37dbf4dfcb7abb7e860c56f";
    const RETURN_ID: &str = "f4ae3a40e9e15a2b5f32de2c5a5a7fa1ed58001b83711dcac38e8450be4e138d";
    const REMINT_ID: &str = "6252a417b976ff5c052a0b04300d0caec957aa06f32f000912959b2482c25523";
    const BURN_ID_ONE: &str = "6aab9b761a7635f366995434454b62e7091547efff7b5c45a4066dd82c566204";
    const RETURN_ID_TWO: &str = "7fee55fccd1c6d24e2cc7e2b04b467b7beea55942e3425dde8b534eb114cd1e1";
    const BURN_ID_TWO: &str = "293333a3500881d719aa99be80e7c04e00b03501440916bc9f8723caf5d413e4";
    const BURN_ID_TWO_REVERSED: &str = "5720cfe625179fd0a6953231ef6c6599a7ddb4d83c1f78ef5fbe9eb1991b0d20";
    const REFUND_ID: &str = "96d6cade4f0e24502b56f84c1ec633891f639ed7146de996e180ee0220b8a798";
    const ESCROW_SUBACCOUNT: &str = "12f37a581370e84a29695a45f63396cd0b13e8a51179e770e0a2ae7d02b720a4";
    const INSTANCE_SALT: &str = "820db831c5eb3b398804f95a7b5bc51f2ef42cf42cf5876e0afb18c4b1133680";

    #[test]
    fn domains_are_exactly_the_documented_strings() {
        assert_eq!(MINT_DOMAIN, "rumi.icusd.sol.mint.v1");
        assert_eq!(REMINT_DOMAIN, "rumi.icusd.sol.remint.v1");
        assert_eq!(BURN_DOMAIN, "rumi.icusd.sol.burn.v1");
        assert_eq!(RETURN_DOMAIN, "rumi.icusd.sol.return.v1");
        assert_eq!(REFUND_DOMAIN, "rumi.icusd.sol.refund.v1");
        assert_eq!(ESCROW_LABEL, "rumi-icusd-sol-bridge:escrow:v1");
    }

    #[test]
    fn domains_are_prefix_free() {
        for (i, a) in ALL_DOMAINS.iter().enumerate() {
            for (j, b) in ALL_DOMAINS.iter().enumerate() {
                if i != j {
                    assert!(!b.starts_with(a), "{a} is a prefix of {b}");
                }
            }
        }
    }

    #[test]
    fn mint_id_is_pinned() {
        assert_eq!(hex::encode(mint_id(&GENESIS, &MINT, &SALT, 7, &RECIPIENT, 100)), MINT_ID);
    }

    #[test]
    fn return_id_is_pinned() {
        assert_eq!(hex::encode(return_id(&GENESIS, &SIG)), RETURN_ID);
        assert_eq!(hex::encode(return_id(&GENESIS, &SIG2)), RETURN_ID_TWO);
    }

    #[test]
    fn remint_id_is_pinned_and_keyed_by_the_held_claim() {
        let ret = return_id(&GENESIS, &SIG);
        assert_eq!(hex::encode(remint_id(&GENESIS, &MINT, &SALT, &ret)), REMINT_ID);
        assert_ne!(remint_id(&GENESIS, &MINT, &SALT, &ret), remint_id(&GENESIS, &MINT, &SALT, &return_id(&GENESIS, &SIG2)));
    }

    #[test]
    fn burn_id_is_pinned_ordered_and_rejects_empty_or_duplicate_lists() {
        let r1 = return_id(&GENESIS, &SIG);
        let r2 = return_id(&GENESIS, &SIG2);
        assert_eq!(hex::encode(burn_id(&GENESIS, &MINT, &SALT, &[r1]).unwrap()), BURN_ID_ONE);
        assert_eq!(hex::encode(burn_id(&GENESIS, &MINT, &SALT, &[r1, r2]).unwrap()), BURN_ID_TWO);
        // The list is ordered: the same returns in another order are another burn.
        assert_eq!(hex::encode(burn_id(&GENESIS, &MINT, &SALT, &[r2, r1]).unwrap()), BURN_ID_TWO_REVERSED);
        assert_eq!(burn_id(&GENESIS, &MINT, &SALT, &[]), Err(IdError::EmptyReturnList));
        assert_eq!(burn_id(&GENESIS, &MINT, &SALT, &[r1, r2, r1]), Err(IdError::DuplicateReturnId));
        let many: Vec<Hash32> = (0..=MAX_COVERED_RETURNS as u8).map(|i| [i; 32]).collect();
        assert_eq!(burn_id(&GENESIS, &MINT, &SALT, &many), Err(IdError::TooManyReturns));
        assert!(burn_id(&GENESIS, &MINT, &SALT, &many[..MAX_COVERED_RETURNS]).is_ok());
    }

    #[test]
    fn refund_id_is_pinned_and_keyed_by_the_stray_signature_only() {
        assert_eq!(hex::encode(refund_id(&GENESIS, &MINT, &SALT, &SIG)), REFUND_ID);
        assert_ne!(refund_id(&GENESIS, &MINT, &SALT, &SIG), refund_id(&GENESIS, &MINT, &SALT, &SIG2));
    }

    #[test]
    fn escrow_subaccount_and_instance_salt_are_pinned() {
        assert_eq!(hex::encode(escrow_subaccount()), ESCROW_SUBACCOUNT);
        assert_eq!(hex::encode(instance_salt(&[0xbb; 10], 1_700_000_000_000_000_000)), INSTANCE_SALT);
        // A replacement instance (new install time or canister) gets a new salt.
        assert_ne!(instance_salt(&[0xbb; 10], 1), instance_salt(&[0xbb; 10], 2));
        assert_ne!(instance_salt(&[0xbb; 10], 1), instance_salt(&[0xbc; 10], 1));
    }

    #[test]
    fn domain_separation_same_fields_different_ids() {
        // The same genesis, mint and salt under each domain give five different ids, and
        // the same fields under the same domain are stable.
        let r = return_id(&GENESIS, &SIG);
        let all = [
            mint_id(&GENESIS, &MINT, &SALT, 7, &RECIPIENT, 100),
            remint_id(&GENESIS, &MINT, &SALT, &r),
            r,
            burn_id(&GENESIS, &MINT, &SALT, &[r]).unwrap(),
            refund_id(&GENESIS, &MINT, &SALT, &SIG),
        ];
        for i in 0..all.len() {
            for j in i + 1..all.len() {
                assert_ne!(all[i], all[j], "ids {i} and {j} collide");
            }
        }
        // Identical field bytes under two domains never collide.
        let mut a = IdHasher::new(MINT_DOMAIN);
        a.field(&GENESIS).field(&MINT);
        let mut b = IdHasher::new(REMINT_DOMAIN);
        b.field(&GENESIS).field(&MINT);
        assert_ne!(a.finish(), b.finish());
    }

    #[test]
    fn every_input_changes_the_mint_id() {
        let base = mint_id(&GENESIS, &MINT, &SALT, 7, &RECIPIENT, 100);
        assert_ne!(base, mint_id(&[0x02; 32], &MINT, &SALT, 7, &RECIPIENT, 100), "genesis hash (cluster)");
        assert_ne!(base, mint_id(&GENESIS, &[0x45; 32], &SALT, 7, &RECIPIENT, 100), "mint key");
        assert_ne!(base, mint_id(&GENESIS, &MINT, &[0x06; 32], 7, &RECIPIENT, 100), "instance salt");
        assert_ne!(base, mint_id(&GENESIS, &MINT, &SALT, 8, &RECIPIENT, 100), "escrow-pull block index");
        assert_ne!(base, mint_id(&GENESIS, &MINT, &SALT, 7, &[0x78; 32], 100), "recipient");
        assert_ne!(base, mint_id(&GENESIS, &MINT, &SALT, 7, &RECIPIENT, 101), "amount");
    }

    #[test]
    fn length_prefixing_removes_concatenation_ambiguity() {
        let mut a = IdHasher::new(MINT_DOMAIN);
        a.field(b"ab").field(b"c");
        let mut b = IdHasher::new(MINT_DOMAIN);
        b.field(b"a").field(b"bc");
        assert_ne!(a.finish(), b.finish());
    }

    #[test]
    fn derivation_paths_follow_the_documented_shape() {
        let genesis = [0xAB; 32];
        let expected_roles: [(KeyRole, &[u8]); 4] = [
            (KeyRole::FeePayer, b"fee-payer"),
            (KeyRole::MintAuthority, b"mint-authority"),
            (KeyRole::ReturnOwner, b"return-owner"),
            (KeyRole::MintAccount, b"mint-account"),
        ];
        for (role, label) in expected_roles {
            let path = derivation_path(role, &genesis);
            assert_eq!(path.len(), 4);
            assert_eq!(path[0], b"rumi-icusd-sol-bridge".to_vec());
            assert_eq!(path[1], b"v1".to_vec());
            assert_eq!(path[2], label.to_vec());
            assert_eq!(path[3], genesis.to_vec());
        }
        assert_eq!(KeyRole::ALL.len(), 4);
    }

    #[test]
    fn derivation_paths_differ_by_role_and_by_cluster() {
        let a = [0x01; 32];
        let b = [0x02; 32];
        let mut seen = std::collections::BTreeSet::new();
        for role in KeyRole::ALL {
            for g in [a, b] {
                assert!(seen.insert(derivation_path(role, &g)), "{role:?} {g:?} repeats a path");
            }
        }
        assert_eq!(seen.len(), 8);
    }

    #[test]
    fn ledger_memos_use_rsol_prefixes() {
        assert_eq!(pull_memo(5), [b"RSOL-DEP".as_slice(), &5u64.to_be_bytes()].concat());
        assert_eq!(refund_ledger_memo(5), [b"RSOL-RFD".as_slice(), &5u64.to_be_bytes()].concat());
        assert_eq!(sweep_memo(5), [b"RSOL-FEE".as_slice(), &5u64.to_be_bytes()].concat());
        assert_eq!(release_memo(&[9; 32]), vec![9u8; 32]);
    }
}
