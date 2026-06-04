//! LIQ-101 (audit 2026-06-03-0c3ceb4): every liquidation entry point must honor
//! the `bot_processing` lock.
//!
//! `bot_claim_liquidation` sets `vault.bot_processing` and DEFERS the debt/
//! collateral write-down until the bot's swap settles. Every user op already
//! calls `require_vault_not_processing(&vault)?`, but the manual and
//! stability-pool liquidation paths did not. A manual / SP liquidation against a
//! bot-claimed vault would seize the same collateral a second time (the recorded
//! collateral still reflects the pre-claim balance), double-seizing from the
//! shared pool.
//!
//! The fix adds `reject_if_bot_processing(vault_id)?` (a vault-id wrapper around
//! `require_vault_not_processing`) immediately after the guard in every
//! liquidation entry, before any icUSD pull or state mutation. This fence reads
//! `src/vault.rs` and asserts each entry calls it; it FAILS on pre-fix code.

use std::path::PathBuf;

fn vault_fn_body<'a>(src: &'a str, header: &str) -> &'a str {
    let start = src
        .find(header)
        .unwrap_or_else(|| panic!("`{}` not found in vault.rs", header));
    let after = start + header.len();
    let end = ["\npub async fn ", "\npub fn ", "\nasync fn ", "\nfn "]
        .iter()
        .filter_map(|m| src[after..].find(m).map(|i| after + i))
        .min()
        .unwrap_or(src.len());
    &src[start..end]
}

#[test]
fn liq_101_all_liquidation_entries_reject_bot_processing() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/vault.rs");
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {}", path.display(), e));

    for header in [
        "pub async fn liquidate_vault_partial(",
        "pub async fn liquidate_vault_partial_with_stable(",
        "pub async fn liquidate_vault_debt_already_burned(",
        "pub async fn liquidate_vault(",
        "pub async fn partial_liquidate_vault(",
    ] {
        let body = vault_fn_body(&src, header);
        assert!(
            body.contains("reject_if_bot_processing"),
            "liquidation entry `{}` must call reject_if_bot_processing(..)? after its guard so it \
             cannot double-seize a vault the bot has already claimed (audit LIQ-101).\n\n{}",
            header, body
        );
    }
}
