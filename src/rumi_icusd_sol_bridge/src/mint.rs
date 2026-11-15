//! The escrow mint and the return account: account layouts, the bind checks, and the bind driver
//! (`chain-solana.md` section 1 "Bind checks", section 8 claims `none -> deployed_unbound -> bound`).
//!
//! Every check reads the account at `finalized` through the [`SolRpc`] mirror and decides on the
//! raw account bytes (base64-decoded by the mirror), never on a provider-parsed JSON view.
//!
//! **Mint bind checks** (a mint that fails any of them is never bound, and nothing is built
//! against it): the account is owned by the profile token program; it is an initialized mint; the
//! mint authority is our `mint-authority` key; the freeze authority is none (in both token
//! programs it can then never be added, note section 1); `decimals == 8`; the supply is 0 on first
//! bind; and the extension list is a subset of the allow-list `{MetadataPointer pointing at the
//! mint itself, TokenMetadata}`, each with an authority of none or ours (below). Anything else (the note names `TransferFeeConfig`,
//! `ScaledUiAmount`, `InterestBearingConfig`, the confidential extensions, `PermanentDelegate`,
//! `TransferHook`, `DefaultAccountState`, `NonTransferable`, `MintCloseAuthority`, `Pausable` and
//! `PermissionedBurn`) fails the bind. The check is an allow-list, so extension types this crate has
//! never heard of fail too.
//!
//! **Return account checks**: the canonical ATA of `(return-owner, token program, mint)` exists,
//! is owned by the profile token program, is an initialized token account of this mint whose
//! owner field is the `return-owner` key, with no delegate, no close authority and not native;
//! Token-2022 extensions on it are limited to `ImmutableOwner`.
//!
//! **Metadata authorities.** When a `MetadataPointer` or a `TokenMetadata` extension is present, its
//! authority field (the first 32 bytes of the value, an optional non-zero key: all zero is none)
//! must be none or the canister's own `mint-authority` role key. The design's authority matrix
//! (section 7) allows the Token-2022 metadata update authority to be none or the canister and
//! nothing else; a foreign pointer authority could re-point the metadata, and a foreign update
//! authority could rewrite the name and symbol users see. Another role key of ours (fee payer,
//! return owner, the one-time mint-account key) is just as foreign here. The pointer is judged
//! before the metadata. A bound mint is re-checked by the same function, so a foreign authority
//! cannot appear later without failing the reverify.
//!
//! **Source pins (a reading of source and layout documents, not proof about the installed program
//! or any wallet).** The extension type ids come from the official `solana-program/token-2022` at
//! b5b7511e5d4f19a6a118b858d83a7fe3b0017b1e, `interface/src/extension/mod.rs` lines 1074-1121
//! (`ImmutableOwner = 7`, `MetadataPointer = 18`, `TokenMetadata = 19`;
//! https://github.com/solana-program/token-2022/blob/b5b7511e5d4f19a6a118b858d83a7fe3b0017b1e/interface/src/extension/mod.rs#L1074).
//! The account-type framing byte (offset 165, after zero padding) and the role of the metadata
//! extensions are as described at https://www.solana-program.com/docs/token-2022. The check is an
//! allow-list of three ids, so a wrong id would have two effects, not one: a legitimate mint could
//! fail to bind, and an extension whose real id differs could be read as one of the allowed three.
//! What stands in the way of the second is the exact shape demanded of each allowed extension
//! (a pointer of exactly 64 bytes naming the mint itself, metadata for this mint, authorities none
//! or ours), and activation re-checks the ids and the live account against the installed program
//! (blocker `solana-token-program-wallet-check`). In-mint metadata is checked here when present;
//! its name, symbol and URI are NOT created by the bootstrap (a disclosed deferred omission: this
//! crate has no instruction encoder for them).

use crate::records::MintState;
use crate::sol_rpc::{AccountSnapshot, RpcFailure, SolRpc};
use crate::state;
use crate::tx::builders::TxProfile;
use crate::tx::{derive_ata, pubkey_to_base58, Pubkey, TokenProgram, TOKEN_DECIMALS};

/// Size of a mint account without extensions, and of a token account without extensions.
pub const MINT_LEN: usize = 82;
pub const TOKEN_ACCOUNT_LEN: usize = 165;
/// Token-2022 account type byte (at offset 165 of an extended account).
pub const ACCOUNT_TYPE_MINT: u8 = 1;
pub const ACCOUNT_TYPE_TOKEN_ACCOUNT: u8 = 2;

/// Token-2022 extension type ids, pinned in the module docs to the official source (a reading, not
/// installed-program proof).
pub const EXT_IMMUTABLE_OWNER: u16 = 7;
pub const EXT_METADATA_POINTER: u16 = 18;
pub const EXT_TOKEN_METADATA: u16 = 19;

/// The parsed content of a mint account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MintView {
    pub mint_authority: Option<Pubkey>,
    pub supply: u64,
    pub decimals: u8,
    pub is_initialized: bool,
    pub freeze_authority: Option<Pubkey>,
    /// `(type id, value)` of every extension, in account order.
    pub extensions: Vec<(u16, Vec<u8>)>,
}

/// The parsed content of a token account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenAccountView {
    pub mint: Pubkey,
    pub owner: Pubkey,
    pub amount: u64,
    pub delegate: Option<Pubkey>,
    /// 0 uninitialized, 1 initialized, 2 frozen.
    pub state: u8,
    pub is_native: bool,
    pub close_authority: Option<Pubkey>,
    pub extensions: Vec<(u16, Vec<u8>)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BindProblem {
    MintWrongOwner(Pubkey),
    MintBadData(String),
    MintNotInitialized,
    MintAuthority(Option<Pubkey>),
    FreezeAuthorityPresent(Pubkey),
    Decimals(u8),
    SupplyNotZero(u64),
    ExtensionNotAllowed(u16),
    MetadataPointerElsewhere,
    /// The metadata pointer's authority is neither none nor the `mint-authority` role key.
    MetadataPointerAuthority(Pubkey),
    TokenMetadataWithoutPointer,
    TokenMetadataWrongMint,
    /// The token metadata's update authority is neither none nor the `mint-authority` role key.
    MetadataUpdateAuthority(Pubkey),
    ReturnWrongOwner(Pubkey),
    ReturnBadData(String),
    ReturnNotInitialized(u8),
    ReturnMint(Pubkey),
    ReturnOwner(Pubkey),
    ReturnDelegate(Pubkey),
    ReturnCloseAuthority(Pubkey),
    ReturnNative,
    ReturnExtensionNotAllowed(u16),
}

impl std::fmt::Display for BindProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

fn coption_key(data: &[u8], at: usize) -> Result<Option<Pubkey>, String> {
    let tag = data.get(at..at + 4).ok_or("truncated COption tag")?;
    let tag = u32::from_le_bytes([tag[0], tag[1], tag[2], tag[3]]);
    let key = data.get(at + 4..at + 36).ok_or("truncated COption key")?;
    match tag {
        0 => Ok(None),
        1 => Pubkey::try_from(key).map(Some).map_err(|_| "key length".to_string()),
        other => Err(format!("COption tag {other}")),
    }
}

fn read_u64(data: &[u8], at: usize) -> Result<u64, String> {
    let b = data.get(at..at + 8).ok_or("truncated u64")?;
    Ok(u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
}

/// Parse the Token-2022 TLV area (`[type u16 LE][length u16 LE][value]*`) that follows the account
/// type byte. A zero type id with only zero bytes after it ends the area (reallocation padding);
/// anything malformed, truncated or duplicated is an error.
pub fn parse_tlv(area: &[u8]) -> Result<Vec<(u16, Vec<u8>)>, String> {
    let mut out: Vec<(u16, Vec<u8>)> = vec![];
    let mut i = 0usize;
    while i < area.len() {
        if area[i..].iter().all(|b| *b == 0) {
            break;
        }
        let header = area.get(i..i + 4).ok_or("truncated extension header")?;
        let kind = u16::from_le_bytes([header[0], header[1]]);
        let len = u16::from_le_bytes([header[2], header[3]]) as usize;
        if kind == 0 {
            return Err("extension type 0 followed by data".into());
        }
        let value = area.get(i + 4..i + 4 + len).ok_or("truncated extension value")?;
        if out.iter().any(|(k, _)| *k == kind) {
            return Err(format!("duplicate extension {kind}"));
        }
        out.push((kind, value.to_vec()));
        i += 4 + len;
    }
    Ok(out)
}

/// Extensions of an account of the given base length: none for the base size, else the type byte
/// and the TLV area. The padding between the base layout and the type byte must be zero.
fn parse_extended(data: &[u8], base_len: usize, expected_type: u8, program: TokenProgram) -> Result<Vec<(u16, Vec<u8>)>, String> {
    if data.len() == base_len {
        return Ok(vec![]);
    }
    if program == TokenProgram::Classic {
        return Err(format!("a classic token program account must be exactly {base_len} bytes, got {}", data.len()));
    }
    if data.len() <= TOKEN_ACCOUNT_LEN {
        return Err(format!("an extended account must be longer than {TOKEN_ACCOUNT_LEN} bytes, got {}", data.len()));
    }
    if data[base_len..TOKEN_ACCOUNT_LEN].iter().any(|b| *b != 0) {
        return Err("non-zero padding before the account type".into());
    }
    if data[TOKEN_ACCOUNT_LEN] != expected_type {
        return Err(format!("account type byte {} (expected {expected_type})", data[TOKEN_ACCOUNT_LEN]));
    }
    parse_tlv(&data[TOKEN_ACCOUNT_LEN + 1..])
}

/// Parse a mint account (82 bytes, plus the Token-2022 extension area).
pub fn parse_mint(data: &[u8], program: TokenProgram) -> Result<MintView, String> {
    if data.len() < MINT_LEN {
        return Err(format!("mint data is {} bytes, shorter than {MINT_LEN}", data.len()));
    }
    let mint_authority = coption_key(data, 0)?;
    let supply = read_u64(data, 36)?;
    let decimals = data[44];
    let is_initialized = match data[45] {
        0 => false,
        1 => true,
        other => return Err(format!("is_initialized byte {other}")),
    };
    let freeze_authority = coption_key(data, 46)?;
    let extensions = parse_extended(data, MINT_LEN, ACCOUNT_TYPE_MINT, program)?;
    Ok(MintView { mint_authority, supply, decimals, is_initialized, freeze_authority, extensions })
}

/// Parse a token account (165 bytes, plus the Token-2022 extension area).
pub fn parse_token_account(data: &[u8], program: TokenProgram) -> Result<TokenAccountView, String> {
    if data.len() < TOKEN_ACCOUNT_LEN {
        return Err(format!("token account data is {} bytes, shorter than {TOKEN_ACCOUNT_LEN}", data.len()));
    }
    let key = |at: usize| Pubkey::try_from(&data[at..at + 32]).map_err(|_| "key length".to_string());
    let mint = key(0)?;
    let owner = key(32)?;
    let amount = read_u64(data, 64)?;
    let delegate = coption_key(data, 72)?;
    let state = data[108];
    let is_native = match data.get(109..113).map(|t| u32::from_le_bytes([t[0], t[1], t[2], t[3]])) {
        Some(0) => false,
        Some(1) => true,
        other => return Err(format!("is_native tag {other:?}")),
    };
    let close_authority = coption_key(data, 129)?;
    // A token account's extension area starts at 165 too; the base length equals the padding start.
    let extensions = if data.len() == TOKEN_ACCOUNT_LEN {
        vec![]
    } else {
        parse_extended(data, TOKEN_ACCOUNT_LEN, ACCOUNT_TYPE_TOKEN_ACCOUNT, program)?
    };
    Ok(TokenAccountView { mint, owner, amount, delegate, state, is_native, close_authority, extensions })
}

/// What the bind compares against.
#[derive(Clone, Debug)]
pub struct BindExpectations {
    pub token_program: TokenProgram,
    pub mint: Pubkey,
    pub mint_authority: Pubkey,
    pub return_owner: Pubkey,
}

impl BindExpectations {
    pub fn from_profile(p: &TxProfile) -> Self {
        BindExpectations { token_program: p.token_program, mint: p.mint, mint_authority: p.mint_authority, return_owner: p.return_owner }
    }
}

/// The mint bind checks. `first_bind` adds the pristine-supply condition.
pub fn check_mint(snapshot: &AccountSnapshot, exp: &BindExpectations, first_bind: bool) -> Result<MintView, BindProblem> {
    if snapshot.owner != exp.token_program.id() {
        return Err(BindProblem::MintWrongOwner(snapshot.owner));
    }
    let view = parse_mint(&snapshot.data, exp.token_program).map_err(BindProblem::MintBadData)?;
    if !view.is_initialized {
        return Err(BindProblem::MintNotInitialized);
    }
    if view.mint_authority != Some(exp.mint_authority) {
        return Err(BindProblem::MintAuthority(view.mint_authority));
    }
    if let Some(f) = view.freeze_authority {
        return Err(BindProblem::FreezeAuthorityPresent(f));
    }
    if view.decimals != TOKEN_DECIMALS {
        return Err(BindProblem::Decimals(view.decimals));
    }
    if first_bind && view.supply != 0 {
        return Err(BindProblem::SupplyNotZero(view.supply));
    }
    // Allow-list: MetadataPointer (pointing at the mint itself) and TokenMetadata (for this mint).
    let mut pointer = false;
    let mut metadata = None;
    for (kind, value) in &view.extensions {
        match *kind {
            EXT_METADATA_POINTER => {
                // [authority OptionalNonZeroPubkey 32][metadata address OptionalNonZeroPubkey 32]
                if value.len() != 64 || value[32..64] != exp.mint {
                    return Err(BindProblem::MetadataPointerElsewhere);
                }
                check_metadata_authority(&value[..32], exp, BindProblem::MetadataPointerAuthority)?;
                pointer = true;
            }
            EXT_TOKEN_METADATA => metadata = Some(value),
            other => return Err(BindProblem::ExtensionNotAllowed(other)),
        }
    }
    if let Some(value) = metadata {
        if !pointer {
            return Err(BindProblem::TokenMetadataWithoutPointer);
        }
        // [update authority OptionalNonZeroPubkey 32][mint 32][name, symbol, uri, additional ...]
        if value.len() < 64 || value[32..64] != exp.mint {
            return Err(BindProblem::TokenMetadataWrongMint);
        }
        check_metadata_authority(&value[..32], exp, BindProblem::MetadataUpdateAuthority)?;
    }
    Ok(view)
}

/// An optional non-zero key (all zero is none) must be none or the `mint-authority` role key.
fn check_metadata_authority(field: &[u8], exp: &BindExpectations, problem: fn(Pubkey) -> BindProblem) -> Result<(), BindProblem> {
    let Ok(key) = Pubkey::try_from(field) else { return Err(BindProblem::MintBadData("metadata authority field is not 32 bytes".into())) };
    if key == [0u8; 32] || key == exp.mint_authority {
        Ok(())
    } else {
        Err(problem(key))
    }
}

/// The return account checks. `snapshot` is the account at the canonical ATA address.
pub fn check_return_account(snapshot: &AccountSnapshot, exp: &BindExpectations) -> Result<TokenAccountView, BindProblem> {
    if snapshot.owner != exp.token_program.id() {
        return Err(BindProblem::ReturnWrongOwner(snapshot.owner));
    }
    let view = parse_token_account(&snapshot.data, exp.token_program).map_err(BindProblem::ReturnBadData)?;
    if view.state != 1 {
        return Err(BindProblem::ReturnNotInitialized(view.state));
    }
    if view.mint != exp.mint {
        return Err(BindProblem::ReturnMint(view.mint));
    }
    if view.owner != exp.return_owner {
        return Err(BindProblem::ReturnOwner(view.owner));
    }
    if let Some(d) = view.delegate {
        return Err(BindProblem::ReturnDelegate(d));
    }
    if let Some(c) = view.close_authority {
        return Err(BindProblem::ReturnCloseAuthority(c));
    }
    if view.is_native {
        return Err(BindProblem::ReturnNative);
    }
    for (kind, _) in &view.extensions {
        if *kind != EXT_IMMUTABLE_OWNER {
            return Err(BindProblem::ReturnExtensionNotAllowed(*kind));
        }
    }
    Ok(view)
}

/// What one bind attempt found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BindOutcome {
    /// Nothing to do: the mint is already bound.
    AlreadyBound,
    /// The mint account does not exist at `finalized`.
    NotCreated,
    /// The mint is pristine and checked (`DeployedUnbound`), but the return account is missing.
    ReturnAccountMissing,
    /// Every check passed: the claim is `Bound`.
    Bound,
    /// A check failed; the claim does not advance. The note is kept for the status query.
    Problem(BindProblem),
    /// A read was `Inconsistent` or failed: try again.
    Unavailable(RpcFailure),
}

fn note_problem(p: &BindProblem) {
    state::mutate_core(|c| c.bind_note = Some(p.to_string()));
}

/// Run the bind checks against the chain and advance the claim. Idempotent: a bound mint is not
/// re-checked here (see [`reverify`]).
pub async fn try_bind<R: SolRpc>(rpc: &R) -> Result<BindOutcome, String> {
    let (profile, current) = state::read_core(|c| (c.creation_profile(), c.mint_state));
    let profile = profile?;
    if current == MintState::Bound {
        return Ok(BindOutcome::AlreadyBound);
    }
    let exp = BindExpectations::from_profile(&profile);

    let mint_snapshot = match rpc.get_account(&profile.mint).await {
        Ok(Some(s)) => s,
        Ok(None) => return Ok(BindOutcome::NotCreated),
        Err(e) => return Ok(BindOutcome::Unavailable(e)),
    };
    if let Err(problem) = check_mint(&mint_snapshot, &exp, true) {
        note_problem(&problem);
        return Ok(BindOutcome::Problem(problem));
    }
    // The mint exists and is pristine: DeployedUnbound (monotone; never back to None).
    state::mutate_core(|c| {
        if c.mint_state == MintState::None {
            c.mint_state = MintState::DeployedUnbound;
            c.mint = Some(profile.mint);
        }
    });

    let return_ata = profile.return_ata().map_err(|e| format!("{e}"))?;
    let ata_snapshot = match rpc.get_account(&return_ata).await {
        Ok(Some(s)) => s,
        Ok(None) => return Ok(BindOutcome::ReturnAccountMissing),
        Err(e) => return Ok(BindOutcome::Unavailable(e)),
    };
    if let Err(problem) = check_return_account(&ata_snapshot, &exp) {
        note_problem(&problem);
        return Ok(BindOutcome::Problem(problem));
    }
    let bound = state::mutate_core(|c| {
        if c.mint_state == MintState::Bound {
            return false;
        }
        c.mint_state = MintState::Bound;
        c.mint = Some(profile.mint);
        c.bind_note = None;
        true
    });
    if bound {
        state::append_audit(None, state::AuditCtx::note(format!("escrow mint {} bound", pubkey_to_base58(&profile.mint))));
    }
    Ok(BindOutcome::Bound)
}

/// Re-read a bound mint and return account and re-run the checks (without the supply condition).
/// A failure halts nothing by itself; the caller reports it. Used by audits and after an upgrade.
pub async fn reverify<R: SolRpc>(rpc: &R) -> Result<Result<(), BindProblem>, String> {
    let profile = state::read_core(|c| c.tx_profile())?;
    let exp = BindExpectations::from_profile(&profile);
    let mint = match rpc.get_account(&profile.mint).await {
        Ok(Some(s)) => s,
        Ok(None) => return Ok(Err(BindProblem::MintBadData("the bound mint no longer exists".into()))),
        Err(e) => return Err(format!("{e:?}")),
    };
    if let Err(p) = check_mint(&mint, &exp, false) {
        return Ok(Err(p));
    }
    let ata = derive_ata(&profile.return_owner, &profile.token_program.id(), &profile.mint).map_err(|e| format!("{e}"))?;
    match rpc.get_account(&ata).await {
        Ok(Some(s)) => Ok(check_return_account(&s, &exp).map(|_| ())),
        Ok(None) => Ok(Err(BindProblem::ReturnBadData("the return account no longer exists".into()))),
        Err(e) => Err(format!("{e:?}")),
    }
}

#[cfg(test)]
pub(crate) mod fixtures {
    //! Account images for the tests and for the mock chain.
    use super::*;

    pub fn coption(key: Option<&Pubkey>) -> Vec<u8> {
        let mut out = vec![];
        match key {
            Some(k) => {
                out.extend_from_slice(&1u32.to_le_bytes());
                out.extend_from_slice(k);
            }
            None => {
                out.extend_from_slice(&0u32.to_le_bytes());
                out.extend_from_slice(&[0u8; 32]);
            }
        }
        out
    }

    pub fn mint_data(authority: Option<&Pubkey>, supply: u64, decimals: u8, initialized: bool, freeze: Option<&Pubkey>) -> Vec<u8> {
        let mut d = coption(authority);
        d.extend_from_slice(&supply.to_le_bytes());
        d.push(decimals);
        d.push(initialized as u8);
        d.extend(coption(freeze));
        assert_eq!(d.len(), MINT_LEN);
        d
    }

    pub fn tlv(entries: &[(u16, Vec<u8>)]) -> Vec<u8> {
        let mut d = vec![];
        for (k, v) in entries {
            d.extend_from_slice(&k.to_le_bytes());
            d.extend_from_slice(&(v.len() as u16).to_le_bytes());
            d.extend_from_slice(v);
        }
        d
    }

    /// A Token-2022 extended mint: base, zero padding to 165, type byte, TLV.
    pub fn extended_mint(base: Vec<u8>, entries: &[(u16, Vec<u8>)]) -> Vec<u8> {
        let mut d = base;
        d.resize(TOKEN_ACCOUNT_LEN, 0);
        d.push(ACCOUNT_TYPE_MINT);
        d.extend(tlv(entries));
        d
    }

    pub fn token_account_data(mint: &Pubkey, owner: &Pubkey, amount: u64, delegate: Option<&Pubkey>, state: u8, close: Option<&Pubkey>) -> Vec<u8> {
        let mut d = vec![];
        d.extend_from_slice(mint);
        d.extend_from_slice(owner);
        d.extend_from_slice(&amount.to_le_bytes());
        d.extend(coption(delegate));
        d.push(state);
        d.extend_from_slice(&0u32.to_le_bytes()); // is_native None
        d.extend_from_slice(&[0u8; 8]);
        d.extend_from_slice(&0u64.to_le_bytes()); // delegated amount
        d.extend(coption(close));
        assert_eq!(d.len(), TOKEN_ACCOUNT_LEN);
        d
    }

    pub fn extended_token_account(base: Vec<u8>, entries: &[(u16, Vec<u8>)]) -> Vec<u8> {
        let mut d = base;
        d.push(ACCOUNT_TYPE_TOKEN_ACCOUNT);
        d.extend(tlv(entries));
        d
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;
    use crate::testkit::{block_on, Fault, MockChain, FEE_PAYER, MINT, MINT_AUTHORITY, RETURN_OWNER};
    use crate::tx::{ATA_PROGRAM_ID, SYSTEM_PROGRAM_ID, TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID};

    fn exp(tp: TokenProgram) -> BindExpectations {
        BindExpectations { token_program: tp, mint: MINT, mint_authority: MINT_AUTHORITY, return_owner: RETURN_OWNER }
    }

    fn good_mint(tp: TokenProgram) -> AccountSnapshot {
        AccountSnapshot { owner: tp.id(), data: mint_data(Some(&MINT_AUTHORITY), 0, 8, true, None) }
    }

    fn pointer_value(authority: Option<&Pubkey>, address: &Pubkey) -> Vec<u8> {
        let mut v = authority.map(|a| a.to_vec()).unwrap_or_else(|| vec![0u8; 32]);
        v.extend_from_slice(address);
        v
    }

    fn metadata_value(mint: &Pubkey) -> Vec<u8> {
        let mut v = vec![0u8; 32]; // update authority none
        v.extend_from_slice(mint);
        v.extend_from_slice(&[4, 0, 0, 0, b'i', b'c', b'U', b'S']); // a name, enough for the check
        v
    }

    #[test]
    fn a_pristine_mint_binds_for_both_token_programs() {
        for tp in [TokenProgram::Classic, TokenProgram::Token2022] {
            let v = check_mint(&good_mint(tp), &exp(tp), true).unwrap();
            assert_eq!((v.supply, v.decimals, v.freeze_authority), (0, 8, None));
            assert_eq!(v.mint_authority, Some(MINT_AUTHORITY));
        }
    }

    #[test]
    fn each_mint_bind_check_fails_on_its_own() {
        let tp = TokenProgram::Token2022;
        let run = |s: AccountSnapshot| check_mint(&s, &exp(tp), true).unwrap_err();
        let data = |a: Option<&Pubkey>, supply, dec, init, freeze: Option<&Pubkey>| AccountSnapshot { owner: tp.id(), data: mint_data(a, supply, dec, init, freeze) };
        // owner = the profile token program (the other token program is not it)
        assert_eq!(run(AccountSnapshot { owner: TOKEN_PROGRAM_ID, data: mint_data(Some(&MINT_AUTHORITY), 0, 8, true, None) }), BindProblem::MintWrongOwner(TOKEN_PROGRAM_ID));
        assert_eq!(run(AccountSnapshot { owner: SYSTEM_PROGRAM_ID, data: mint_data(Some(&MINT_AUTHORITY), 0, 8, true, None) }), BindProblem::MintWrongOwner(SYSTEM_PROGRAM_ID));
        // initialized
        assert_eq!(run(data(Some(&MINT_AUTHORITY), 0, 8, false, None)), BindProblem::MintNotInitialized);
        // mint authority = ours (not another key, not none)
        assert_eq!(run(data(Some(&[0x99; 32]), 0, 8, true, None)), BindProblem::MintAuthority(Some([0x99; 32])));
        assert_eq!(run(data(None, 0, 8, true, None)), BindProblem::MintAuthority(None));
        // freeze authority none
        assert_eq!(run(data(Some(&MINT_AUTHORITY), 0, 8, true, Some(&[0x98; 32]))), BindProblem::FreezeAuthorityPresent([0x98; 32]));
        // decimals 8
        assert_eq!(run(data(Some(&MINT_AUTHORITY), 0, 6, true, None)), BindProblem::Decimals(6));
        assert_eq!(run(data(Some(&MINT_AUTHORITY), 0, 9, true, None)), BindProblem::Decimals(9));
        // supply 0 on first bind only
        assert_eq!(run(data(Some(&MINT_AUTHORITY), 1, 8, true, None)), BindProblem::SupplyNotZero(1));
        assert!(check_mint(&data(Some(&MINT_AUTHORITY), 5, 8, true, None), &exp(tp), false).is_ok(), "a bound mint legitimately has supply");
        // malformed data
        assert!(matches!(run(AccountSnapshot { owner: tp.id(), data: vec![0; 10] }), BindProblem::MintBadData(_)));
        let mut bad_flag = mint_data(Some(&MINT_AUTHORITY), 0, 8, true, None);
        bad_flag[45] = 2;
        assert!(matches!(run(AccountSnapshot { owner: tp.id(), data: bad_flag }), BindProblem::MintBadData(_)));
        let mut bad_tag = mint_data(Some(&MINT_AUTHORITY), 0, 8, true, None);
        bad_tag[0] = 7;
        assert!(matches!(run(AccountSnapshot { owner: tp.id(), data: bad_tag }), BindProblem::MintBadData(_)));
        // a classic mint is exactly 82 bytes
        let classic_extended = extended_mint(mint_data(Some(&MINT_AUTHORITY), 0, 8, true, None), &[]);
        let r = check_mint(&AccountSnapshot { owner: TOKEN_PROGRAM_ID, data: classic_extended }, &exp(TokenProgram::Classic), true);
        assert!(matches!(r, Err(BindProblem::MintBadData(_))), "{r:?}");
    }

    #[test]
    fn the_extension_allow_list_admits_only_the_metadata_pointer_and_token_metadata() {
        let tp = TokenProgram::Token2022;
        let base = || mint_data(Some(&MINT_AUTHORITY), 0, 8, true, None);
        let with = |entries: &[(u16, Vec<u8>)]| AccountSnapshot { owner: tp.id(), data: extended_mint(base(), entries) };
        // No extension, a pointer at the mint, and pointer + metadata all pass.
        assert!(check_mint(&with(&[]), &exp(tp), true).is_ok());
        assert!(check_mint(&with(&[(EXT_METADATA_POINTER, pointer_value(None, &MINT))]), &exp(tp), true).is_ok());
        let both = with(&[(EXT_METADATA_POINTER, pointer_value(Some(&MINT_AUTHORITY), &MINT)), (EXT_TOKEN_METADATA, metadata_value(&MINT))]);
        assert!(check_mint(&both, &exp(tp), true).is_ok());
        // A pointer at another account, or a metadata entry without a pointer or for another mint, fails.
        assert_eq!(check_mint(&with(&[(EXT_METADATA_POINTER, pointer_value(None, &[7; 32]))]), &exp(tp), true).unwrap_err(), BindProblem::MetadataPointerElsewhere);
        assert_eq!(check_mint(&with(&[(EXT_METADATA_POINTER, vec![0; 10])]), &exp(tp), true).unwrap_err(), BindProblem::MetadataPointerElsewhere);
        assert_eq!(check_mint(&with(&[(EXT_TOKEN_METADATA, metadata_value(&MINT))]), &exp(tp), true).unwrap_err(), BindProblem::TokenMetadataWithoutPointer);
        let wrong_mint = with(&[(EXT_METADATA_POINTER, pointer_value(None, &MINT)), (EXT_TOKEN_METADATA, metadata_value(&[8; 32]))]);
        assert_eq!(check_mint(&wrong_mint, &exp(tp), true).unwrap_err(), BindProblem::TokenMetadataWrongMint);
        // Every other extension type fails, whichever it is: the list is an allow-list, so
        // unknown ids fail too. (Ids: 1 TransferFeeConfig, 3 MintCloseAuthority, 4 ConfidentialTransferMint,
        // 6 DefaultAccountState, 9 NonTransferable, 10 InterestBearingConfig, 12 PermanentDelegate,
        // 14 TransferHook, 25 ScaledUiAmount, 26 Pausable, and ids this crate never heard of.)
        for kind in [1u16, 3, 4, 6, 9, 10, 12, 14, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 99, 1000, 0xfffe] {
            let r = check_mint(&with(&[(kind, vec![0; 8])]), &exp(tp), true);
            assert_eq!(r.unwrap_err(), BindProblem::ExtensionNotAllowed(kind), "extension {kind}");
        }
        // A forbidden extension next to an allowed one still fails.
        let mixed = with(&[(EXT_METADATA_POINTER, pointer_value(None, &MINT)), (12, vec![0; 32])]);
        assert_eq!(check_mint(&mixed, &exp(tp), true).unwrap_err(), BindProblem::ExtensionNotAllowed(12));
    }

    #[test]
    fn metadata_authorities_must_be_none_or_the_canisters_mint_authority_key() {
        let tp = TokenProgram::Token2022;
        let base = || mint_data(Some(&MINT_AUTHORITY), 0, 8, true, None);
        let with = |entries: &[(u16, Vec<u8>)]| AccountSnapshot { owner: tp.id(), data: extended_mint(base(), entries) };
        let metadata_with = |update_authority: Option<&Pubkey>| {
            let mut v = metadata_value(&MINT);
            if let Some(a) = update_authority {
                v[..32].copy_from_slice(a);
            }
            v
        };
        // The pointer's authority: none and the mint-authority key pass; anything else is refused,
        // another role key of ours included (only the mint-authority role may keep a metadata power).
        for ok in [None, Some(&MINT_AUTHORITY)] {
            let pointer = with(&[(EXT_METADATA_POINTER, pointer_value(ok, &MINT))]);
            assert!(check_mint(&pointer, &exp(tp), true).is_ok(), "{ok:?}");
        }
        for foreign in [[1u8; 32], FEE_PAYER, RETURN_OWNER, MINT] {
            let pointer = with(&[(EXT_METADATA_POINTER, pointer_value(Some(&foreign), &MINT))]);
            assert_eq!(check_mint(&pointer, &exp(tp), true).unwrap_err(), BindProblem::MetadataPointerAuthority(foreign));
            // A bound mint is re-checked by the same function: a foreign authority never slips in later.
            assert_eq!(check_mint(&pointer, &exp(tp), false).unwrap_err(), BindProblem::MetadataPointerAuthority(foreign));
        }
        // The metadata's update authority: the same rule.
        let pointer = (EXT_METADATA_POINTER, pointer_value(None, &MINT));
        for ok in [None, Some(&MINT_AUTHORITY)] {
            let both = with(&[pointer.clone(), (EXT_TOKEN_METADATA, metadata_with(ok))]);
            assert!(check_mint(&both, &exp(tp), true).is_ok(), "{ok:?}");
        }
        for foreign in [[2u8; 32], FEE_PAYER, RETURN_OWNER] {
            let both = with(&[pointer.clone(), (EXT_TOKEN_METADATA, metadata_with(Some(&foreign)))]);
            assert_eq!(check_mint(&both, &exp(tp), true).unwrap_err(), BindProblem::MetadataUpdateAuthority(foreign));
            assert_eq!(check_mint(&both, &exp(tp), false).unwrap_err(), BindProblem::MetadataUpdateAuthority(foreign));
        }
        // Both foreign: the pointer is judged first (a stable, testable order).
        let both_foreign = with(&[(EXT_METADATA_POINTER, pointer_value(Some(&[1; 32]), &MINT)), (EXT_TOKEN_METADATA, metadata_with(Some(&[2; 32])))]);
        assert_eq!(check_mint(&both_foreign, &exp(tp), true).unwrap_err(), BindProblem::MetadataPointerAuthority([1; 32]));
        // The ordinary checks and the empty-extension path are unchanged.
        assert!(check_mint(&with(&[]), &exp(tp), true).is_ok());
        assert!(check_mint(&good_mint(tp), &exp(tp), true).is_ok());
        assert_eq!(check_mint(&AccountSnapshot { owner: tp.id(), data: mint_data(Some(&[9; 32]), 0, 8, true, None) }, &exp(tp), true).unwrap_err(), BindProblem::MintAuthority(Some([9; 32])));
    }

    #[test]
    fn malformed_extension_areas_fail_closed() {
        let tp = TokenProgram::Token2022;
        let base = mint_data(Some(&MINT_AUTHORITY), 0, 8, true, None);
        let try_data = |d: Vec<u8>| check_mint(&AccountSnapshot { owner: tp.id(), data: d }, &exp(tp), true);
        // Truncated header, truncated value, duplicate, non-zero padding, wrong account type byte,
        // type 0 followed by data, and a length between 83 and 165.
        let mut d = extended_mint(base.clone(), &[]);
        d.extend_from_slice(&[18, 0, 64]);
        assert!(matches!(try_data(d), Err(BindProblem::MintBadData(_))));
        let mut d = extended_mint(base.clone(), &[]);
        d.extend_from_slice(&[18, 0, 64, 0, 1, 2]);
        assert!(matches!(try_data(d), Err(BindProblem::MintBadData(_))));
        let dup = extended_mint(base.clone(), &[(EXT_METADATA_POINTER, pointer_value(None, &MINT)), (EXT_METADATA_POINTER, pointer_value(None, &MINT))]);
        assert!(matches!(try_data(dup), Err(BindProblem::MintBadData(_))));
        let mut padded = extended_mint(base.clone(), &[]);
        padded[100] = 1;
        assert!(matches!(try_data(padded), Err(BindProblem::MintBadData(_))));
        let mut wrong_type = extended_mint(base.clone(), &[]);
        wrong_type[TOKEN_ACCOUNT_LEN] = ACCOUNT_TYPE_TOKEN_ACCOUNT;
        assert!(matches!(try_data(wrong_type), Err(BindProblem::MintBadData(_))));
        let mut type_zero = extended_mint(base.clone(), &[]);
        type_zero.extend_from_slice(&[0, 0, 1, 0, 5]);
        assert!(matches!(try_data(type_zero), Err(BindProblem::MintBadData(_))));
        let mut short = base.clone();
        short.resize(120, 0);
        assert!(matches!(try_data(short), Err(BindProblem::MintBadData(_))));
        // Trailing zero padding (a reallocation) after the last extension is fine.
        let mut padded_ok = extended_mint(base, &[(EXT_METADATA_POINTER, pointer_value(None, &MINT))]);
        padded_ok.extend_from_slice(&[0; 7]);
        assert!(try_data(padded_ok).is_ok());
    }

    fn good_return_account(tp: TokenProgram) -> AccountSnapshot {
        let base = token_account_data(&MINT, &RETURN_OWNER, 0, None, 1, None);
        let data = if tp == TokenProgram::Token2022 { extended_token_account(base, &[(EXT_IMMUTABLE_OWNER, vec![])]) } else { base };
        AccountSnapshot { owner: tp.id(), data }
    }

    #[test]
    fn the_return_account_checks() {
        for tp in [TokenProgram::Classic, TokenProgram::Token2022] {
            let v = check_return_account(&good_return_account(tp), &exp(tp)).unwrap();
            assert_eq!((v.mint, v.owner, v.amount), (MINT, RETURN_OWNER, 0));
        }
        let tp = TokenProgram::Token2022;
        let run = |data: Vec<u8>, owner: Pubkey| check_return_account(&AccountSnapshot { owner, data }, &exp(tp)).unwrap_err();
        let acct = |mint: &Pubkey, owner: &Pubkey, delegate: Option<&Pubkey>, state: u8, close: Option<&Pubkey>| token_account_data(mint, owner, 0, delegate, state, close);
        assert_eq!(run(acct(&MINT, &RETURN_OWNER, None, 1, None), TOKEN_PROGRAM_ID), BindProblem::ReturnWrongOwner(TOKEN_PROGRAM_ID));
        assert_eq!(run(acct(&MINT, &RETURN_OWNER, None, 0, None), tp.id()), BindProblem::ReturnNotInitialized(0));
        assert_eq!(run(acct(&MINT, &RETURN_OWNER, None, 2, None), tp.id()), BindProblem::ReturnNotInitialized(2), "frozen");
        assert_eq!(run(acct(&[5; 32], &RETURN_OWNER, None, 1, None), tp.id()), BindProblem::ReturnMint([5; 32]));
        assert_eq!(run(acct(&MINT, &FEE_PAYER, None, 1, None), tp.id()), BindProblem::ReturnOwner(FEE_PAYER), "the owner field is the return-owner key, not another role");
        assert_eq!(run(acct(&MINT, &RETURN_OWNER, Some(&[6; 32]), 1, None), tp.id()), BindProblem::ReturnDelegate([6; 32]));
        assert_eq!(run(acct(&MINT, &RETURN_OWNER, None, 1, Some(&[7; 32])), tp.id()), BindProblem::ReturnCloseAuthority([7; 32]));
        let mut native = acct(&MINT, &RETURN_OWNER, None, 1, None);
        native[109] = 1;
        assert_eq!(run(native, tp.id()), BindProblem::ReturnNative);
        // Token-2022 extensions: ImmutableOwner only (MemoTransfer 8, CpiGuard 11, ... fail).
        for kind in [8u16, 11, 13, 15, 99] {
            let d = extended_token_account(acct(&MINT, &RETURN_OWNER, None, 1, None), &[(kind, vec![0])]);
            assert_eq!(run(d, tp.id()), BindProblem::ReturnExtensionNotAllowed(kind));
        }
        assert!(matches!(run(vec![0; 100], tp.id()), BindProblem::ReturnBadData(_)));
        // A classic token account is exactly 165 bytes.
        let long = extended_token_account(acct(&MINT, &RETURN_OWNER, None, 1, None), &[]);
        let r = check_return_account(&AccountSnapshot { owner: TOKEN_PROGRAM_ID, data: long }, &exp(TokenProgram::Classic));
        assert!(matches!(r, Err(BindProblem::ReturnBadData(_))));
    }

    // ---- the bind driver against the mock chain ----

    fn setup(tp: TokenProgram) -> (MockChain, TxProfile) {
        state::reset_for_tests();
        state::mutate_core(|c| {
            c.initialized = true;
            c.config.token_program = tp;
            c.keys = Some(state::RoleKeys { fee_payer: FEE_PAYER, mint_authority: MINT_AUTHORITY, return_owner: RETURN_OWNER, mint_account: Some(MINT) });
        });
        let profile = state::read_core(|c| c.creation_profile()).unwrap();
        (MockChain::new(), profile)
    }

    fn put_good_mint(chain: &MockChain, tp: TokenProgram) {
        chain.set_account(MINT, tp.id(), mint_data(Some(&MINT_AUTHORITY), 0, 8, true, None));
    }

    fn put_good_ata(chain: &MockChain, profile: &TxProfile) {
        let ata = profile.return_ata().unwrap();
        let snap = good_return_account(profile.token_program);
        chain.set_account(ata, snap.owner, snap.data);
    }

    #[test]
    fn the_claim_moves_none_to_deployed_unbound_to_bound_and_only_forward() {
        for tp in [TokenProgram::Classic, TokenProgram::Token2022] {
            let (chain, profile) = setup(tp);
            assert_eq!(state::read_core(|c| c.mint_state), MintState::None);
            assert!(state::read_core(|c| c.tx_profile()).is_err(), "no operation messages before the bind");
            assert_eq!(block_on(try_bind(&chain)).unwrap(), BindOutcome::NotCreated);
            assert_eq!(state::read_core(|c| c.mint_state), MintState::None);
            put_good_mint(&chain, tp);
            assert_eq!(block_on(try_bind(&chain)).unwrap(), BindOutcome::ReturnAccountMissing);
            assert_eq!(state::read_core(|c| (c.mint_state, c.mint)), (MintState::DeployedUnbound, Some(MINT)));
            assert!(state::read_core(|c| c.tx_profile()).is_err(), "deployed but unbound still builds nothing");
            put_good_ata(&chain, &profile);
            assert_eq!(block_on(try_bind(&chain)).unwrap(), BindOutcome::Bound);
            assert_eq!(state::read_core(|c| (c.mint_state, c.bind_note.clone())), (MintState::Bound, None));
            assert!(state::read_core(|c| c.tx_profile()).is_ok());
            // Idempotent: no reads once bound.
            let reads = chain.call_count("get_account");
            assert_eq!(block_on(try_bind(&chain)).unwrap(), BindOutcome::AlreadyBound);
            assert_eq!(chain.call_count("get_account"), reads);
        }
    }

    #[test]
    fn a_failed_check_never_advances_the_claim_and_leaves_a_note() {
        let tp = TokenProgram::Token2022;
        let (chain, profile) = setup(tp);
        // A mint someone else controls.
        chain.set_account(MINT, tp.id(), mint_data(Some(&[0x99; 32]), 0, 8, true, None));
        let out = block_on(try_bind(&chain)).unwrap();
        assert_eq!(out, BindOutcome::Problem(BindProblem::MintAuthority(Some([0x99; 32]))));
        assert_eq!(state::read_core(|c| c.mint_state), MintState::None);
        assert!(state::read_core(|c| c.bind_note.clone()).unwrap().contains("MintAuthority"));
        // Fixing the mint (here: a fresh correct mint) lets the bind proceed; a bad return account blocks the last step.
        put_good_mint(&chain, tp);
        let ata = profile.return_ata().unwrap();
        chain.set_account(ata, tp.id(), token_account_data(&MINT, &FEE_PAYER, 0, None, 1, None));
        let out = block_on(try_bind(&chain)).unwrap();
        assert_eq!(out, BindOutcome::Problem(BindProblem::ReturnOwner(FEE_PAYER)));
        assert_eq!(state::read_core(|c| c.mint_state), MintState::DeployedUnbound, "the pristine mint is recorded, the bind is not complete");
        put_good_ata(&chain, &profile);
        assert_eq!(block_on(try_bind(&chain)).unwrap(), BindOutcome::Bound);
    }

    #[test]
    fn a_mint_with_supply_cannot_be_bound_but_a_bound_mint_may_have_supply() {
        let tp = TokenProgram::Token2022;
        let (chain, profile) = setup(tp);
        chain.set_account(MINT, tp.id(), mint_data(Some(&MINT_AUTHORITY), 5, 8, true, None));
        assert_eq!(block_on(try_bind(&chain)).unwrap(), BindOutcome::Problem(BindProblem::SupplyNotZero(5)));
        // Bound mints are re-verified without the supply condition.
        put_good_mint(&chain, tp);
        put_good_ata(&chain, &profile);
        assert_eq!(block_on(try_bind(&chain)).unwrap(), BindOutcome::Bound);
        chain.set_account(MINT, tp.id(), mint_data(Some(&MINT_AUTHORITY), 1_000, 8, true, None));
        assert_eq!(block_on(reverify(&chain)).unwrap(), Ok(()));
        chain.set_account(MINT, tp.id(), mint_data(Some(&MINT_AUTHORITY), 1_000, 8, true, Some(&[1; 32])));
        assert_eq!(block_on(reverify(&chain)).unwrap(), Err(BindProblem::FreezeAuthorityPresent([1; 32])));
    }

    #[test]
    fn unavailable_reads_wait_and_change_nothing() {
        let tp = TokenProgram::Token2022;
        let (chain, _profile) = setup(tp);
        put_good_mint(&chain, tp);
        chain.fail_next("get_account", Fault::Inconsistent);
        assert_eq!(block_on(try_bind(&chain)).unwrap(), BindOutcome::Unavailable(RpcFailure::Inconsistent));
        chain.fail_next("get_account", Fault::Error("x".into()));
        assert!(matches!(block_on(try_bind(&chain)).unwrap(), BindOutcome::Unavailable(RpcFailure::Error(_))));
        assert_eq!(state::read_core(|c| c.mint_state), MintState::None);
        // A consistent null for the mint is NotCreated, never a bind.
        chain.fail_next("get_account", Fault::Null);
        assert_eq!(block_on(try_bind(&chain)).unwrap(), BindOutcome::NotCreated);
    }

    #[test]
    fn the_bind_needs_the_mint_account_key() {
        state::reset_for_tests();
        let chain = MockChain::new();
        assert!(block_on(try_bind(&chain)).is_err());
        let _ = (ATA_PROGRAM_ID, TOKEN_2022_PROGRAM_ID);
    }
}
