//! rumi_analytics canister - Phase 1.
//! See docs/plans/2026-04-07-rumi-analytics-design.md.

mod storage;

#[ic_cdk_macros::query]
fn ping() -> &'static str {
    "rumi_analytics ok"
}

ic_cdk::export_candid!();
