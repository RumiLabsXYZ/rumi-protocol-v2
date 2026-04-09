//! ICRC-3 block tailing. Fetches blocks from icusd_ledger or 3pool,
//! parses transfer/mint/burn operations, applies to BalanceTracker.

use candid::Principal;
use crate::{sources, state, storage};
use storage::balance_tracker::{self, Account, Token};
use storage::cursors;
use sources::icusd_ledger::ICRC3Value;
use super::{BATCH_SIZE, BACKFILL_BATCH_SIZE, update_cursor_success, update_cursor_error, update_cursor_source_count};

pub async fn tail_icusd_blocks() {
    let ledger = state::read_state(|s| s.sources.icusd_ledger);
    let is_backfill = state::read_state(|s| s.backfill_active_icusd.unwrap_or(false));
    let batch = if is_backfill { BACKFILL_BATCH_SIZE } else { BATCH_SIZE };

    tail_blocks(
        ledger,
        Token::IcUsd,
        cursors::CURSOR_ID_ICUSD_BLOCKS,
        || cursors::icusd_blocks::get(),
        |v| cursors::icusd_blocks::set(v),
        batch,
        is_backfill,
    ).await;
}

pub async fn tail_3pool_blocks() {
    let three_pool = state::read_state(|s| s.sources.three_pool);
    let is_backfill = state::read_state(|s| s.backfill_active_3usd.unwrap_or(false));
    let batch = if is_backfill { BACKFILL_BATCH_SIZE } else { BATCH_SIZE };

    tail_blocks(
        three_pool,
        Token::ThreeUsd,
        cursors::CURSOR_ID_3POOL_BLOCKS,
        || cursors::three_pool_blocks::get(),
        |v| cursors::three_pool_blocks::set(v),
        batch,
        is_backfill,
    ).await;
}

async fn tail_blocks<G, S>(
    canister: Principal,
    token: Token,
    cursor_id: u8,
    get_cursor: G,
    set_cursor: S,
    batch_size: u64,
    is_backfill: bool,
) where
    G: Fn() -> u64,
    S: Fn(u64),
{
    let cursor = get_cursor();

    let result = match sources::icusd_ledger::icrc3_get_blocks(canister, cursor, batch_size).await {
        Ok(r) => r,
        Err(e) => {
            ic_cdk::println!("[tail_icrc3] icrc3_get_blocks failed for {:?}: {}", token, e);
            state::mutate_state(|s| {
                match token {
                    Token::IcUsd => s.error_counters.icusd_ledger += 1,
                    Token::ThreeUsd => s.error_counters.three_pool += 1,
                }
                update_cursor_error(s, cursor_id, e.clone());
            });
            return;
        }
    };

    let log_length = nat_to_u64(&result.log_length);

    if result.blocks.is_empty() {
        if is_backfill && cursor >= log_length {
            clear_backfill_flag(token);
        }
        return;
    }

    let mut processed = 0u64;
    for block_with_id in &result.blocks {
        if let Err(e) = process_block(token, &block_with_id.block) {
            ic_cdk::println!("[tail_icrc3] skipping malformed block: {}", e);
        }
        processed += 1;
    }

    if processed > 0 {
        set_cursor(cursor + processed);
        state::mutate_state(|s| {
            update_cursor_success(s, cursor_id, ic_cdk::api::time());
            update_cursor_source_count(s, cursor_id, log_length);
        });
    }

    if is_backfill && (cursor + processed) >= log_length {
        clear_backfill_flag(token);
    }
}

fn clear_backfill_flag(token: Token) {
    state::mutate_state(|s| {
        match token {
            Token::IcUsd => s.backfill_active_icusd = Some(false),
            Token::ThreeUsd => s.backfill_active_3usd = Some(false),
        }
    });
    ic_cdk::println!("[tail_icrc3] backfill complete for {:?}", token);
}

fn process_block(token: Token, block: &ICRC3Value) -> Result<(), String> {
    let timestamp_ns = extract_nat_field(block, "ts").unwrap_or(0);

    let tx = extract_map_field(block, "tx")
        .ok_or_else(|| "missing tx field".to_string())?;

    // Resolve block type. ICRC-3 canonical form puts it at the block level as
    // "btype" (e.g., "1mint", "1xfer"). The DFINITY reference ic-icrc1-ledger
    // and some older implementations instead put "op" inside the "tx" map
    // (e.g., "mint", "burn", "xfer"). We handle both conventions.
    let btype: String = if let Some(bt) = extract_text_field(block, "btype") {
        bt
    } else if let Some(op) = extract_text_field(&tx, "op") {
        // Normalize "op" values to the ICRC-3 btype convention.
        match op.as_str() {
            "mint" => "1mint".to_string(),
            "burn" => "1burn".to_string(),
            "xfer" | "transfer" => "1xfer".to_string(),
            "approve" => "2approve".to_string(),
            other => other.to_string(),
        }
    } else {
        String::new()
    };

    match btype.as_str() {
        "1xfer" => {
            let from = extract_account_field(&tx, "from")
                .ok_or_else(|| "1xfer missing from".to_string())?;
            let to = extract_account_field(&tx, "to")
                .ok_or_else(|| "1xfer missing to".to_string())?;
            let amt = extract_nat_field(&tx, "amt")
                .ok_or_else(|| "1xfer missing amt".to_string())?;
            // NOTE: When fee is absent from the block, ICRC-1 charged the default
            // fee. We default to 0 here, which may cause tracked sender balances
            // to drift slightly high if the ledger's default fee is nonzero.
            // Acceptable for analytics; revisit if precision matters.
            let fee = extract_nat_field(&tx, "fee").unwrap_or(0);
            balance_tracker::apply_transfer(token, &from, &to, amt, fee, timestamp_ns);
        }
        "1mint" => {
            let to = extract_account_field(&tx, "to")
                .ok_or_else(|| "1mint missing to".to_string())?;
            let amt = extract_nat_field(&tx, "amt")
                .ok_or_else(|| "1mint missing amt".to_string())?;
            balance_tracker::apply_mint(token, &to, amt, timestamp_ns);
        }
        "1burn" => {
            let from = extract_account_field(&tx, "from")
                .ok_or_else(|| "1burn missing from".to_string())?;
            let amt = extract_nat_field(&tx, "amt")
                .ok_or_else(|| "1burn missing amt".to_string())?;
            balance_tracker::apply_burn(token, &from, amt);
        }
        "2approve" => {
            // No balance change
        }
        other => {
            ic_cdk::println!("[tail_icrc3] unknown btype: {}", other);
        }
    }

    Ok(())
}

// --- ICRC3Value extraction helpers ---
// ICRC3Value::Map is BTreeMap<String, ICRC3Value>
// ICRC3Value::Blob is serde_bytes::ByteBuf

fn extract_map_field(value: &ICRC3Value, field: &str) -> Option<ICRC3Value> {
    match value {
        ICRC3Value::Map(entries) => entries.get(field).cloned(),
        _ => None,
    }
}

fn extract_nat_field(value: &ICRC3Value, field: &str) -> Option<u64> {
    match extract_map_field(value, field)? {
        ICRC3Value::Nat(n) => Some(nat_to_u64(&n)),
        _ => None,
    }
}

fn extract_text_field(value: &ICRC3Value, field: &str) -> Option<String> {
    match extract_map_field(value, field)? {
        ICRC3Value::Text(s) => Some(s),
        _ => None,
    }
}

fn extract_account_field(value: &ICRC3Value, field: &str) -> Option<Account> {
    let acct_val = extract_map_field(value, field)?;
    match &acct_val {
        ICRC3Value::Map(entries) => {
            let owner_blob = entries.get("owner")
                .and_then(|v| match v { ICRC3Value::Blob(b) => Some(b), _ => None })?;
            let owner = Principal::from_slice(owner_blob.as_ref());
            let subaccount = entries.get("subaccount")
                .and_then(|v| match v {
                    ICRC3Value::Blob(b) => {
                        let bytes = b.as_ref();
                        if bytes.len() == 32 {
                            let mut sa = [0u8; 32];
                            sa.copy_from_slice(bytes);
                            Some(sa)
                        } else {
                            None
                        }
                    }
                    _ => None,
                });
            Some(Account { owner, subaccount })
        }
        ICRC3Value::Array(arr) => {
            let owner_blob = match arr.first()? {
                ICRC3Value::Blob(b) => b,
                _ => return None,
            };
            let owner = Principal::from_slice(owner_blob.as_ref());
            let subaccount = arr.get(1).and_then(|v| match v {
                ICRC3Value::Blob(b) => {
                    let bytes = b.as_ref();
                    if bytes.len() == 32 {
                        let mut sa = [0u8; 32];
                        sa.copy_from_slice(bytes);
                        Some(sa)
                    } else {
                        None
                    }
                }
                _ => None,
            });
            Some(Account { owner, subaccount })
        }
        _ => None,
    }
}

fn nat_to_u64(nat: &candid::Nat) -> u64 {
    use num_traits::ToPrimitive;
    nat.0.to_u64().unwrap_or(u64::MAX)
}
