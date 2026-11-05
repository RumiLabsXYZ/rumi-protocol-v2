use candid::Principal;
use std::cell::Cell;
thread_local! {
    static STARTED: Cell<u64> = const { Cell::new(0) };
    static FINISHED: Cell<u64> = const { Cell::new(0) };
    static TARGET_COMPLETED: Cell<u64> = const { Cell::new(0) };
}
#[ic_cdk::query]
fn started() -> u64 {
    STARTED.with(Cell::get)
}
#[ic_cdk::query]
fn finished() -> u64 {
    FINISHED.with(Cell::get)
}
#[ic_cdk::query]
fn target_completed() -> u64 {
    TARGET_COMPLETED.with(Cell::get)
}
#[ic_cdk::update]
async fn start(target: Principal) {
    STARTED.with(|c| c.set(c.get() + 1));
    let _: ((),) = ic_cdk::call(target, "hold_reply", (20u32,)).await.unwrap();
    FINISHED.with(|c| c.set(c.get() + 1));
}
#[ic_cdk::update]
async fn hold_reply(depth: u32) {
    if depth > 0 {
        let cmc = Principal::from_text("rkp4c-7iaaa-aaaaa-aaaca-cai").unwrap();
        let minter = Principal::from_text("lh22c-kyaaa-aaaar-qb5nq-cai").unwrap();
        let target = if ic_cdk::id() == cmc { minter } else { cmc };
        let _: ((),) = ic_cdk::call(target, "hold_reply", (depth - 1,))
            .await
            .unwrap();
    }
    if depth == 20 {
        TARGET_COMPLETED.with(|c| c.set(c.get() + 1));
    }
}
