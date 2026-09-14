use rumi_cycle_manager::{
    self_cycles_status, CycleManagerCyclesStatus, DEFAULT_FREEZE_THRESHOLD_SECS,
    DEFAULT_LOW_WATERMARK_CYCLES,
};

mod state;
mod types;

#[ic_cdk::init]
fn init(args: types::InitArgs) {
    if let Err(err) = state::init(args) {
        ic_cdk::trap(&format!("rumi_cycle_sentinel: init failed: {err:?}"));
    }
}

#[ic_cdk::post_upgrade]
fn post_upgrade() {
    if let Err(err) = state::validate_whole_state(ic_cdk::id()) {
        ic_cdk::trap(&format!(
            "rumi_cycle_sentinel: post_upgrade state validation failed: {err:?}"
        ));
    }
}

#[ic_cdk::query]
fn cycles_status() -> CycleManagerCyclesStatus {
    self_cycles_status(
        DEFAULT_LOW_WATERMARK_CYCLES,
        true,
        DEFAULT_FREEZE_THRESHOLD_SECS,
    )
}
