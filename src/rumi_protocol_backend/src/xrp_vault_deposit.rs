//! SEC-11: credit a new immutable inbound receipt, never an account-balance delta.
//! Outgoing claims already leave vault collateral when created. Their later
//! settlement/retirement cannot create another inbound receipt or erase dedup.
use crate::chains::xrp::{config, deposit_rpc, ted25519, XRP_CHAIN_ID};
use crate::state::State;
use crate::vault::Vault;
use crate::{mutate_state, read_state, ProtocolError};
use candid::{CandidType, Principal};
use serde::{Deserialize, Serialize};

pub const RECEIPTS_PER_VAULT: u64 = 1024;
pub const RECEIPTS_GLOBAL: usize = 100_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct XrpVaultOrigin {
    pub owner: Principal,
    pub opening_ledger_index: u32,
    pub opening_event_index: u64,
    pub key_name: String,
    pub network: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct XrpTopupReceipt {
    pub vault_id: u64,
    pub network: String,
    pub proof: deposit_rpc::NativePaymentReceipt,
}

#[derive(Clone, Debug, CandidType, Deserialize)]
pub struct XrpVaultDepositAddress {
    pub custody_address: String,
    pub opening_ledger_index: u64,
    pub network: String,
    pub receipt_count: u64,
    pub receipt_limit: u64,
}

fn error(msg: &str) -> ProtocolError {
    ProtocolError::GenericError(msg.into())
}

fn origin_from_open(
    vault: &Vault,
    block: u64,
    event_index: u64,
    key_name: &str,
) -> Result<XrpVaultOrigin, ProtocolError> {
    let floor = u32::try_from(block).map_err(|_| error("XRP opening ledger exceeds u32"))?;
    if floor == 0
        || vault.owner == Principal::anonymous()
        || vault.collateral_type != crate::state::xrp_collateral_principal()
    {
        return Err(error("Event is not an original native-XRP vault opening"));
    }
    Ok(XrpVaultOrigin {
        owner: vault.owner,
        opening_ledger_index: floor,
        opening_event_index: event_index,
        network: if config::is_xrp_production_key_name(key_name) {
            "mainnet"
        } else {
            "testnet"
        }
        .into(),
        key_name: key_name.into(),
    })
}

/// Called by the exact OpenVault recording and replay paths. Existing bindings
/// are immutable; a duplicate/replaced vault cannot redirect the original key.
pub fn seed_open_origin(state: &mut State, vault: &Vault, block: u64, event_index: u64) {
    if vault.collateral_type != crate::state::xrp_collateral_principal() {
        return;
    }
    if let Ok(origin) = origin_from_open(
        vault,
        block,
        event_index,
        config::XRP_PRODUCTION_SCHNORR_KEY_NAME,
    ) {
        state
            .xrp_vault_origins
            .entry(vault.vault_id)
            .or_insert(origin);
    }
}

pub fn validate_origin(
    state: &State,
    vault_id: u64,
    origin: &XrpVaultOrigin,
) -> Result<(), ProtocolError> {
    let vault = state
        .vault_id_to_vaults
        .get(&vault_id)
        .ok_or_else(|| error("Vault not found"))?;
    if vault.owner != origin.owner
        || origin.owner == Principal::anonymous()
        || vault.collateral_type != crate::state::xrp_collateral_principal()
        || origin.opening_ledger_index == 0
        || !config::is_xrp_production_key_name(&origin.key_name)
        || origin.network != "mainnet"
    {
        return Err(error(
            "XRP origin does not match current vault owner and native collateral",
        ));
    }
    if let Some(saved) = state.xrp_vault_origins.get(&vault_id) {
        if saved != origin {
            return Err(error("Original XRP custody binding cannot change"));
        }
    }
    Ok(())
}

pub fn apply_origin(
    state: &mut State,
    vault_id: u64,
    origin: XrpVaultOrigin,
) -> Result<(), ProtocolError> {
    validate_origin(state, vault_id, &origin)?;
    state.xrp_vault_origins.entry(vault_id).or_insert(origin);
    Ok(())
}

fn validate_owner(
    state: &State,
    caller: Principal,
    vault_id: u64,
) -> Result<&Vault, ProtocolError> {
    if caller == Principal::anonymous() {
        return Err(ProtocolError::AnonymousCallerNotAllowed);
    }
    let vault = state
        .vault_id_to_vaults
        .get(&vault_id)
        .ok_or_else(|| error("Vault not found"))?;
    if vault.owner != caller {
        return Err(error("Caller is not the vault owner"));
    }
    let cfg = state
        .get_collateral_config(&vault.collateral_type)
        .ok_or_else(|| error("Collateral not configured"))?;
    if vault.collateral_type != crate::state::xrp_collateral_principal()
        || !cfg.is_native_xrp()
        || cfg.ledger_canister_id != crate::state::xrp_collateral_principal()
    {
        return Err(error("Vault does not use native-XRP custody"));
    }
    if !config::is_xrp_production_key_name(&state.xrp_schnorr_key_name) {
        return Err(error(
            "Native-XRP vault deposits require the production key",
        ));
    }
    Ok(vault)
}

fn resolve_origin(
    caller: Principal,
    vault_id: u64,
    event_index: Option<u64>,
) -> Result<XrpVaultOrigin, ProtocolError> {
    read_state(|state| validate_owner(state, caller, vault_id).map(|_| ()))?;
    if let Some(origin) = read_state(|state| state.xrp_vault_origins.get(&vault_id).cloned()) {
        read_state(|state| validate_origin(state, vault_id, &origin))?;
        return Ok(origin);
    }
    let index = event_index.ok_or_else(|| {
        error("Opening event index is required to reconcile this existing XRP vault")
    })?;
    let event = crate::storage::get_event_at(index)
        .ok_or_else(|| error("Opening event index not found"))?;
    let origin = match event {
        crate::event::Event::OpenVault {
            vault, block_index, ..
        } if vault.vault_id == vault_id && vault.owner == caller => {
            let key = read_state(|s| s.xrp_schnorr_key_name.clone());
            origin_from_open(&vault, block_index, index, &key)?
        }
        _ => {
            return Err(error(
                "Supplied event is not this owner's native-XRP opening",
            ))
        }
    };
    // No caller-supplied event contents are trusted: only the retained stable log.
    mutate_state(|state| crate::event::record_xrp_origin(state, vault_id, origin.clone()))?;
    Ok(origin)
}

/// The caller holds VaultLiquidationGuard across all awaits. Repeat the
/// durable/current-state fences as well: a timer/admin/recovery transition can
/// change them while an RPC read is suspended. This is deliberately pure so
/// regression vectors exercise the same checks as every live callback.
fn validate_current_state(
    state: &State,
    caller: Principal,
    vault_id: u64,
    origin: &XrpVaultOrigin,
    now: u64,
) -> Result<(), ProtocolError> {
    let vault = validate_owner(state, caller, vault_id)?;
    validate_origin(state, vault_id, origin)?;
    if state.frozen {
        return Err(ProtocolError::TemporarilyUnavailable(
            "Protocol is frozen".into(),
        ));
    }
    let cfg = state
        .get_collateral_config(&vault.collateral_type)
        .ok_or_else(|| error("Collateral not configured"))?;
    if !cfg.status.allows_add_collateral() {
        return Err(error("XRP collateral is not accepting topups"));
    }
    if state.xrp_schnorr_key_name != origin.key_name || origin.network != "mainnet" {
        return Err(error(
            "XRP custody key/network changed; retry after reconciliation",
        ));
    }
    if vault.bot_processing
        || state.has_pending_nicp_claim(vault_id)
        || state.pending_borrow_mints.contains_key(&vault_id)
        || state
            .push_sweep_journals
            .values()
            .any(|j| j.vault_id == vault_id)
        || state
            .sp_liquidation_journals
            .values()
            .any(|j| j.vault_id == vault_id && j.phase.blocks_vault())
    {
        return Err(error(
            "XRP vault has an unresolved collateral/debt operation",
        ));
    }
    if state
        .sp_xrp_absorb_preflights
        .get(&vault_id)
        .is_some_and(|p| p.expires_at_ns >= now)
    {
        return Err(error(
            "XRP vault has a stability-pool liquidation reservation",
        ));
    }
    Ok(())
}

fn revalidate(
    caller: Principal,
    vault_id: u64,
    origin: &XrpVaultOrigin,
    now: u64,
) -> Result<(), ProtocolError> {
    read_state(|state| validate_current_state(state, caller, vault_id, origin, now))
}

fn capacity(state: &State, vault_id: u64) -> Result<(), ProtocolError> {
    if state.xrp_topup_receipts.len() >= RECEIPTS_GLOBAL
        || state
            .xrp_topup_receipt_counts
            .get(&vault_id)
            .copied()
            .unwrap_or(0)
            >= RECEIPTS_PER_VAULT
    {
        return Err(error(
            "XRP deposit receipt capacity reached; do not send another topup",
        ));
    }
    Ok(())
}

fn existing(
    state: &State,
    vault_id: u64,
    network: &str,
    hash: &str,
) -> Result<Option<u64>, ProtocolError> {
    match state.xrp_topup_receipts.get(&(network.into(), hash.into())) {
        Some(receipt) if receipt.vault_id == vault_id => Ok(Some(receipt.proof.delivered_drops)),
        Some(_) => Err(error(
            "XRP transaction was already credited to another vault",
        )),
        None => Ok(None),
    }
}

pub fn validate_credit(state: &State, receipt: &XrpTopupReceipt) -> Result<(), ProtocolError> {
    if existing(
        state,
        receipt.vault_id,
        &receipt.network,
        &receipt.proof.tx_hash,
    )?
    .is_some()
    {
        return Ok(());
    }
    let origin = state
        .xrp_vault_origins
        .get(&receipt.vault_id)
        .ok_or_else(|| error("Missing XRP origin"))?;
    validate_origin(state, receipt.vault_id, origin)?;
    if receipt.network != origin.network
        || receipt.proof.ledger_index <= origin.opening_ledger_index
        || receipt.proof.delivered_drops == 0
    {
        return Err(error(
            "XRP receipt predates or conflicts with original custody binding",
        ));
    }
    capacity(state, receipt.vault_id)?;
    let vault = &state.vault_id_to_vaults[&receipt.vault_id];
    State::checked_margin_total(
        vault.collateral_amount,
        crate::numeric::ICP::new(receipt.proof.delivered_drops),
    )?;
    Ok(())
}

/// Replay is idempotent even if the same exact receipt event is encountered
/// again. Dedup and collateral mutate in one synchronous transition, no await.
/// Positive addition uses the checked CURRENT collateral total, never a
/// pre-await amount. Debt/ordinary accounting changes need not be equal to an
/// old snapshot: addition commutes with them while the operation fences hold.
pub fn apply_credit(state: &mut State, receipt: XrpTopupReceipt) -> Result<u64, ProtocolError> {
    if let Some(amount) = existing(
        state,
        receipt.vault_id,
        &receipt.network,
        &receipt.proof.tx_hash,
    )? {
        return Ok(amount);
    }
    validate_credit(state, &receipt)?;
    let amount = receipt.proof.delivered_drops;
    state.try_add_margin_to_vault(receipt.vault_id, crate::numeric::ICP::new(amount))?;
    *state
        .xrp_topup_receipt_counts
        .entry(receipt.vault_id)
        .or_default() += 1;
    state.xrp_topup_receipts.insert(
        (receipt.network.clone(), receipt.proof.tx_hash.clone()),
        receipt,
    );
    Ok(amount)
}

fn validate_destination(
    proof: &deposit_rpc::NativePaymentReceipt,
    address: &str,
) -> Result<(), ProtocolError> {
    if proof.destination != address || proof.source == address {
        return Err(error(
            "Payment destination/source does not match original custody",
        ));
    }
    Ok(())
}

pub async fn get_xrp_vault_deposit_address(
    vault_id: u64,
    opening_event_index: Option<u64>,
) -> Result<XrpVaultDepositAddress, ProtocolError> {
    let caller = ic_cdk::caller();
    read_state(|state| validate_owner(state, caller, vault_id).map(|_| ()))?;
    let _principal_guard = crate::guard::GuardPrincipal::new(caller, "xrp_deposit_address")?;
    let _vault_guard = crate::guard::VaultLiquidationGuard::new(vault_id)?;
    let origin = resolve_origin(caller, vault_id, opening_event_index)?;
    revalidate(caller, vault_id, &origin, ic_cdk::api::time())?;
    let (_, address) = ted25519::derive_xrp_address(ted25519::custody_derivation_path(
        XRP_CHAIN_ID,
        origin.owner,
        vault_id,
    ))
    .await
    .map_err(|e| ProtocolError::GenericError(format!("XRP custody derivation failed: {e}")))?;
    revalidate(caller, vault_id, &origin, ic_cdk::api::time())?;
    Ok(XrpVaultDepositAddress {
        custody_address: address,
        opening_ledger_index: u64::from(origin.opening_ledger_index),
        network: origin.network,
        receipt_count: read_state(|s| {
            s.xrp_topup_receipt_counts
                .get(&vault_id)
                .copied()
                .unwrap_or(0)
        }),
        // The per-vault allowance also reflects remaining global capacity.
        receipt_limit: read_state(|s| {
            let count = s
                .xrp_topup_receipt_counts
                .get(&vault_id)
                .copied()
                .unwrap_or(0);
            RECEIPTS_PER_VAULT
                .min(count + RECEIPTS_GLOBAL.saturating_sub(s.xrp_topup_receipts.len()) as u64)
        }),
    })
}

pub async fn confirm_xrp_vault_topup(
    vault_id: u64,
    hash: String,
    opening_event_index: Option<u64>,
) -> Result<u64, ProtocolError> {
    let caller = ic_cdk::caller();
    read_state(|state| validate_owner(state, caller, vault_id).map(|_| ()))?;
    let hash = deposit_rpc::canonical_hash(&hash).map_err(ProtocolError::GenericError)?;
    let _principal_guard = crate::guard::GuardPrincipal::new(caller, "xrp_vault_topup")?;
    let _vault_guard = crate::guard::VaultLiquidationGuard::new(vault_id)?;
    let origin = resolve_origin(caller, vault_id, opening_event_index)?;
    // Duplicates remain readable at capacity and issue no new outcalls/event.
    if let Some(amount) = read_state(|s| existing(s, vault_id, &origin.network, &hash))? {
        return Ok(amount);
    }
    read_state(|s| capacity(s, vault_id))?;
    revalidate(caller, vault_id, &origin, ic_cdk::api::time())?;
    let (_, address) = ted25519::derive_xrp_address(ted25519::custody_derivation_path(
        XRP_CHAIN_ID,
        origin.owner,
        vault_id,
    ))
    .await
    .map_err(|e| ProtocolError::GenericError(format!("XRP custody derivation failed: {e}")))?;
    revalidate(caller, vault_id, &origin, ic_cdk::api::time())?;
    read_state(|s| capacity(s, vault_id))?;
    let proof = deposit_rpc::fetch(&origin.key_name, &hash)
        .await
        .map_err(ProtocolError::GenericError)?;
    revalidate(caller, vault_id, &origin, ic_cdk::api::time())?;
    validate_destination(&proof, &address)?;
    mutate_state(|state| {
        crate::event::record_xrp_topup(
            state,
            XrpTopupReceipt {
                vault_id,
                network: origin.network,
                proof,
            },
        )
    })
}

#[cfg(test)]
mod sec11_tests {
    use super::*;
    fn init() -> crate::InitArg {
        crate::InitArg {
            xrc_principal: Principal::anonymous(),
            icusd_ledger_principal: Principal::anonymous(),
            icp_ledger_principal: Principal::anonymous(),
            fee_e8s: 0,
            developer_principal: Principal::anonymous(),
            treasury_principal: None,
            stability_pool_principal: None,
            ckusdt_ledger_principal: None,
            ckusdc_ledger_principal: None,
        }
    }
    fn vault() -> Vault {
        Vault {
            owner: Principal::from_slice(&[1]),
            vault_id: 7,
            collateral_type: crate::state::xrp_collateral_principal(),
            collateral_amount: 10_000_000,
            borrowed_icusd_amount: 0.into(),
            accrued_interest: 0.into(),
            last_accrual_time: 0,
            bot_processing: false,
        }
    }
    fn state() -> State {
        let mut s = State::from(init());
        s.xrp_schnorr_key_name = config::XRP_PRODUCTION_SCHNORR_KEY_NAME.into();
        let mut cfg = s
            .collateral_configs
            .get(&s.icp_ledger_principal)
            .unwrap()
            .clone();
        cfg.ledger_canister_id = crate::state::xrp_collateral_principal();
        cfg.custody_kind = Some(crate::state::CustodyKind::NativeXrp);
        s.collateral_configs.insert(cfg.ledger_canister_id, cfg);
        let v = vault();
        seed_open_origin(&mut s, &v, 100, 1);
        s.open_vault(v);
        s
    }
    fn receipt(hash: u8, ledger: u32) -> XrpTopupReceipt {
        XrpTopupReceipt {
            vault_id: 7,
            network: "mainnet".into(),
            proof: deposit_rpc::NativePaymentReceipt {
                tx_hash: format!("{hash:02X}").repeat(32),
                ledger_index: ledger,
                ledger_hash: "BB".repeat(32),
                delivered_drops: 10_000,
                source: "rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh".into(),
                destination: "rPT1Sjq2YGrBMTttX4GZHjKu9dyfzbpAYe".into(),
                flags: 0,
            },
        }
    }
    #[test]
    fn sec11_fixed_floor_same_ledger_dedup_and_retired_outflow_never_recredit() {
        let mut s = state();
        assert!(apply_credit(&mut s, receipt(1, 100)).is_err());
        assert!(apply_credit(&mut s, receipt(1, 99)).is_err());
        apply_credit(&mut s, receipt(1, 101)).unwrap();
        apply_credit(&mut s, receipt(2, 101)).unwrap();
        // An outbound payout leaves the vault books; receipt accounting never
        // observes its old account_info balance, claim presence, or retirement.
        s.vault_id_to_vaults.get_mut(&7).unwrap().collateral_amount -= 4_000_000;
        let before = s.vault_id_to_vaults[&7].collateral_amount;
        assert_eq!(apply_credit(&mut s, receipt(1, 101)).unwrap(), 10_000);
        assert_eq!(s.vault_id_to_vaults[&7].collateral_amount, before);
        apply_credit(&mut s, receipt(3, 102)).unwrap();
        assert_eq!(s.vault_id_to_vaults[&7].collateral_amount, before + 10_000);
        assert_eq!(s.xrp_topup_receipt_counts[&7], 3);
        assert_eq!(s.xrp_vault_origins[&7].opening_ledger_index, 100);
        let original = s.xrp_vault_origins[&7].clone();
        seed_open_origin(&mut s, &vault(), 900, 55);
        assert_eq!(
            s.xrp_vault_origins[&7], original,
            "never reset original floor"
        );
    }
    #[test]
    fn sec11_auth_origin_capacity_overflow_and_close_preserve_tombstones() {
        let mut s = state();
        let owner = vault().owner;
        assert!(validate_owner(&s, Principal::anonymous(), 7).is_err());
        assert!(validate_owner(&s, Principal::from_slice(&[2]), 7).is_err());
        assert!(validate_owner(&s, owner, 7).is_ok());
        let proof = receipt(1, 101).proof;
        assert!(validate_destination(&proof, &proof.destination).is_ok());
        assert!(validate_destination(&proof, &proof.source).is_err());
        assert!(validate_destination(&proof, "rWrongDestination").is_err());
        let mut wrong = s.xrp_vault_origins[&7].clone();
        wrong.owner = Principal::from_slice(&[2]);
        assert!(apply_origin(&mut s, 7, wrong).is_err());
        let r = receipt(1, 101);
        apply_credit(&mut s, r.clone()).unwrap();
        s.xrp_topup_receipt_counts.insert(7, RECEIPTS_PER_VAULT);
        assert!(validate_credit(&s, &receipt(2, 101)).is_err());
        assert_eq!(apply_credit(&mut s, r.clone()).unwrap(), 10_000);
        s.xrp_topup_receipt_counts.insert(7, 1);
        s.vault_id_to_vaults.get_mut(&7).unwrap().collateral_amount = u64::MAX;
        assert!(apply_credit(&mut s, receipt(2, 101)).is_err());
        assert_eq!(s.xrp_topup_receipts.len(), 1);
        s.close_vault(7);
        assert_eq!(s.xrp_topup_receipts.len(), 1, "closing cannot erase dedup");
        assert!(s.xrp_vault_origins.contains_key(&7));
    }
    #[test]
    fn sec11_current_state_fences_reject_intervening_mutations() {
        let mut s = state();
        let owner = vault().owner;
        let origin = s.xrp_vault_origins[&7].clone();
        let xrp = crate::state::xrp_collateral_principal();
        assert!(validate_current_state(&s, owner, 7, &origin, 100).is_ok());

        s.frozen = true;
        assert!(validate_current_state(&s, owner, 7, &origin, 100).is_err());
        s.frozen = false;
        for status in [
            crate::state::CollateralStatus::Frozen,
            crate::state::CollateralStatus::Deprecated,
            crate::state::CollateralStatus::Sunset,
        ] {
            s.collateral_configs.get_mut(&xrp).unwrap().status = status;
            assert!(validate_current_state(&s, owner, 7, &origin, 100).is_err());
        }
        s.collateral_configs.get_mut(&xrp).unwrap().status = crate::state::CollateralStatus::Paused;
        assert!(
            validate_current_state(&s, owner, 7, &origin, 100).is_ok(),
            "paused collateral still accepts backing"
        );
        s.collateral_configs.get_mut(&xrp).unwrap().status = crate::state::CollateralStatus::Active;
        s.collateral_configs
            .get_mut(&xrp)
            .unwrap()
            .ledger_canister_id = Principal::from_slice(&[22]);
        assert!(validate_current_state(&s, owner, 7, &origin, 100).is_err());
        s.collateral_configs
            .get_mut(&xrp)
            .unwrap()
            .ledger_canister_id = xrp;

        s.vault_id_to_vaults.get_mut(&7).unwrap().owner = Principal::from_slice(&[2]);
        assert!(validate_current_state(&s, owner, 7, &origin, 100).is_err());
        s.vault_id_to_vaults.get_mut(&7).unwrap().owner = owner;
        s.xrp_schnorr_key_name = config::XRP_TEST_SCHNORR_KEY_NAME.into();
        assert!(validate_current_state(&s, owner, 7, &origin, 100).is_err());
        s.xrp_schnorr_key_name = config::XRP_PRODUCTION_SCHNORR_KEY_NAME.into();
        s.vault_id_to_vaults.get_mut(&7).unwrap().bot_processing = true;
        assert!(validate_current_state(&s, owner, 7, &origin, 100).is_err());
        s.vault_id_to_vaults.get_mut(&7).unwrap().bot_processing = false;

        s.sp_xrp_absorb_preflights.insert(
            7,
            crate::state::StoredXrpSpAbsorbPreflight {
                caller: Principal::from_slice(&[3]),
                vault_id: 7,
                icusd_burn_e8s: 100,
                total_to_seize_drops: 100,
                collateral_received_drops: 100,
                collateral_price_e8s: 100_000_000,
                expires_at_ns: 100,
            },
        );
        assert!(validate_current_state(&s, owner, 7, &origin, 100).is_err());
        assert!(validate_current_state(&s, owner, 7, &origin, 101).is_ok());
        s.sp_xrp_absorb_preflights.clear();
        s.pending_borrow_mints.insert(
            7,
            crate::state::BorrowMintJournal {
                vault_id: 7,
                owner,
                collateral_type: xrp,
                borrowed_amount_e8s: 100,
                fee_amount_e8s: 0,
                tuple: crate::state::BorrowMintTuple {
                    ledger: Principal::from_slice(&[4]),
                    destination: owner,
                    amount_e8s: 100,
                    memo: [0; 16],
                    created_at_time_ns: 1,
                    op_nonce: 1,
                },
                phase: crate::state::BorrowMintPhase::SubmittedOrUnknown,
                history_floor: None,
                absence_scan: None,
                dispatch_attempt_count: Some(1),
                typed_too_old: false,
            },
        );
        assert!(validate_current_state(&s, owner, 7, &origin, 100).is_err());
        s.pending_borrow_mints.clear();
        assert!(validate_current_state(&s, owner, 7, &origin, 100).is_ok());
        s.vault_id_to_vaults.remove(&7);
        assert!(
            validate_current_state(&s, owner, 7, &origin, 100).is_err(),
            "closed vault cannot receive a stale callback credit"
        );
    }

    #[test]
    fn sec11_old_snapshot_defaults_and_new_receipts_roundtrip() {
        let mut s = state();
        apply_credit(&mut s, receipt(1, 101)).unwrap();
        let mut bytes = vec![];
        ciborium::ser::into_writer(&s, &mut bytes).unwrap();
        let restored: State = ciborium::de::from_reader(bytes.as_slice()).unwrap();
        assert_eq!(restored.xrp_topup_receipts, s.xrp_topup_receipts);
        let mut value: ciborium::Value = ciborium::de::from_reader(bytes.as_slice()).unwrap();
        if let ciborium::Value::Map(ref mut fields) = value {
            fields.retain(|(k,_)| !matches!(k,ciborium::Value::Text(k) if ["xrp_vault_origins","xrp_topup_receipts","xrp_topup_receipt_counts"].contains(&k.as_str())));
        }
        let mut old = vec![];
        ciborium::ser::into_writer(&value, &mut old).unwrap();
        let restored: State = ciborium::de::from_reader(old.as_slice()).unwrap();
        assert!(restored.xrp_vault_origins.is_empty());
        assert!(restored.xrp_topup_receipts.is_empty());
        assert_eq!(
            restored.vault_id_to_vaults[&7].collateral_amount,
            s.vault_id_to_vaults[&7].collateral_amount
        );
    }
    #[test]
    fn sec11_legacy_origin_requires_real_stable_opening_and_checked_floor() {
        let mut s = state();
        s.xrp_vault_origins.clear();
        let index = crate::storage::count_events();
        crate::storage::record_event_at(
            &crate::event::Event::OpenVault {
                vault: vault(),
                block_index: 100,
                timestamp: Some(1),
            },
            1,
        );
        let opening = crate::storage::get_event_at(index).unwrap();
        if let crate::event::Event::OpenVault {
            vault: v,
            block_index,
            ..
        } = opening
        {
            let origin = origin_from_open(
                &v,
                block_index,
                index,
                config::XRP_PRODUCTION_SCHNORR_KEY_NAME,
            )
            .unwrap();
            apply_origin(&mut s, 7, origin.clone()).unwrap();
            assert_eq!(s.xrp_vault_origins[&7], origin);
            assert!(
                origin_from_open(&v, 0, index, config::XRP_PRODUCTION_SCHNORR_KEY_NAME).is_err()
            );
            assert!(origin_from_open(
                &v,
                u64::from(u32::MAX) + 1,
                index,
                config::XRP_PRODUCTION_SCHNORR_KEY_NAME
            )
            .is_err());
            let mut wrong = v.clone();
            wrong.owner = Principal::anonymous();
            assert!(
                origin_from_open(&wrong, 100, index, config::XRP_PRODUCTION_SCHNORR_KEY_NAME)
                    .is_err()
            );
        } else {
            panic!("opening event lookup changed");
        }
        assert!(crate::storage::get_event_at(u64::MAX).is_none());
    }
    #[test]
    fn sec11_ordered_private_journal_replays_receipt_and_credit_exactly_once() {
        let public = vec![
            crate::event::Event::Init(init()),
            crate::event::Event::OpenVault {
                vault: vault(),
                block_index: 100,
                timestamp: Some(1),
            },
        ];
        let public_only = crate::event::replay(public.clone().into_iter()).unwrap();
        let mut live = state();
        apply_credit(&mut live, receipt(1, 101)).unwrap();
        assert!(
            live.check_semantically_eq(&public_only).is_err(),
            "public-only self-check loses the private collateral credit"
        );
        let entry = crate::storage::PendingPayoutJournalEntry {
            after_event_count: 2,
            event: crate::event::PendingPayoutEvent::XrpVaultTopupCredited {
                receipt: receipt(1, 101),
            },
        };
        let restored = crate::event::replay_with_pending_payout_events(
            public.into_iter(),
            vec![entry.clone(), entry].into_iter(),
        )
        .unwrap();
        assert_eq!(
            restored.vault_id_to_vaults[&7].collateral_amount,
            10_010_000
        );
        assert_eq!(restored.xrp_topup_receipts.len(), 1);
        assert_eq!(restored.xrp_topup_receipt_counts[&7], 1);
        assert_eq!(restored.xrp_vault_origins[&7].network, "mainnet");
        live.check_semantically_eq(&restored)
            .expect("ordered journal self-check must match live collateral");
        assert_eq!(live.xrp_topup_receipts, restored.xrp_topup_receipts);
        assert_eq!(
            live.xrp_topup_receipt_counts,
            restored.xrp_topup_receipt_counts
        );
        assert_eq!(live.xrp_vault_origins, restored.xrp_vault_origins);
    }
}
