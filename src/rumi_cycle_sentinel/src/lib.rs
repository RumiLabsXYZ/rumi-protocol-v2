use rumi_cycle_manager::{
    self_cycles_status, CycleManagerCyclesStatus, DEFAULT_FREEZE_THRESHOLD_SECS,
    DEFAULT_LOW_WATERMARK_CYCLES,
};

#[ic_cdk::query]
fn cycles_status() -> CycleManagerCyclesStatus {
    self_cycles_status(
        DEFAULT_LOW_WATERMARK_CYCLES,
        true,
        DEFAULT_FREEZE_THRESHOLD_SECS,
    )
}
