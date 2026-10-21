//! Bounded, sealed accounting repair for already-minted interest. No ledger calls.
use crate::{
    receipt_store,
    state::{self, mutate_state, read_state, StabilityPoolState},
    types::PoolEventType,
};
use candid::{CandidType, Deserialize, Encode, Principal};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

// Exact canonical Candid allocation payload, retained in protected evidence.
// This source seal is not production execution authorization.
pub const SEALED_MANIFEST_HASH: [u8; 32] = [
    19, 250, 86, 84, 132, 109, 129, 109, 141, 235, 248, 222, 84, 76, 53, 143, 19, 210, 88, 116,
    163, 110, 49, 174, 53, 34, 95, 172, 63, 186, 97, 131,
];
const MAX_PACKETS: usize = 512;
const MAX_RECIPIENTS: usize = 100;

#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct RecoveryPacket {
    pub source_mint_block: u64,
    pub token_ledger: Principal,
    pub amount: u64,
    pub collateral_type: Principal,
    pub allocations: BTreeMap<Principal, u64>,
}
#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct RecoveryManifest {
    pub baseline_digest: Vec<u8>,
    pub packets: Vec<RecoveryPacket>,
}
#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct RecoveryProgress {
    pub manifest_hash: Vec<u8>,
    pub manifest: RecoveryManifest,
    pub after_digests: Vec<Vec<u8>>,
    pub next_packet: u16,
    pub credited_raw: u64,
    pub live_interest_only: bool,
}
#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct RecoveryStatus {
    pub manifest_hash: Vec<u8>,
    pub packet_count: u16,
    pub next_packet: u16,
    pub credited_raw: u64,
    pub live_interest_only: bool,
}
#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct RecoveryPacketReceipt {
    pub manifest_hash: Vec<u8>,
    pub source_mint_block: u64,
    pub payload: state::InterestMintReceiptPayload,
    pub allocation_digest: Vec<u8>,
    pub credited: bool,
}
fn hash(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}
pub fn manifest_hash(m: &RecoveryManifest) -> Vec<u8> {
    hash(&Encode!(m).expect("encode recovery manifest"))
}
pub fn accounting_digest(s: &StabilityPoolState) -> Vec<u8> {
    hash(
        &Encode!(
            &s.deposits,
            &s.total_stablecoin_balances,
            &s.stablecoin_registry,
            &s.collateral_registry,
            &s.chain_collateral_sentinels,
            &s.protocol_canister_id,
            &s.configuration,
            &s.total_interest_received_e8s
        )
        .expect("encode recovery accounting baseline"),
    )
}
pub fn locked() -> bool {
    read_state(|s| {
        s.interest_recovery_bootstrap_hold.unwrap_or(false)
            || s.interest_recovery
                .as_ref()
                .is_some_and(|p| !p.live_interest_only)
    })
}
pub fn live() -> bool {
    read_state(|s| {
        s.interest_recovery
            .as_ref()
            .is_some_and(|p| p.live_interest_only)
    })
}
pub fn ensure_unlocked() {
    if locked() {
        ic_cdk::trap("interest accounting recovery holds all other pool updates");
    }
}
fn admin() -> Result<(), String> {
    let caller = ic_cdk::api::caller();
    if caller == Principal::anonymous()
        || !read_state(|s| s.configuration.authorized_admins.contains(&caller))
    {
        return Err("Unauthorized".into());
    }
    Ok(())
}
fn receiver_or_admin() -> Result<(), String> {
    let caller = ic_cdk::api::caller();
    if caller == Principal::anonymous()
        || !read_state(|s| {
            s.protocol_canister_id == caller || s.configuration.authorized_admins.contains(&caller)
        })
    {
        return Err("Unauthorized".into());
    }
    Ok(())
}
fn status(p: &RecoveryProgress) -> RecoveryStatus {
    RecoveryStatus {
        manifest_hash: p.manifest_hash.clone(),
        packet_count: p.manifest.packets.len() as u16,
        next_packet: p.next_packet,
        credited_raw: p.credited_raw,
        live_interest_only: p.live_interest_only,
    }
}
fn payload(p: &RecoveryPacket) -> state::InterestMintReceiptPayload {
    state::InterestMintReceiptPayload {
        token_ledger: p.token_ledger,
        amount: p.amount,
        collateral_type: Some(p.collateral_type),
    }
}
pub fn apply_packet(s: &mut StabilityPoolState, p: &RecoveryPacket) -> Result<(), String> {
    let token = s
        .stablecoin_registry
        .get(&p.token_ledger)
        .ok_or("unknown interest token")?;
    if token.symbol != "icUSD"
        || token.decimals != 8
        || !s.collateral_registry.contains_key(&p.collateral_type)
    {
        return Err("unsupported interest tuple".into());
    }
    if p.amount == 0 || p.allocations.is_empty() || p.allocations.len() > MAX_RECIPIENTS {
        return Err("invalid allocation bound".into());
    }
    let total: u128 = p.allocations.values().map(|v| *v as u128).sum();
    if total != p.amount as u128 {
        return Err("allocation does not conserve amount".into());
    }
    // Validate every checked addition before writing any field.
    for (owner, amount) in &p.allocations {
        let position = s.deposits.get(owner).ok_or("unknown recovery recipient")?;
        position
            .stablecoin_balances
            .get(&p.token_ledger)
            .copied()
            .unwrap_or(0)
            .checked_add(*amount)
            .ok_or("position overflow")?;
        position
            .total_interest_earned_e8s
            .unwrap_or(0)
            .checked_add(*amount)
            .ok_or("position lifetime interest overflow")?;
    }
    let aggregate = s
        .total_stablecoin_balances
        .get(&p.token_ledger)
        .copied()
        .unwrap_or(0)
        .checked_add(p.amount)
        .ok_or("pool balance overflow")?;
    let interest = s
        .total_interest_received_e8s
        .unwrap_or(0)
        .checked_add(p.amount)
        .ok_or("pool interest overflow")?;
    for (owner, amount) in &p.allocations {
        if *amount == 0 {
            continue;
        }
        let position = s.deposits.get_mut(owner).expect("validated recipient");
        *position
            .stablecoin_balances
            .entry(p.token_ledger)
            .or_insert(0) += *amount;
        *position.total_interest_earned_e8s.get_or_insert(0) += *amount;
    }
    s.total_stablecoin_balances
        .insert(p.token_ledger, aggregate);
    s.total_interest_received_e8s = Some(interest);
    Ok(())
}
pub fn stage(m: RecoveryManifest) -> Result<RecoveryStatus, String> {
    admin()?;
    let digest = manifest_hash(&m);
    if SEALED_MANIFEST_HASH == [0; 32] || digest != SEALED_MANIFEST_HASH {
        return Err("manifest is not the reviewed sealed allocation".into());
    }
    if let Some(p) = read_state(|s| s.interest_recovery.clone()) {
        return if p.manifest_hash == digest {
            Ok(status(&p))
        } else {
            Err("another recovery is already retained".into())
        };
    }
    crate::ensure_no_pool_balance_async_in_flight()
        .map_err(|_| "pool async operation in flight")?;
    if crate::pool_balance_mutation_blocked()
        || crate::pool_guard::chain_absorb_auto_tick_in_flight()
    {
        return Err("pool mutation in flight".into());
    }
    let mut simulation = read_state(Clone::clone);
    if !simulation.configuration.emergency_pause
        || m.baseline_digest != accounting_digest(&simulation)
    {
        return Err("pause or accounting baseline mismatch".into());
    }
    if m.packets.is_empty()
        || m.packets.len() > MAX_PACKETS
        || m.packets
            .windows(2)
            .any(|w| w[0].source_mint_block >= w[1].source_mint_block)
    {
        return Err("invalid ordered packet bound".into());
    }
    let mut after_digests = Vec::with_capacity(m.packets.len());
    for p in &m.packets {
        if receipt_store::interest_mint_receipt_status(
            &simulation,
            p.source_mint_block,
            &payload(p),
        ) != state::InterestMintReceiptStatus::New
        {
            return Err("existing or ambiguous source receipt".into());
        }
        apply_packet(&mut simulation, p)?;
        after_digests.push(accounting_digest(&simulation));
    }
    let p = RecoveryProgress {
        manifest_hash: digest,
        manifest: m,
        after_digests,
        next_packet: 0,
        credited_raw: 0,
        live_interest_only: false,
    };
    let result = status(&p);
    mutate_state(|s| s.interest_recovery = Some(p));
    Ok(result)
}
pub fn continue_batch(
    hash: Vec<u8>,
    expected_next: u16,
    limit: u8,
) -> Result<RecoveryStatus, String> {
    admin()?;
    let p = read_state(|s| s.interest_recovery.clone()).ok_or("no staged recovery")?;
    if p.manifest_hash != hash || limit == 0 || limit > 10 || expected_next > p.next_packet {
        return Err("wrong recovery, cursor or limit".into());
    }
    if expected_next < p.next_packet || p.live_interest_only {
        return Ok(status(&p));
    }
    for _ in 0..limit {
        let p = read_state(|s| s.interest_recovery.clone()).ok_or("no recovery")?;
        let index = p.next_packet as usize;
        if index == p.manifest.packets.len() {
            break;
        }
        let row = p.manifest.packets[index].clone();
        let before = if index == 0 {
            &p.manifest.baseline_digest
        } else {
            &p.after_digests[index - 1]
        };
        if !read_state(|s| s.configuration.emergency_pause && accounting_digest(s) == *before) {
            return Err("recovery predecessor mismatch".into());
        }
        if read_state(|s| {
            receipt_store::interest_mint_receipt_status(s, row.source_mint_block, &payload(&row))
        }) != state::InterestMintReceiptStatus::New
        {
            return Err("source receipt changed unexpectedly".into());
        }
        mutate_state(|s| {
            let receipt = state::InterestMintReceipt {
                payload: Some(payload(&row)),
                disposition: state::InterestMintReceiptDisposition::Distributed,
            };
            assert_eq!(
                receipt_store::record_interest_mint_receipt(s, row.source_mint_block, receipt),
                state::InterestMintReceiptStatus::New,
                "source receipt changed"
            );
            apply_packet(s, &row).expect("prevalidated recovery accounting");
            assert_eq!(
                accounting_digest(s),
                p.after_digests[index],
                "recovery result mismatch"
            );
            s.record_interest_mint_legacy_cache(row.source_mint_block);
            s.push_event(
                ic_cdk::api::caller(),
                PoolEventType::InterestReceived {
                    token_ledger: row.token_ledger,
                    amount: row.amount,
                },
            );
            let progress = s.interest_recovery.as_mut().expect("recovery retained");
            progress.next_packet += 1;
            progress.credited_raw = progress
                .credited_raw
                .checked_add(row.amount)
                .expect("recovery total overflow");
        });
    }
    Ok(read_state(|s| {
        status(s.interest_recovery.as_ref().expect("recovery retained"))
    }))
}
pub fn finish(hash: Vec<u8>) -> Result<RecoveryStatus, String> {
    admin()?;
    let p = read_state(|s| s.interest_recovery.clone()).ok_or("no recovery")?;
    if p.manifest_hash != hash || p.next_packet as usize != p.manifest.packets.len() {
        return Err("recovery not complete".into());
    }
    if p.live_interest_only {
        return Ok(status(&p));
    }
    if !read_state(|s| {
        s.configuration.emergency_pause && p.after_digests.last() == Some(&accounting_digest(s))
    }) {
        return Err("final accounting mismatch".into());
    }
    for row in &p.manifest.packets {
        if read_state(|s| {
            receipt_store::interest_mint_receipt_status(s, row.source_mint_block, &payload(row))
        }) != state::InterestMintReceiptStatus::Duplicate
        {
            return Err("missing exact permanent receipt".into());
        }
    }
    mutate_state(|s| {
        s.interest_recovery
            .as_mut()
            .expect("retained")
            .live_interest_only = true;
        s.interest_recovery_bootstrap_hold = Some(false);
    });
    Ok(read_state(|s| {
        status(s.interest_recovery.as_ref().expect("retained"))
    }))
}
pub fn get_status() -> Result<Option<RecoveryStatus>, String> {
    admin()?;
    Ok(read_state(|s| s.interest_recovery.as_ref().map(status)))
}
pub fn get_receipt(block: u64) -> Result<Option<RecoveryPacketReceipt>, String> {
    receiver_or_admin()?;
    Ok(read_state(|s| {
        s.interest_recovery.as_ref().and_then(|p| {
            p.manifest
                .packets
                .iter()
                .position(|r| r.source_mint_block == block)
                .filter(|i| *i < p.next_packet as usize)
                .map(|i| {
                    let row = &p.manifest.packets[i];
                    let credited =
                        receipt_store::interest_mint_receipt_status(s, block, &payload(row))
                            == state::InterestMintReceiptStatus::Duplicate;
                    RecoveryPacketReceipt {
                        manifest_hash: p.manifest_hash.clone(),
                        source_mint_block: block,
                        payload: payload(row),
                        allocation_digest: hash(
                            &Encode!(&row.allocations).expect("encode allocation"),
                        ),
                        credited,
                    }
                })
        })
    }))
}
