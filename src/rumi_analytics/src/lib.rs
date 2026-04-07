//! rumi_analytics canister - Phase 1 skeleton.
//! See docs/plans/2026-04-07-rumi-analytics-design.md for the full design.

#[ic_cdk_macros::query]
fn ping() -> &'static str {
    "rumi_analytics ok"
}

ic_cdk::export_candid!();
