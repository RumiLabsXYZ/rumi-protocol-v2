//! AR-B-001 / AR-B-003 / RED-001 / RED-002 / ORC-001 (audit 2026-06-09-e49ed10).
//!
//! AR-B-001: the redemption water-fill bypassed both the per-vault op lock and
//! the `bot_processing` exclusion, so a redemption could seize collateral from
//! a vault a liquidation (or the bot's claim->confirm window) was mid-flight
//! on — double-paying from the single shared collateral account. The
//! bot-confirm path also used a NON-saturating debt subtraction that trapped
//! (vault permanently stuck `bot_processing=true`) if anything reduced the
//! vault's debt during the claim->confirm window.
//!
//! AR-B-003: owner write-ops (borrow / repay / partial-withdraw / add-margin /
//! close) held only the per-caller `GuardPrincipal`, so a liquidation or
//! redemption could mutate the vault between the op's pre-`await` snapshot and
//! its post-`await` commit; the commits used asserting arithmetic that trapped
//! AFTER the user's transfer had already settled (phantom collateral / repaid
//! icUSD with no debt credit).
//!
//! RED-001: the redemption payout (`margin`) was computed from the requested
//! claim, not from the icUSD the water-fill actually consumed, so an
//! over-large redemption burned icUSD it could not consume and was paid
//! collateral no vault was debited for, draining co-collateral backing.
//!
//! RED-002: `redeem_collateral` validated freshness, priced the fee, and
//! bumped the base rate against the CALLER-supplied collateral type while the
//! water-fill seized the priority winner.
//!
//! ORC-001: `bot_claim_liquidation` omitted the per-vault freshness gate and
//! the liquidation-freeze brake that every manual/SP liquidation entry
//! enforces.
//!
//! These are source fences (the state-level behavioral fences live in
//! `state.rs`'s `arb001_*` / `arb003_*` / `red001_*` unit tests, and the lock
//! exclusivity invariant in `guard.rs`).

use std::path::PathBuf;

fn read(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

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
fn arb003_user_write_ops_hold_per_vault_guard() {
    let src = read("src/vault.rs");
    for header in [
        "pub async fn borrow_from_vault(",
        "pub async fn repay_to_vault(",
        "pub async fn repay_to_vault_with_stable(",
        "pub async fn partial_repay_to_vault(",
        "pub async fn add_margin_to_vault(",
        "pub async fn add_margin_with_deposit(",
        "pub async fn close_vault(",
        "pub async fn withdraw_collateral(",
        "pub async fn withdraw_partial_collateral(",
        "pub async fn withdraw_and_close_vault(",
        "pub async fn repay_and_close_vault(",
    ] {
        let body = fn_body(&src, header);
        assert!(
            body.contains("VaultLiquidationGuard::new"),
            "owner write-op `{}` must acquire the per-vault op lock so a concurrent \
             liquidation/redemption cannot invalidate its pre-await snapshot (AR-B-003).",
            header
        );
    }
    // The open-and-borrow compound paths lock around their borrow phase.
    for header in [
        "pub async fn open_vault_and_borrow(",
        "pub async fn open_vault_with_deposit(",
    ] {
        let body = fn_body(&src, header);
        assert!(
            body.contains("VaultLiquidationGuard::new"),
            "compound open+borrow `{}` must lock the fresh vault across its borrow await (AR-B-003).",
            header
        );
    }
}

#[test]
fn arb001_redemption_waterfill_skips_locked_and_bot_vaults() {
    let src = read("src/state.rs");
    let body = fn_body(&src, "pub fn redeem_on_vaults(");
    assert!(
        body.contains("bot_processing") && body.contains("is_vault_liquidating"),
        "redeem_on_vaults must skip bot-claimed vaults AND vaults under the per-vault op \
         lock; otherwise redemption seizes collateral another flow already paid out (AR-B-001)."
    );
    let helper = fn_body(&src, "pub fn total_redeemable_debt_for(");
    assert!(
        helper.contains("bot_processing") && helper.contains("is_vault_liquidating"),
        "total_redeemable_debt_for must mirror the water-fill's eligibility filter (RED-001)."
    );
}

#[test]
fn arb001_bot_confirm_and_admin_resolve_saturate() {
    let src = read("src/main.rs");
    for header in [
        "async fn bot_confirm_liquidation(",
        "fn admin_resolve_stuck_claim(",
    ] {
        let body = fn_body(&src, header);
        assert!(
            !body.contains("-= ICUSD::new(claim.debt_amount)"),
            "`{}` must not use a non-saturating debt subtraction: it traps on a shrunken \
             vault, sticking it at bot_processing=true with the bot's collateral gone (AR-B-001).",
            header
        );
        assert!(
            body.contains("saturating_sub(ICUSD::new(claim.debt_amount))"),
            "`{}` must saturate the claim write-down (AR-B-001).",
            header
        );
    }
}

#[test]
fn red001_redemption_payout_derived_from_consumed() {
    let src = read("src/event.rs");
    let body = fn_body(&src, "pub fn record_redemption_on_vaults(");
    assert!(
        body.contains("let margin: ICP = consumed / ct_price"),
        "the redemption payout must be derived from the icUSD actually consumed by the \
         water-fill, never from the requested claim (RED-001)."
    );
    assert!(
        !body.contains("let margin: ICP = icusd_amount / ct_price"),
        "claim-derived payout regressed (RED-001)."
    );

    let vault_src = read("src/vault.rs");
    let redeem = fn_body(&vault_src, "pub async fn redeem_collateral(");
    assert!(
        redeem.contains("total_redeemable_debt_for"),
        "redeem_collateral must reject claims exceeding the redeemable debt up front (RED-001)."
    );
    assert!(
        redeem.contains("pending_refunds.insert"),
        "redeem_collateral must durably refund the unconsumed remainder when the inline \
         refund fails (RED-001, ICC-007 saga)."
    );
}

#[test]
fn red002_redeem_collateral_keys_checks_on_priority_winner() {
    let src = read("src/vault.rs");
    let body = fn_body(&src, "pub async fn redeem_collateral(");
    assert!(
        body.contains("get_collateral_types_by_redemption_priority"),
        "redeem_collateral must resolve the priority winner up front (RED-002)."
    );
    assert!(
        body.contains("ensure_fresh_price_for(&redeem_ct)"),
        "freshness must be enforced on the collateral actually seized (RED-002)."
    );
    assert!(
        body.contains("get_redemption_fee_for(&redeem_ct"),
        "the dynamic fee must be priced against the seized collateral (RED-002)."
    );
    assert!(
        !body.contains("get_redemption_fee_for(&collateral_type"),
        "fee keyed on the caller-supplied type regressed (RED-002)."
    );
}

#[test]
fn orc001_bot_claim_enforces_freshness_and_freeze_gates() {
    let src = read("src/main.rs");
    let body = fn_body(&src, "async fn bot_claim_liquidation(");
    assert!(
        body.contains("validate_freshness_for_vault(vault_id).await?"),
        "bot_claim_liquidation must enforce the per-collateral freshness gate (ORC-001)."
    );
    assert!(
        body.contains("validate_liquidation_not_frozen()?"),
        "bot_claim_liquidation must honor the liquidation freeze brake (ORC-001)."
    );
}

#[test]
fn arb001_liquidation_payouts_recapped_to_applied_amounts() {
    let src = read("src/vault.rs");
    for header in [
        "pub async fn liquidate_vault_partial(",
        "pub async fn liquidate_vault_partial_with_stable(",
        "pub async fn liquidate_vault_debt_already_burned(",
        "pub async fn partial_liquidate_vault(",
    ] {
        let body = fn_body(&src, header);
        assert!(
            body.contains("payout_to_liquidator"),
            "`{}` must pay the post-await APPLIED collateral, not the stale pre-await \
             snapshot (AR-B-001/BK-001).",
            header
        );
        assert!(
            !body.contains("margin: collateral_to_liquidator"),
            "`{}` still inserts the stale pre-await payout (AR-B-001/BK-001).",
            header
        );
    }
    let full = fn_body(&src, "pub async fn liquidate_vault(");
    assert!(
        full.contains("liquidator_pay") && full.contains("live_collateral"),
        "liquidate_vault must clamp liquidator + excess payouts to live collateral (AR-B-001)."
    );
}
