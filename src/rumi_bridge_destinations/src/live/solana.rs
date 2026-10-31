//! Solana side of the live transport (feature `live-transport`, card BB-49, step 49e).
//!
//! Reads a Solana destination's mint supply and the balance of the canister-owned return account
//! from the cluster itself, never from the bridge instance's own report. The two numbers come from
//! ONE `getMultipleAccounts` response per provider at the `finalized` commitment, so each provider's
//! pair has a single context slot (the supply and the custody describe the same state), and a
//! quorum of providers must agree on both. The escrow `E_d` is read separately by the existing IC
//! transport, like every destination escrow.
//!
//! The provider rule mirrors the EVM and XRPL readers ([`super::evm`], [`super::xrpl`]) and reuses
//! their [`super::evm::JsonRpcTransport`]:
//!
//! 1. every provider is asked `getGenesisHash`, bound to the profile network's genesis hash
//!    (`network.identity`): a provider on another cluster is excluded from every vote. It is then
//!    asked `getSlot` at `finalized`;
//! 2. `n` is the `quorum`-th highest finalized slot reported (one far-ahead provider cannot raise
//!    it) and the profile's quorum must be met by the bound providers;
//! 3. every provider whose finalized slot is at or above `n` is asked `getMultipleAccounts` for
//!    `[mint, return account]` with commitment `finalized`, encoding `base64` and
//!    `minContextSlot = n`. The reply's context slot must be at least `n`; both accounts must exist,
//!    be owned by the configured token program, and parse (see below). A provider below `n` does
//!    not vote;
//! 4. a quorum of identical `(supply, custody)` answers wins. The reported slot is the LOWEST
//!    context slot among the providers that agreed.
//!
//! # What this does and does not prove
//!
//! Solana serves account state only at the node's own current commitment, so unlike the EVM and XRPL
//! readers this reader cannot pin the read to one numbered historical height. Providers answer at
//! their own finalized slots (all at or above `n`), and what the quorum shares is the pair of values.
//! A burn, mint or transfer that lands between two providers' reads therefore shows up as a
//! DISAGREEMENT and the collection fails rather than guessing; agreement of a quorum on identical
//! values is a limited consistency check, not proof that nothing moved around the reads.
//!
//! # Account layouts
//!
//! The accounts are read as raw bytes and parsed here (the SPL layouts), not taken from a provider's
//! `jsonParsed` interpretation. A mint must be initialized with 8 decimals; a token account must be
//! initialized and belong to the configured mint. For the classic program the data lengths are exact
//! (82 and 165); for Token-2022 the base layout is followed by an account-type byte (1 for a mint, 2
//! for a token account) when extensions are present. Anything else is a failed read.
//!
//! Providers are named by position in errors, never by URL. The configured addresses are operator
//! input: the mint and the return account must be 32-byte base58 keys and differ.

use serde::Deserialize;
use serde_json::{json, Value};

use super::evm::{call, tally, JsonRpcTransport};
use crate::collect::{ReadError, SolanaSupply};
use crate::types::{FinalityRule, IdentityKind, Network};

/// The decimals of the bridged icUSD mint (the profile's representation).
pub const ICUSD_DECIMALS: u8 = 8;

/// Largest account data accepted from a provider (Token-2022 extensions included).
const MAX_ACCOUNT_BYTES: usize = 4096;

/// The SPL Token program (classic).
pub const SPL_TOKEN_PROGRAM_B58: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
/// The SPL Token-2022 program.
pub const SPL_TOKEN_2022_PROGRAM_B58: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";

const MINT_LEN: usize = 82;
const TOKEN_ACCOUNT_LEN: usize = 165;
const ACCOUNT_TYPE_MINT: u8 = 1;
const ACCOUNT_TYPE_ACCOUNT: u8 = 2;

/// Which token program the mint and the return account belong to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum TokenProgram {
    /// The classic SPL Token program.
    #[serde(rename = "spl_token")]
    SplToken,
    /// SPL Token-2022 (the profile's recommended representation).
    #[serde(rename = "spl_token_2022")]
    SplToken2022,
}

impl TokenProgram {
    /// The program id, base58, as a provider reports an account `owner`.
    pub fn id_b58(self) -> &'static str {
        match self {
            TokenProgram::SplToken => SPL_TOKEN_PROGRAM_B58,
            TokenProgram::SplToken2022 => SPL_TOKEN_2022_PROGRAM_B58,
        }
    }
}

// ---------------------------------------------------------------------------
// base58 and base64 (no dependency is added)
// ---------------------------------------------------------------------------

const BASE58: &[u8; 58] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

/// A base58 text as exactly 32 bytes (a Solana key or hash).
pub fn base58_to_32(text: &str) -> Result<[u8; 32], String> {
    if text.is_empty() || text.len() > 44 {
        return Err("is not a 32-byte base58 key".into());
    }
    let mut digits: Vec<u8> = Vec::new(); // little-endian base 256
    for c in text.bytes() {
        let value = BASE58
            .iter()
            .position(|b| *b == c)
            .ok_or("has a character outside the base58 alphabet")? as u32;
        let mut carry = value;
        for d in digits.iter_mut() {
            carry += u32::from(*d) * 58;
            *d = (carry & 0xff) as u8;
            carry >>= 8;
        }
        while carry > 0 {
            digits.push((carry & 0xff) as u8);
            carry >>= 8;
        }
    }
    // Each leading '1' is one leading zero byte (the arithmetic above only produced the value's own
    // non-zero top bytes).
    let zeros = text.bytes().take_while(|b| *b == b'1').count();
    digits.extend(std::iter::repeat_n(0u8, zeros));
    digits.reverse();
    if digits.len() != 32 {
        return Err("is not a 32-byte base58 key".into());
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&digits);
    Ok(out)
}

/// Strict standard base64 (RFC 4648, with padding, no whitespace).
pub fn base64_decode(text: &str) -> Result<Vec<u8>, String> {
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return Err("base64 length is not a multiple of 4".into());
    }
    let value = |c: u8| -> Result<u32, String> {
        match c {
            b'A'..=b'Z' => Ok(u32::from(c - b'A')),
            b'a'..=b'z' => Ok(u32::from(c - b'a') + 26),
            b'0'..=b'9' => Ok(u32::from(c - b'0') + 52),
            b'+' => Ok(62),
            b'/' => Ok(63),
            _ => Err("base64 has a character outside the alphabet".into()),
        }
    };
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let chunks = bytes.len() / 4;
    for (i, chunk) in bytes.chunks(4).enumerate() {
        let last = i + 1 == chunks;
        let pad = chunk.iter().rev().take_while(|c| **c == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return Err("base64 padding is misplaced".into());
        }
        let mut n: u32 = 0;
        for (j, c) in chunk.iter().enumerate() {
            let v = if j >= 4 - pad { 0 } else { value(*c)? };
            n = (n << 6) | v;
        }
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
        // Canonical padding: the unused low bits must be zero.
        if (pad == 1 && (n & 0xff) != 0) || (pad == 2 && (n & 0xffff) != 0) {
            return Err("base64 has non-zero padding bits".into());
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Account layouts
// ---------------------------------------------------------------------------

fn u64_le(data: &[u8], at: usize) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&data[at..at + 8]);
    u64::from_le_bytes(b)
}

/// The supply of an initialized 8-decimal mint of `program`.
pub fn parse_mint(data: &[u8], program: TokenProgram) -> Result<u128, String> {
    if data.len() < MINT_LEN {
        return Err(format!(
            "the mint account is {} bytes, shorter than the {MINT_LEN} of a mint",
            data.len()
        ));
    }
    let shape_ok = match program {
        TokenProgram::SplToken => data.len() == MINT_LEN,
        TokenProgram::SplToken2022 => {
            data.len() == MINT_LEN
                || (data.len() > TOKEN_ACCOUNT_LEN && data[TOKEN_ACCOUNT_LEN] == ACCOUNT_TYPE_MINT)
        }
    };
    if !shape_ok {
        return Err(
            "the mint account has a layout that is not a mint of the configured program".into(),
        );
    }
    if data[45] != 1 {
        return Err("the mint is not initialized".into());
    }
    if data[44] != ICUSD_DECIMALS {
        return Err(format!(
            "the mint has {} decimals, the bridged icUSD has {ICUSD_DECIMALS}",
            data[44]
        ));
    }
    Ok(u128::from(u64_le(data, 36)))
}

/// The balance of an initialized token account of `program` that belongs to `mint`.
pub fn parse_token_account(
    data: &[u8],
    program: TokenProgram,
    mint: &[u8; 32],
) -> Result<u128, String> {
    if data.len() < TOKEN_ACCOUNT_LEN {
        return Err(format!(
            "the return account is {} bytes, shorter than the {TOKEN_ACCOUNT_LEN} of a token account",
            data.len()
        ));
    }
    let shape_ok = match program {
        TokenProgram::SplToken => data.len() == TOKEN_ACCOUNT_LEN,
        TokenProgram::SplToken2022 => {
            data.len() == TOKEN_ACCOUNT_LEN || data[TOKEN_ACCOUNT_LEN] == ACCOUNT_TYPE_ACCOUNT
        }
    };
    if !shape_ok {
        return Err(
            "the return account has a layout that is not a token account of the configured program"
                .into(),
        );
    }
    if data[..32] != mint[..] {
        return Err("the return account belongs to another mint".into());
    }
    if data[108] != 1 {
        return Err("the return account is not an initialized, unfrozen token account".into());
    }
    Ok(u128::from(u64_le(data, 64)))
}

// ---------------------------------------------------------------------------
// The target and the reading
// ---------------------------------------------------------------------------

/// What to read: the bridged mint, the canister-owned return account and the token program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub mint: String,
    pub mint_key: [u8; 32],
    pub return_account: String,
    pub program: TokenProgram,
}

impl Target {
    /// Validate the operator's input: both are 32-byte base58 keys and they differ.
    pub fn new(mint: &str, return_account: &str, program: TokenProgram) -> Result<Target, String> {
        let mint_key = base58_to_32(mint).map_err(|e| format!("the mint {e}"))?;
        let account_key =
            base58_to_32(return_account).map_err(|e| format!("the return account {e}"))?;
        if mint_key == account_key {
            return Err("the mint and the return account are the same key".into());
        }
        Ok(Target {
            mint: mint.to_string(),
            mint_key,
            return_account: return_account.to_string(),
            program,
        })
    }
}

/// The profile network's genesis hash.
fn expected_genesis(network: &Network) -> Result<String, String> {
    let id = &network.identity.value;
    if id.kind != IdentityKind::SolanaGenesisHash {
        return Err("the profile identity is not a Solana genesis hash".into());
    }
    Ok(id.value.clone())
}

fn finalized_slot(result: &Value) -> Result<u64, String> {
    result
        .as_u64()
        .ok_or_else(|| "the slot is not an unsigned integer".to_string())
}

/// One account of a `getMultipleAccounts` value: owned by `program`, base64 data.
fn account_bytes(entry: &Value, program: TokenProgram, what: &str) -> Result<Vec<u8>, String> {
    if entry.is_null() {
        return Err(format!("the {what} does not exist"));
    }
    let o = entry
        .as_object()
        .ok_or_else(|| format!("the {what} is not an object"))?;
    if o.get("owner").and_then(Value::as_str) != Some(program.id_b58()) {
        return Err(format!(
            "the {what} is not owned by the configured token program"
        ));
    }
    let data = o
        .get("data")
        .and_then(Value::as_array)
        .filter(|a| a.len() == 2 && a[1].as_str() == Some("base64"))
        .ok_or_else(|| format!("the {what} data is not [base64, \"base64\"]"))?;
    let text = data[0]
        .as_str()
        .ok_or_else(|| format!("the {what} data is not text"))?;
    if text.len() > MAX_ACCOUNT_BYTES * 2 {
        return Err(format!("the {what} data is too large"));
    }
    let bytes = base64_decode(text).map_err(|e| format!("the {what} data: {e}"))?;
    if bytes.len() > MAX_ACCOUNT_BYTES {
        return Err(format!("the {what} data is too large"));
    }
    Ok(bytes)
}

/// `(supply, custody, context slot)` from a `getMultipleAccounts` result asked at `minimum`.
fn pair_at(result: &Value, target: &Target, minimum: u64) -> Result<(u128, u128, u64), String> {
    let slot = result
        .get("context")
        .and_then(|c| c.get("slot"))
        .and_then(Value::as_u64)
        .ok_or("the answer has no context slot")?;
    if slot < minimum {
        return Err(format!(
            "the answer's context slot {slot} is below the agreed slot {minimum}"
        ));
    }
    let value = result
        .get("value")
        .and_then(Value::as_array)
        .filter(|v| v.len() == 2)
        .ok_or("the answer does not carry exactly the two requested accounts")?;
    let supply = parse_mint(
        &account_bytes(&value[0], target.program, "mint")?,
        target.program,
    )?;
    let custody = parse_token_account(
        &account_bytes(&value[1], target.program, "return account")?,
        target.program,
        &target.mint_key,
    )?;
    if custody > supply {
        return Err("the return account holds more than the mint's whole supply".into());
    }
    Ok((supply, custody, slot))
}

/// The mint supply and the return-account balance, read together at the quorum-agreed finalized slot.
pub fn read_supply_and_custody(
    t: &dyn JsonRpcTransport,
    target: &Target,
    network: &Network,
) -> Result<SolanaSupply, ReadError> {
    let fail = |e: String| ReadError::Failed(format!("{}: {e}", network.network_id));
    if network.finality.rule != FinalityRule::SolanaCommitment {
        return Err(fail(format!(
            "{:?} is not the Solana commitment rule",
            network.finality.rule
        )));
    }
    if network.finality.parameters.value.commitment.as_deref() != Some("finalized") {
        return Err(fail("the profile's commitment is not `finalized`".into()));
    }
    let quorum = usize::from(network.rpc.min_quorum.value);
    let count = t.provider_count();
    if quorum == 0 || quorum > count {
        return Err(fail(format!(
            "quorum {quorum} is not in 1..={count} configured providers"
        )));
    }
    let genesis = expected_genesis(network).map_err(fail)?;

    // 1. Cluster binding and the finalized slot of every provider.
    let tips: Vec<Result<u64, String>> = (0..count)
        .map(|i| {
            let reported = call(t, i, "getGenesisHash", json!([]))?;
            if reported.as_str() != Some(genesis.as_str()) {
                return Err("reports another cluster than the profile's genesis hash".to_string());
            }
            let slot = call(t, i, "getSlot", json!([{"commitment": "finalized"}]))?;
            finalized_slot(&slot)
        })
        .collect();
    let mut heights: Vec<u64> = tips
        .iter()
        .filter_map(|r| r.as_ref().ok())
        .copied()
        .collect();
    if heights.len() < quorum {
        let excluded: Vec<String> = tips
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.as_ref().err().map(|e| format!("p{} {e}", i + 1)))
            .collect();
        return Err(fail(format!(
            "only {} of {count} providers are on the profile's cluster with a finalized slot, quorum is {quorum}; excluded: {}",
            heights.len(),
            excluded.join("; ")
        )));
    }

    // 2. The agreed slot: the quorum-th highest.
    heights.sort_unstable_by(|a, b| b.cmp(a));
    let n = heights[quorum - 1];

    // 3. The pair from one response per provider at or above `n`.
    let votes: Vec<Result<(u128, u128, u64), String>> = tips
        .into_iter()
        .enumerate()
        .map(|(i, tip)| match tip {
            Ok(slot) if slot >= n => call(
                t,
                i,
                "getMultipleAccounts",
                json!([
                    [target.mint, target.return_account],
                    {"commitment": "finalized", "encoding": "base64", "minContextSlot": n}
                ]),
            )
            .and_then(|r| pair_at(&r, target, n)),
            Ok(slot) => Err(format!(
                "finalized slot {slot} is below the agreed slot {n}"
            )),
            Err(e) => Err(format!("excluded: {e}")),
        })
        .collect();

    // 4. A quorum of identical (supply, custody) answers.
    let pairs: Vec<Result<(u128, u128), String>> = votes
        .iter()
        .map(|v| v.clone().map(|(supply, custody, _)| (supply, custody)))
        .collect();
    let (supply, custody) = tally(
        pairs,
        quorum,
        &format!("supply and custody at finalized slot {n} or later"),
    )
    .map_err(fail)?;
    let slot = votes
        .iter()
        .filter_map(|v| v.as_ref().ok())
        .filter(|(s, c, _)| (*s, *c) == (supply, custody))
        .map(|(_, _, slot)| *slot)
        .min()
        .unwrap_or(n);
    Ok(SolanaSupply {
        supply,
        custody,
        finality: network.finality.rule,
        slot,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::live::evm::tests::{network, Mock, Script};

    pub(crate) const MAINNET_GENESIS: &str = "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d";
    const DEVNET_GENESIS: &str = "EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG";

    pub(crate) fn b58_encode(bytes: &[u8; 32]) -> String {
        let mut digits: Vec<u8> = Vec::new(); // little-endian base 58
        for b in bytes {
            let mut carry = u32::from(*b);
            for d in digits.iter_mut() {
                carry += u32::from(*d) << 8;
                *d = (carry % 58) as u8;
                carry /= 58;
            }
            while carry > 0 {
                digits.push((carry % 58) as u8);
                carry /= 58;
            }
        }
        let zeros = bytes.iter().take_while(|b| **b == 0).count();
        let mut out = "1".repeat(zeros);
        out.extend(digits.iter().rev().map(|d| BASE58[*d as usize] as char));
        out
    }

    pub(crate) fn b64_encode(bytes: &[u8]) -> String {
        const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let n = (u32::from(chunk[0]) << 16)
                | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
                | u32::from(*chunk.get(2).unwrap_or(&0));
            out.push(A[(n >> 18) as usize & 63] as char);
            out.push(A[(n >> 12) as usize & 63] as char);
            out.push(if chunk.len() > 1 {
                A[(n >> 6) as usize & 63] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                A[n as usize & 63] as char
            } else {
                '='
            });
        }
        out
    }

    pub(crate) const MINT_KEY: [u8; 32] = [0x11; 32];
    pub(crate) const ACCOUNT_KEY: [u8; 32] = [0x22; 32];

    pub(crate) fn mint_text() -> String {
        b58_encode(&MINT_KEY)
    }
    pub(crate) fn account_text() -> String {
        b58_encode(&ACCOUNT_KEY)
    }

    pub(crate) fn target(program: TokenProgram) -> Target {
        Target::new(&mint_text(), &account_text(), program).unwrap()
    }

    /// A mint account: `supply` and `decimals`, initialized or not; `extended` adds a Token-2022 type byte.
    pub(crate) fn mint_data(
        supply: u64,
        decimals: u8,
        initialized: bool,
        extended: bool,
    ) -> Vec<u8> {
        let mut d = vec![0u8; MINT_LEN];
        d[36..44].copy_from_slice(&supply.to_le_bytes());
        d[44] = decimals;
        d[45] = u8::from(initialized);
        if extended {
            d.resize(TOKEN_ACCOUNT_LEN + 1, 0);
            d[TOKEN_ACCOUNT_LEN] = ACCOUNT_TYPE_MINT;
            d.extend_from_slice(&[0xAB; 12]);
        }
        d
    }

    /// A token account of `mint` holding `amount`, in `state` (1 = initialized).
    pub(crate) fn account_data(mint: &[u8; 32], amount: u64, state: u8, extended: bool) -> Vec<u8> {
        let mut d = vec![0u8; TOKEN_ACCOUNT_LEN];
        d[..32].copy_from_slice(mint);
        d[32..64].copy_from_slice(&[0x33; 32]);
        d[64..72].copy_from_slice(&amount.to_le_bytes());
        d[108] = state;
        if extended {
            d.push(ACCOUNT_TYPE_ACCOUNT);
            d.extend_from_slice(&[0xCD; 9]);
        }
        d
    }

    fn entry(owner: &str, data: &[u8]) -> Value {
        json!({
            "data": [b64_encode(data), "base64"],
            "executable": false,
            "lamports": 2039280,
            "owner": owner,
            "rentEpoch": 0,
            "space": data.len()
        })
    }

    /// Knobs of one synthetic provider.
    #[derive(Clone)]
    pub(crate) struct P {
        pub genesis: String,
        /// Its finalized slot.
        pub slot: u64,
        /// Added to the finalized slot to give the context slot of its account reply.
        pub context_extra: u64,
        pub program: TokenProgram,
        pub mint: Option<Vec<u8>>,
        pub account: Option<Vec<u8>>,
        /// Overrides the owner reported for the return account.
        pub account_owner: Option<String>,
    }

    impl P {
        pub(crate) fn mainnet(program: TokenProgram, slot: u64, supply: u64, custody: u64) -> P {
            let extended = program == TokenProgram::SplToken2022;
            P {
                genesis: MAINNET_GENESIS.to_string(),
                slot,
                context_extra: 0,
                program,
                mint: Some(mint_data(supply, 8, true, extended)),
                account: Some(account_data(&MINT_KEY, custody, 1, extended)),
                account_owner: None,
            }
        }

        pub(crate) fn script(self) -> Script {
            Box::new(move |method, params| match method {
                "getGenesisHash" => Ok(json!(self.genesis)),
                "getSlot" => {
                    assert_eq!(params[0]["commitment"], json!("finalized"));
                    Ok(json!(self.slot))
                }
                "getMultipleAccounts" => {
                    let config = &params[1];
                    assert_eq!(config["commitment"], json!("finalized"), "{config}");
                    assert_eq!(config["encoding"], json!("base64"), "{config}");
                    let minimum = config["minContextSlot"]
                        .as_u64()
                        .expect("a numbered minContextSlot");
                    if self.slot < minimum {
                        return Err("Minimum context slot has not been reached".into());
                    }
                    let owner = self.program.id_b58();
                    let value = |data: &Option<Vec<u8>>, owner: &str| match data {
                        Some(d) => entry(owner, d),
                        None => Value::Null,
                    };
                    Ok(json!({
                        "context": {"slot": self.slot + self.context_extra},
                        "value": [
                            value(&self.mint, owner),
                            value(&self.account, self.account_owner.as_deref().unwrap_or(owner)),
                        ]
                    }))
                }
                other => Err(format!("unexpected method {other}")),
            })
        }
    }

    fn mock(providers: Vec<P>) -> Mock {
        Mock::new(providers.into_iter().map(P::script).collect())
    }

    fn net() -> Network {
        network("solana-mainnet")
    }

    #[test]
    fn base58_and_base64_decode_known_values() {
        // The two token programs, against their raw bytes.
        let classic = base58_to_32(SPL_TOKEN_PROGRAM_B58).unwrap();
        assert_eq!(classic[..4], [0x06, 0xdd, 0xf6, 0xe1]);
        assert_eq!(classic[31], 0xa9);
        let t22 = base58_to_32(SPL_TOKEN_2022_PROGRAM_B58).unwrap();
        assert_eq!(t22[..4], [0x06, 0xdd, 0xf6, 0xe1]);
        assert_eq!(t22[4..8], [0xee, 0x75, 0x8f, 0xde]);
        assert_eq!(t22[31], 0xfc);
        // Round trip, including leading zero bytes.
        for key in [[0x11u8; 32], [0u8; 32], {
            let mut k = [7u8; 32];
            k[0] = 0;
            k[1] = 0;
            k
        }] {
            assert_eq!(base58_to_32(&b58_encode(&key)).unwrap(), key);
        }
        // The System Program is 32 ones.
        assert_eq!(base58_to_32(&"1".repeat(32)).unwrap(), [0u8; 32]);
        // Rejections: a character outside the alphabet, the wrong length, empty.
        assert!(base58_to_32("0OIl").is_err());
        assert!(base58_to_32("").is_err());
        assert!(base58_to_32("abc").is_err());
        assert!(base58_to_32(&"z".repeat(45)).is_err());
        // The genesis hashes in the profiles are 32-byte keys.
        assert!(base58_to_32(MAINNET_GENESIS).is_ok() && base58_to_32(DEVNET_GENESIS).is_ok());

        for bytes in [
            vec![],
            vec![1u8],
            vec![1, 2],
            vec![1, 2, 3],
            (0u8..=255).collect::<Vec<u8>>(),
        ] {
            assert_eq!(base64_decode(&b64_encode(&bytes)).unwrap(), bytes);
        }
        assert!(base64_decode("AAA").is_err(), "length");
        assert!(base64_decode("AA=A").is_err(), "padding in the middle");
        assert!(base64_decode("A===").is_err(), "too much padding");
        assert!(base64_decode("AA\nA").is_err(), "whitespace");
        assert!(base64_decode("AB==").is_err(), "non-zero padding bits");
        assert!(base64_decode("QUJD").is_ok());
    }

    #[test]
    fn mint_and_token_account_layouts_are_strict() {
        for (program, extended) in [
            (TokenProgram::SplToken, false),
            (TokenProgram::SplToken2022, false),
            (TokenProgram::SplToken2022, true),
        ] {
            assert_eq!(
                parse_mint(&mint_data(123, 8, true, extended), program),
                Ok(123),
                "{program:?} {extended}"
            );
            assert_eq!(
                parse_token_account(
                    &account_data(&MINT_KEY, 45, 1, extended),
                    program,
                    &MINT_KEY
                ),
                Ok(45)
            );
        }
        // The classic program has no extensions: a longer mint or account is refused.
        assert!(parse_mint(&mint_data(1, 8, true, true), TokenProgram::SplToken).is_err());
        assert!(parse_token_account(
            &account_data(&MINT_KEY, 1, 1, true),
            TokenProgram::SplToken,
            &MINT_KEY
        )
        .is_err());
        // Wrong decimals, uninitialized, truncated.
        assert!(
            parse_mint(&mint_data(1, 6, true, false), TokenProgram::SplToken2022)
                .unwrap_err()
                .contains("decimals")
        );
        assert!(
            parse_mint(&mint_data(1, 8, false, false), TokenProgram::SplToken2022)
                .unwrap_err()
                .contains("initialized")
        );
        assert!(parse_mint(&[0u8; 10], TokenProgram::SplToken2022).is_err());
        // A token account is not a mint (and a mint is not a token account), and an extension
        // type byte that is not the right kind is refused.
        assert!(parse_mint(
            &account_data(&MINT_KEY, 1, 1, false),
            TokenProgram::SplToken2022
        )
        .is_err());
        assert!(parse_token_account(
            &mint_data(1, 8, true, true),
            TokenProgram::SplToken2022,
            &MINT_KEY
        )
        .is_err());
        let mut wrong_type = account_data(&MINT_KEY, 1, 1, true);
        wrong_type[TOKEN_ACCOUNT_LEN] = ACCOUNT_TYPE_MINT;
        assert!(parse_token_account(&wrong_type, TokenProgram::SplToken2022, &MINT_KEY).is_err());
        // Another mint, uninitialized and frozen accounts.
        assert!(parse_token_account(
            &account_data(&[0x77; 32], 1, 1, false),
            TokenProgram::SplToken,
            &MINT_KEY
        )
        .unwrap_err()
        .contains("another mint"));
        for state in [0u8, 2] {
            assert!(
                parse_token_account(
                    &account_data(&MINT_KEY, 1, state, false),
                    TokenProgram::SplToken,
                    &MINT_KEY
                )
                .is_err(),
                "state {state}"
            );
        }
        // The full u64 range is exact.
        assert_eq!(
            parse_mint(&mint_data(u64::MAX, 8, true, false), TokenProgram::SplToken),
            Ok(u128::from(u64::MAX))
        );
    }

    #[test]
    fn targets_are_validated_before_any_request() {
        assert!(Target::new(&mint_text(), &account_text(), TokenProgram::SplToken2022).is_ok());
        assert!(Target::new("not base58 !", &account_text(), TokenProgram::SplToken2022).is_err());
        assert!(Target::new(&mint_text(), "abc", TokenProgram::SplToken2022).is_err());
        let same = Target::new(&mint_text(), &mint_text(), TokenProgram::SplToken2022).unwrap_err();
        assert!(same.contains("same key"), "{same}");
    }

    /// The profile's quorum (2 identical answers) is part of what these tests fix.
    #[test]
    fn the_profiles_quorum_is_two() {
        assert_eq!(usize::from(net().rpc.min_quorum.value), 2);
    }

    #[test]
    fn three_agreeing_providers_give_the_supply_and_custody_with_the_lowest_agreeing_slot() {
        let mut ahead = P::mainnet(
            TokenProgram::SplToken2022,
            1_005,
            10_200_000_000,
            1_250_000_000,
        );
        ahead.context_extra = 2;
        let m = mock(vec![
            P::mainnet(
                TokenProgram::SplToken2022,
                1_000,
                10_200_000_000,
                1_250_000_000,
            ),
            P::mainnet(
                TokenProgram::SplToken2022,
                1_001,
                10_200_000_000,
                1_250_000_000,
            ),
            ahead,
        ]);
        let got = read_supply_and_custody(&m, &target(TokenProgram::SplToken2022), &net()).unwrap();
        assert_eq!((got.supply, got.custody), (10_200_000_000, 1_250_000_000));
        assert_eq!(got.finality, FinalityRule::SolanaCommitment);
        // The agreed slot is the 2nd highest finalized slot (1001); only the providers at or above it
        // answered, with context slots 1001 and 1007, and the reported slot is the lowest of those.
        assert_eq!(got.slot, 1_001);
        let log = m.log.borrow();
        let asked: Vec<(usize, &Value)> = log
            .iter()
            .filter(|(_, r)| r["method"] == "getMultipleAccounts")
            .map(|(i, r)| (*i, r))
            .collect();
        assert_eq!(
            asked.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
            vec![1, 2]
        );
        // Every account request was pinned: finalized, base64, a numbered minContextSlot.
        for (_, r) in asked {
            assert_eq!(r["params"][0], json!([mint_text(), account_text()]));
            assert_eq!(r["params"][1]["commitment"], json!("finalized"));
            assert_eq!(r["params"][1]["encoding"], json!("base64"));
            assert_eq!(r["params"][1]["minContextSlot"], json!(1_001));
        }
        for (_, r) in log.iter() {
            let text = r.to_string();
            for forbidden in ["processed", "confirmed", "jsonParsed"] {
                assert!(!text.contains(forbidden), "sent {forbidden}: {text}");
            }
        }
    }

    #[test]
    fn a_provider_far_ahead_cannot_raise_the_agreed_slot_and_one_below_it_does_not_vote() {
        let m = mock(vec![
            P::mainnet(TokenProgram::SplToken2022, 9_999_999, 5, 1),
            P::mainnet(TokenProgram::SplToken2022, 1_000, 5, 1),
            P::mainnet(TokenProgram::SplToken2022, 990, 5, 1),
        ]);
        let got = read_supply_and_custody(&m, &target(TokenProgram::SplToken2022), &net()).unwrap();
        assert_eq!((got.supply, got.custody, got.slot), (5, 1, 1_000));
        let log = m.log.borrow();
        let asked: Vec<usize> = log
            .iter()
            .filter(|(_, r)| r["method"] == "getMultipleAccounts")
            .map(|(i, _)| *i)
            .collect();
        // The agreed slot is the 2nd highest (1000): the far-ahead provider and the one at 1000 vote,
        // the one at 990 is never asked for accounts.
        assert_eq!(asked, vec![0, 1]);
        for (_, r) in log
            .iter()
            .filter(|(_, r)| r["method"] == "getMultipleAccounts")
        {
            assert_eq!(r["params"][1]["minContextSlot"], json!(1_000));
        }
    }

    #[test]
    fn disagreement_on_the_custody_or_the_supply_is_a_failed_read_not_a_guess() {
        // Every provider answers differently: no value reaches quorum.
        let m = mock(vec![
            P::mainnet(TokenProgram::SplToken2022, 1_000, 100, 10),
            P::mainnet(TokenProgram::SplToken2022, 1_000, 100, 11),
            P::mainnet(TokenProgram::SplToken2022, 1_000, 101, 10),
        ]);
        let err =
            read_supply_and_custody(&m, &target(TokenProgram::SplToken2022), &net()).unwrap_err();
        let ReadError::Failed(text) = err else {
            panic!("{err:?}")
        };
        assert!(text.contains("no quorum"), "{text}");
        assert!(text.starts_with("solana-mainnet: "), "{text}");
        // Providers are named by position only.
        assert!(text.contains("p1") && !text.contains("http"), "{text}");
    }

    #[test]
    fn a_lone_dissenter_is_outvoted() {
        let m = mock(vec![
            P::mainnet(TokenProgram::SplToken2022, 1_000, 100, 10),
            P::mainnet(TokenProgram::SplToken2022, 1_000, 100, 10),
            P::mainnet(TokenProgram::SplToken2022, 1_000, 999, 10),
        ]);
        let got = read_supply_and_custody(&m, &target(TokenProgram::SplToken2022), &net()).unwrap();
        assert_eq!((got.supply, got.custody), (100, 10));
    }

    #[test]
    fn a_provider_on_another_cluster_is_excluded_from_every_vote() {
        let mut devnet = P::mainnet(TokenProgram::SplToken2022, 5_000, 999, 999);
        devnet.genesis = DEVNET_GENESIS.to_string();
        let m = mock(vec![
            devnet.clone(),
            P::mainnet(TokenProgram::SplToken2022, 1_000, 100, 10),
            P::mainnet(TokenProgram::SplToken2022, 1_000, 100, 10),
        ]);
        let got = read_supply_and_custody(&m, &target(TokenProgram::SplToken2022), &net()).unwrap();
        assert_eq!((got.supply, got.custody), (100, 10));
        // The devnet provider's far-ahead slot did not count and it was never asked for accounts.
        assert!(!m
            .log
            .borrow()
            .iter()
            .any(|(i, r)| *i == 0 && r["method"] == "getMultipleAccounts"));
        assert!(!m
            .log
            .borrow()
            .iter()
            .any(|(i, r)| *i == 0 && r["method"] == "getSlot"));
        // Too few providers on the cluster: the quorum cannot be met, and the exclusion is named.
        let m = mock(vec![
            devnet.clone(),
            devnet,
            P::mainnet(TokenProgram::SplToken2022, 1_000, 100, 10),
        ]);
        let err =
            read_supply_and_custody(&m, &target(TokenProgram::SplToken2022), &net()).unwrap_err();
        let ReadError::Failed(text) = err else {
            panic!()
        };
        assert!(
            text.contains("another cluster") && text.contains("p1"),
            "{text}"
        );
        assert!(!m
            .log
            .borrow()
            .iter()
            .any(|(_, r)| r["method"] == "getMultipleAccounts"));
    }

    #[test]
    fn account_replies_that_are_not_the_configured_accounts_are_refused() {
        let base = || P::mainnet(TokenProgram::SplToken2022, 1_000, 100, 10);
        let cases: Vec<(&str, P)> = vec![
            (
                "the return account does not exist",
                P {
                    account: None,
                    ..base()
                },
            ),
            (
                "the mint does not exist",
                P {
                    mint: None,
                    ..base()
                },
            ),
            (
                "the return account is owned by another program",
                P {
                    account_owner: Some(SPL_TOKEN_PROGRAM_B58.into()),
                    ..base()
                },
            ),
            (
                "the mint has 6 decimals",
                P {
                    mint: Some(mint_data(100, 6, true, true)),
                    ..base()
                },
            ),
            (
                "the mint is uninitialized",
                P {
                    mint: Some(mint_data(100, 8, false, true)),
                    ..base()
                },
            ),
            (
                "the account belongs to another mint",
                P {
                    account: Some(account_data(&[0x77; 32], 10, 1, true)),
                    ..base()
                },
            ),
            (
                "the account is frozen",
                P {
                    account: Some(account_data(&MINT_KEY, 10, 2, true)),
                    ..base()
                },
            ),
            (
                "custody exceeds supply",
                P {
                    account: Some(account_data(&MINT_KEY, 101, 1, true)),
                    ..base()
                },
            ),
        ];
        for (name, bad) in cases {
            // All three providers are wrong the same way: nothing can win.
            let m = mock(vec![bad.clone(), bad.clone(), bad]);
            let err = read_supply_and_custody(&m, &target(TokenProgram::SplToken2022), &net())
                .unwrap_err();
            assert!(matches!(err, ReadError::Failed(_)), "{name}: {err:?}");
        }
        // One bad provider among three cannot corrupt a quorum of good ones.
        let m = mock(vec![
            P {
                mint: Some(mint_data(999, 6, true, true)),
                ..base()
            },
            base(),
            base(),
        ]);
        assert_eq!(
            read_supply_and_custody(&m, &target(TokenProgram::SplToken2022), &net())
                .unwrap()
                .supply,
            100
        );
        // The configured program decides the owner: a classic-program target refuses Token-2022 accounts.
        let m = mock(vec![base(), base(), base()]);
        assert!(read_supply_and_custody(&m, &target(TokenProgram::SplToken), &net()).is_err());
    }

    #[test]
    fn a_context_slot_below_the_agreed_slot_is_not_accepted() {
        // A stale node behind a balancer: it claims a finalized slot of 1000 and returns the SAME valid
        // accounts, but its reply context is below the slot it was asked for.
        let stale = || -> Script {
            let inner = P::mainnet(TokenProgram::SplToken2022, 1_000, 100, 10).script();
            Box::new(move |method, params| {
                let mut reply = inner(method, params)?;
                if method == "getMultipleAccounts" {
                    let minimum = params[1]["minContextSlot"].as_u64().unwrap();
                    reply["context"]["slot"] = json!(minimum - 1);
                }
                Ok(reply)
            })
        };
        let good = |extra: u64| {
            let mut p = P::mainnet(TokenProgram::SplToken2022, 1_000, 100, 10);
            p.context_extra = extra;
            p.script()
        };
        // Two good providers outvote it, and its context slot does not lower the reported slot.
        let m = Mock::new(vec![stale(), good(7), good(9)]);
        let got = read_supply_and_custody(&m, &target(TokenProgram::SplToken2022), &net()).unwrap();
        assert_eq!((got.supply, got.custody, got.slot), (100, 10, 1_007));
        // With only one good provider the quorum is not met: the stale reply is not a vote.
        let m = Mock::new(vec![stale(), good(0)]);
        let err =
            read_supply_and_custody(&m, &target(TokenProgram::SplToken2022), &net()).unwrap_err();
        let ReadError::Failed(text) = err else {
            panic!()
        };
        assert!(
            text.contains("below the agreed slot") && text.contains("p1"),
            "{text}"
        );
    }

    #[test]
    fn transport_failures_and_rpc_errors_are_failed_reads() {
        let down: Script = Box::new(|_, _| Err("TRANSPORT".into()));
        let m = Mock::new(vec![
            down,
            Box::new(|_, _| Err("TRANSPORT".into())),
            Box::new(|_, _| Err("TRANSPORT".into())),
        ]);
        let err =
            read_supply_and_custody(&m, &target(TokenProgram::SplToken2022), &net()).unwrap_err();
        assert!(
            matches!(&err, ReadError::Failed(t) if t.contains("only 0 of 3 providers")),
            "{err:?}"
        );
        // An rpc error object is a failure too.
        let m = Mock::new(vec![
            Box::new(|_, _| Err("node is behind".into())),
            Box::new(|_, _| Err("node is behind".into())),
            Box::new(|_, _| Err("node is behind".into())),
        ]);
        assert!(read_supply_and_custody(&m, &target(TokenProgram::SplToken2022), &net()).is_err());
    }

    #[test]
    fn the_rule_and_the_quorum_come_from_the_profile() {
        // An EVM network is not a Solana network: nothing is asked.
        let evm = network("base-mainnet");
        let m = mock(vec![P::mainnet(TokenProgram::SplToken2022, 1, 1, 0)]);
        let err =
            read_supply_and_custody(&m, &target(TokenProgram::SplToken2022), &evm).unwrap_err();
        assert!(
            matches!(&err, ReadError::Failed(t) if t.contains("not the Solana commitment rule")),
            "{err:?}"
        );
        assert!(m.log.borrow().is_empty());
        // The profile's quorum is 2: a single configured provider cannot meet it, and nothing is asked.
        let m = mock(vec![P::mainnet(TokenProgram::SplToken2022, 1, 1, 0)]);
        let err =
            read_supply_and_custody(&m, &target(TokenProgram::SplToken2022), &net()).unwrap_err();
        assert!(
            matches!(&err, ReadError::Failed(t) if t.contains("quorum 2 is not in 1..=1")),
            "{err:?}"
        );
        assert!(m.log.borrow().is_empty());
    }
}
