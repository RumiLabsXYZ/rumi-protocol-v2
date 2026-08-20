//! tECDSA address derivation for Monad (secp256k1).
//!
//! Pure helpers (pubkey -> address, derivation paths) are unit-tested against
//! the canonical k=1 vector. The async `ecdsa_public_key` call hits the
//! management canister and is covered by the PocketIC integration test (Task 17)
//! and manual staging (Task 23).

use crate::chains::config::ChainId;
use candid::Principal;
use ic_cdk::api::management_canister::ecdsa::{
    ecdsa_public_key, EcdsaCurve, EcdsaKeyId, EcdsaPublicKeyArgument,
};
use k256::elliptic_curve::sec1::ToEncodedPoint;
use k256::PublicKey;
use sha3::{Digest, Keccak256};

use crate::state::read_state;

/// Derivation path for a per-user collateral custody address.
/// `[chain_id (LE u32), principal bytes, nonce (LE u64)]`.
pub fn custody_derivation_path(chain: ChainId, user: Principal, nonce: u64) -> Vec<Vec<u8>> {
    vec![
        chain.0.to_le_bytes().to_vec(),
        user.as_slice().to_vec(),
        nonce.to_le_bytes().to_vec(),
    ]
}

/// Derivation path for the per-chain settlement (minter) address.
pub fn settlement_derivation_path(chain: ChainId) -> Vec<Vec<u8>> {
    vec![chain.0.to_le_bytes().to_vec(), b"settlement".to_vec()]
}

fn key_id() -> EcdsaKeyId {
    // The key name is runtime-configurable (State::chains_ecdsa_key_name): default
    // `test_key_1` (staging/testnet), `key_1` on a production canister. Only ever
    // called from async canister paths (derive/sign), so reading State is safe.
    EcdsaKeyId {
        curve: EcdsaCurve::Secp256k1,
        name: read_state(|s| s.chains_ecdsa_key_name.clone()),
    }
}

/// Convert a secp256k1 public key (33-byte compressed or 65-byte uncompressed)
/// to a lowercase 0x EVM address: keccak256(uncompressed[1..])[12..].
pub fn evm_address_from_pubkey(pubkey: &[u8]) -> Result<String, String> {
    let pk = PublicKey::from_sec1_bytes(pubkey).map_err(|e| format!("bad pubkey: {e}"))?;
    let uncompressed = pk.to_encoded_point(false); // 0x04 || X(32) || Y(32)
    let bytes = uncompressed.as_bytes();
    if bytes.len() != 65 {
        return Err(format!("expected 65-byte uncompressed pubkey, got {}", bytes.len()));
    }
    let hash = Keccak256::digest(&bytes[1..]); // drop the 0x04 prefix
    let addr = &hash[12..]; // last 20 bytes
    Ok(format!("0x{}", hex::encode(addr)))
}

/// True iff `s` is a well-formed EVM address: a `0x`/`0X` prefix followed by
/// EXACTLY 40 hex digits (20 bytes), case-insensitive. This is the format the
/// tx-building helpers (`tx::abi_word_address`/`parse_address`) require; an
/// address that passes this can never panic those helpers. (Format only — no
/// EIP-55 checksum; derived addresses are lowercase and RPC responses vary.)
pub fn is_valid_evm_address(s: &str) -> bool {
    let hex = match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(h) => h,
        None => return false,
    };
    hex.len() == 40 && hex.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Async: fetch the derived public key from the management canister and return
/// both the raw pubkey and the EVM address. Used by deposit-address queries and
/// by the settlement worker to learn its minter address.
pub async fn derive_evm_address(derivation_path: Vec<Vec<u8>>) -> Result<(Vec<u8>, String), String> {
    let arg = EcdsaPublicKeyArgument {
        canister_id: None,
        derivation_path,
        key_id: key_id(),
    };
    let (res,) = ecdsa_public_key(arg).await.map_err(|(code, msg)| format!("{code:?}: {msg}"))?;
    let addr = evm_address_from_pubkey(&res.public_key)?;
    Ok((res.public_key, addr))
}

// ─── Settlement-address cache (Task 11 review M1; wired Task 15) ────────────────

thread_local! {
    static SETTLEMENT_ADDR_CACHE: std::cell::RefCell<std::collections::BTreeMap<ChainId, String>> =
        const { std::cell::RefCell::new(std::collections::BTreeMap::new()) };
}

/// Cached settlement (minter) address for `chain`. Derives + caches on first
/// use; returns the cached value thereafter. The address is deterministic
/// (`settlement_derivation_path` has no nonce), so caching is always correct;
/// the cache is a thread_local (not persisted) and simply re-derives once per
/// chain after an upgrade. Returns (path, address) so callers that also need the
/// derivation path for signing get both.
///
/// SETTLEMENT-ONLY: this caches the SETTLEMENT address exclusively (the key is
/// `chain`, and the path is always `settlement_derivation_path(chain)`). It must
/// NEVER be used for a per-vault custody address — those derive from
/// `custody_derivation_path(chain, user, nonce)` and are per-vault, not per-chain.
pub async fn cached_settlement_address(chain: ChainId) -> Result<(Vec<Vec<u8>>, String), String> {
    let path = settlement_derivation_path(chain);
    // Synchronous cache read — the borrow is dropped before any `.await`.
    if let Some(addr) = SETTLEMENT_ADDR_CACHE.with(|c| c.borrow().get(&chain).cloned()) {
        return Ok((path, addr));
    }
    // Captured BEFORE the `.await` below: see the key-generation guard section
    // further down this file for why this is load-bearing, not decorative.
    let captured_generation = current_ecdsa_key_generation();
    let (_pubkey, addr) = derive_evm_address(path.clone()).await?;
    let addr = SETTLEMENT_ADDR_CACHE
        .with(|c| commit_if_generation_current(c, chain, addr, captured_generation))?;
    Ok((path, addr))
}

// ─── Interest-treasury address (Task 12) ───────────────────────────────────────

/// Derivation path for the per-chain interest-treasury (revenue) address.
/// Distinct from the settlement (minter) path so realized interest revenue is
/// held separately from the operational hot wallet while staying
/// canister-controlled (it can later be swept via the same custody-withdrawal
/// machinery).
pub fn interest_treasury_derivation_path(chain: ChainId) -> Vec<Vec<u8>> {
    vec![chain.0.to_le_bytes().to_vec(), b"interest-treasury".to_vec()]
}

thread_local! {
    static INTEREST_TREASURY_ADDR_CACHE: std::cell::RefCell<std::collections::BTreeMap<ChainId, String>> =
        const { std::cell::RefCell::new(std::collections::BTreeMap::new()) };
}

/// Cached per-chain interest-treasury address (Task 12). Mirrors
/// `cached_settlement_address`: derives + caches on first use (deterministic, no
/// nonce). The minter (settlement) address SIGNS interest mints; this address is
/// only the `to:` recipient that receives the minted interest revenue. Returns
/// (path, address).
pub async fn cached_interest_treasury_address(
    chain: ChainId,
) -> Result<(Vec<Vec<u8>>, String), String> {
    let path = interest_treasury_derivation_path(chain);
    if let Some(addr) = INTEREST_TREASURY_ADDR_CACHE.with(|c| c.borrow().get(&chain).cloned()) {
        return Ok((path, addr));
    }
    let captured_generation = current_ecdsa_key_generation();
    let (_pubkey, addr) = derive_evm_address(path.clone()).await?;
    let addr = INTEREST_TREASURY_ADDR_CACHE
        .with(|c| commit_if_generation_current(c, chain, addr, captured_generation))?;
    Ok((path, addr))
}

// ─── Liquidation-reserve address (Increment 3, spec §4.8) ───────────────────────

/// Derivation path for the per-chain liquidation-RESERVE (PSM sink) address.
/// Distinct from settlement/interest/custody paths. Bot-liquidation swaps settle
/// their USDC output here; the bridge sweeps FROM it (out of scope). The vault's
/// OWN custody key signs the swap (it holds the CFX); this is only the `to:`.
pub fn reserve_derivation_path(chain: ChainId) -> Vec<Vec<u8>> {
    vec![chain.0.to_le_bytes().to_vec(), b"liquidation-reserve".to_vec()]
}

thread_local! {
    static RESERVE_ADDR_CACHE: std::cell::RefCell<std::collections::BTreeMap<ChainId, String>> =
        const { std::cell::RefCell::new(std::collections::BTreeMap::new()) };
}

/// Cached per-chain liquidation-reserve address (spec §4.8). Mirrors
/// `cached_settlement_address`: deterministic (no nonce), derives + caches on
/// first use. Resolved at swap submit (the USDC `to:`) and at confirm (to match
/// the realized `Transfer(_, reserve, amount)` log). Returns (path, address).
pub async fn cached_reserve_address(chain: ChainId) -> Result<(Vec<Vec<u8>>, String), String> {
    let path = reserve_derivation_path(chain);
    if let Some(addr) = RESERVE_ADDR_CACHE.with(|c| c.borrow().get(&chain).cloned()) {
        return Ok((path, addr));
    }
    let captured_generation = current_ecdsa_key_generation();
    let (_pubkey, addr) = derive_evm_address(path.clone()).await?;
    let addr = RESERVE_ADDR_CACHE
        .with(|c| commit_if_generation_current(c, chain, addr, captured_generation))?;
    Ok((path, addr))
}

// ─── De-scaffold pass (2026-08-20): stale-cache fix for ECDSA key rotation ───
//
// BUG (found live on prod tfesu during this PR's own verification pass): all
// three caches above are keyed ONLY by `ChainId`, with no dependency on
// `State::chains_ecdsa_key_name`. `set_chains_ecdsa_key_name` changes which
// threshold key `key_id()` reads, but a warm cache entry short-circuits
// `derive_evm_address` entirely, so `cached_reserve_address` (and settlement/
// interest-treasury) kept returning the OLD key's address after a rotation.
// Confirmed on mainnet: `get_chain_reserve_address(1030)` under `test_key_1`
// then under `key_1` (after `set_chains_ecdsa_key_name("key_1")`) returned the
// IDENTICAL address, which is impossible for two independent root keys.
//
// The custody path (`custody_derivation_path`) is NOT cached (each call
// re-derives), so per-vault deposit addresses were never affected; this bug is
// scoped to the three per-chain caches here.
pub fn clear_address_caches() {
    SETTLEMENT_ADDR_CACHE.with(|c| c.borrow_mut().clear());
    INTEREST_TREASURY_ADDR_CACHE.with(|c| c.borrow_mut().clear());
    RESERVE_ADDR_CACHE.with(|c| c.borrow_mut().clear());
}

// ─── Key-generation guard (2026-08-20, structural fix; security review) ─────
//
// `clear_address_caches()` alone is NOT sufficient under IC async
// interleaving. Each `cached_*` fn above reads the current key name (via
// `key_id()`, called synchronously inside `derive_evm_address`, BEFORE its
// `.await`) and only writes the cache AFTER that `.await` resolves. IC
// canister execution can interleave a suspended message with any other:
//
//   1. `cached_reserve_address` starts under `test_key_1`, calls
//      `derive_evm_address`, and suspends at the management-canister await.
//   2. While suspended, `set_chains_ecdsa_key_name("key_1")` runs to
//      completion: it mutates `chains_ecdsa_key_name` AND calls
//      `clear_address_caches()`.
//   3. The suspended derive resumes. It still completes with the address
//      derived under the OLD key (`test_key_1`), and, pre-fix,
//      unconditionally wrote that stale address into the cache the rotation
//      had JUST cleared, re-poisoning it.
//
// `clear_address_caches()` cannot close this gap: it runs at step 2, but the
// poisoning write happens at step 3, strictly AFTER. The fix is a
// monotonically bumped generation counter, captured before the derive starts
// and re-checked immediately after it resolves; a mismatch means a rotation
// raced the derive, and the result is discarded (never cached, never
// returned to the caller) rather than trusted. `clear_address_caches()` is
// kept as defense in depth (it also empties an entry immediately, rather
// than leaving it to expire lazily on next read), but the generation guard
// is the actual invariant.
thread_local! {
    /// Heap-only (thread_local), NOT persisted: a canister upgrade resets
    /// this to 0. That is harmless: the three address caches are heap too and
    /// reset to EMPTY at the exact same moment (a fresh Wasm instance has no
    /// thread_local state at all), so generation 0 post-upgrade always pairs
    /// with empty caches, never with a stale entry from a prior key.
    static ECDSA_KEY_GENERATION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// The current key generation. Read synchronously before issuing a tECDSA
/// derive request; the caller re-checks it after the derive resolves via
/// `commit_if_generation_current`.
pub fn current_ecdsa_key_generation() -> u64 {
    ECDSA_KEY_GENERATION.with(|g| g.get())
}

/// Bump the key generation. Called by `set_chains_ecdsa_key_name` (main.rs)
/// on every SUCCESSFUL key change, alongside `clear_address_caches()`.
pub fn bump_ecdsa_key_generation() {
    ECDSA_KEY_GENERATION.with(|g| g.set(g.get().saturating_add(1)));
}

/// Sync commit gate shared by all three per-chain address caches (settlement,
/// interest-treasury, reserve): apply a freshly-derived address IF the
/// generation captured before the derive started still matches the CURRENT
/// generation; otherwise a key rotation raced the derive, and the stale
/// result is rejected (`Err`, not cached, not returned), so the caller
/// re-derives on its next call instead of trusting an address signed by a
/// key the canister no longer uses.
///
/// Deliberately a plain sync function (no `.await` inside): this is what lets
/// a test drive the exact interleaving deterministically without spinning up
/// an async runtime or a mock management canister: capture a generation,
/// bump it (simulating a rotation landing mid-derive), then call this with
/// the stale captured value and assert the result is rejected and the cache
/// stays empty.
fn commit_if_generation_current(
    cache: &std::cell::RefCell<std::collections::BTreeMap<ChainId, String>>,
    chain: ChainId,
    addr: String,
    captured_generation: u64,
) -> Result<String, String> {
    if current_ecdsa_key_generation() != captured_generation {
        return Err(format!(
            "ecdsa key changed while deriving the address for chain {}; discarding stale result, retry",
            chain.0
        ));
    }
    cache.borrow_mut().insert(chain, addr.clone());
    Ok(addr)
}

#[cfg(test)]
mod address_cache_invalidation_tests {
    // A descendant module of `tecdsa` (declared inline, not in a sibling
    // `tests_*.rs` file) so it can reach the private thread_locals directly
    // without adding test-only public accessors to production code.
    use super::{
        clear_address_caches, ChainId, INTEREST_TREASURY_ADDR_CACHE, RESERVE_ADDR_CACHE,
        SETTLEMENT_ADDR_CACHE,
    };

    fn warm_all_caches(chain: ChainId) {
        SETTLEMENT_ADDR_CACHE.with(|c| c.borrow_mut().insert(chain, "0xsettlement".to_string()));
        INTEREST_TREASURY_ADDR_CACHE.with(|c| c.borrow_mut().insert(chain, "0xtreasury".to_string()));
        RESERVE_ADDR_CACHE.with(|c| c.borrow_mut().insert(chain, "0xreserve".to_string()));
    }

    fn all_caches_empty() -> bool {
        SETTLEMENT_ADDR_CACHE.with(|c| c.borrow().is_empty())
            && INTEREST_TREASURY_ADDR_CACHE.with(|c| c.borrow().is_empty())
            && RESERVE_ADDR_CACHE.with(|c| c.borrow().is_empty())
    }

    #[test]
    fn clear_address_caches_empties_all_three_caches() {
        let chain = ChainId(1030);
        warm_all_caches(chain);
        assert!(!all_caches_empty(), "precondition: caches must be warm before clearing");

        clear_address_caches();

        assert!(
            all_caches_empty(),
            "settlement/interest-treasury/reserve caches must all be empty after a key rotation"
        );
    }

    #[test]
    fn clear_address_caches_is_a_noop_on_already_empty_caches() {
        // Calling clear before anything was ever cached (e.g. a fresh canister,
        // or a second rotation before any address lookup) must not panic.
        clear_address_caches();
        assert!(all_caches_empty());
    }

    #[test]
    fn clear_address_caches_clears_every_registered_chain_not_just_one() {
        warm_all_caches(ChainId(71));
        warm_all_caches(ChainId(1030));
        clear_address_caches();
        assert!(all_caches_empty(), "clear must drop entries for every chain, not just the last-warmed one");
    }
}

/// De-scaffold pass (2026-08-20): the deterministic interleaving regression
/// for the key-generation guard (security review finding, F8). Drives
/// `commit_if_generation_current` directly around the exact interleaving
/// window a real async derive cannot be paused at deterministically: capture
/// a generation, THEN bump it (simulating a `set_chains_ecdsa_key_name` that
/// lands while the derive is suspended at its management-canister await),
/// THEN attempt to commit the (now-stale-keyed) result.
#[cfg(test)]
mod ecdsa_key_generation_guard_tests {
    use super::{
        bump_ecdsa_key_generation, commit_if_generation_current, current_ecdsa_key_generation,
        ChainId, RESERVE_ADDR_CACHE, SETTLEMENT_ADDR_CACHE,
    };

    /// THE regression: a derive that started before a rotation, and resolves
    /// after it, must have its result discarded, not cached, not returned.
    #[test]
    fn stale_generation_result_is_discarded_and_never_cached() {
        let chain = ChainId(1030);
        // The derive "starts": capture the generation before the (simulated)
        // await, exactly as `cached_reserve_address` does.
        let captured = current_ecdsa_key_generation();
        // While the derive is "suspended", a key rotation lands and bumps the
        // generation (mirrors `set_chains_ecdsa_key_name`'s success path).
        bump_ecdsa_key_generation();
        // The derive "resumes" and tries to commit its OLD-key-derived result.
        let result = RESERVE_ADDR_CACHE.with(|c| {
            commit_if_generation_current(c, chain, "0xstale-pre-rotation-address".to_string(), captured)
        });
        assert!(
            result.is_err(),
            "a result whose captured generation no longer matches current must be rejected"
        );
        let cached = RESERVE_ADDR_CACHE.with(|c| c.borrow().get(&chain).cloned());
        assert_eq!(cached, None, "the stale address must never be written to the cache");
    }

    /// Mirror case: no rotation happens between capture and commit -> the
    /// result is trusted (cached AND returned), proving the guard does not
    /// reject legitimate, non-raced derives.
    #[test]
    fn matching_generation_result_is_committed_and_cached() {
        let chain = ChainId(71);
        let captured = current_ecdsa_key_generation();
        let result = SETTLEMENT_ADDR_CACHE.with(|c| {
            commit_if_generation_current(c, chain, "0xfresh-address".to_string(), captured)
        });
        assert_eq!(result, Ok("0xfresh-address".to_string()));
        let cached = SETTLEMENT_ADDR_CACHE.with(|c| c.borrow().get(&chain).cloned());
        assert_eq!(cached, Some("0xfresh-address".to_string()));
    }

    /// A SECOND rotation landing after commit does not retroactively un-cache
    /// an already-trusted entry (the guard only protects the write that is
    /// in flight when a rotation happens, not entries already committed under
    /// an earlier, still-current-at-the-time generation).
    #[test]
    fn already_committed_entries_survive_a_later_unrelated_bump() {
        let chain = ChainId(71);
        let captured = current_ecdsa_key_generation();
        RESERVE_ADDR_CACHE
            .with(|c| commit_if_generation_current(c, chain, "0xok".to_string(), captured))
            .expect("commit succeeds");
        bump_ecdsa_key_generation();
        let cached = RESERVE_ADDR_CACHE.with(|c| c.borrow().get(&chain).cloned());
        assert_eq!(
            cached,
            Some("0xok".to_string()),
            "a later bump must not retroactively evict an already-committed entry \
             (that is clear_address_caches's job, called explicitly by set_chains_ecdsa_key_name)"
        );
    }
}
