//! BK-001 / BK-002 (audit 2026-06-05-d67e100): every collateral-seizing
//! liquidation entry point must hold a PER-VAULT liquidation lock, not just the
//! per-CALLER `GuardPrincipal`.
//!
//! Root cause: `GuardPrincipal::new(caller, "..._{vault_id}")` keys on the
//! CALLER principal (the vault id only decorates the operation-name string), so
//! two different liquidators (two humans, or the stability pool + a human)
//! racing the SAME vault both pass the guard. Each snapshots the vault's full
//! collateral before its `await`; the ASYNC-001 re-cap fixed the vault-
//! accounting underflow/wrap, but the collateral PAYOUT
//! (`pending_margin_transfers.insert(.. margin: collateral_to_liquidator ..)`)
//! still uses the STALE per-caller snapshot. The loser is therefore paid the
//! full pre-state collateral out of the SHARED collateral pool (`from_subaccount:
//! None`), draining other vaults' backing — an economic over-seize / insolvency
//! vector the re-cap alone did not close.
//!
//! Fix: a `VaultLiquidationGuard` keyed on `vault_id` (transient thread-local,
//! released on return and on continuation-trap via ic-cdk cleanup), acquired in
//! all five manual/SP liquidation entry points AND in `bot_claim_liquidation`.
//!
//! Two fences below:
//!   1. Source fence: each liquidation entry acquires `VaultLiquidationGuard`.
//!   2. Behavioral fence: the guard is exclusive per vault and independent
//!      across vaults (re-exports the in-crate unit test invariant).

use std::path::PathBuf;

fn fn_body<'a>(src: &'a str, header: &str) -> &'a str {
    let start = src
        .find(header)
        .unwrap_or_else(|| panic!("`{}` not found", header));
    let after = start + header.len();
    let end = ["\npub async fn ", "\npub fn ", "\nasync fn ", "\nfn "]
        .iter()
        .filter_map(|m| src[after..].find(m).map(|i| after + i))
        .min()
        .unwrap_or(src.len());
    &src[start..end]
}

#[test]
fn bk_001_002_manual_sp_entries_hold_per_vault_guard() {
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
        let body = fn_body(&src, header);
        assert!(
            body.contains("VaultLiquidationGuard::new"),
            "liquidation entry `{}` must acquire VaultLiquidationGuard::new(vault_id)? so two \
             different callers cannot race the same vault and both be paid the full pre-state \
             collateral from the shared pool (audit BK-001/002).",
            header
        );
    }
}

#[test]
fn bk_001_002_bot_claim_holds_per_vault_guard() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/main.rs");
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {}", path.display(), e));
    let body = fn_body(&src, "async fn bot_claim_liquidation(");
    assert!(
        body.contains("VaultLiquidationGuard::new"),
        "bot_claim_liquidation must also acquire the per-vault liquidation guard so a bot claim \
         cannot interleave with an in-flight manual/SP liquidation of the same vault (BK-001/002)."
    );
}
