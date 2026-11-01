use candid::{CandidType, Deserialize, Nat, Principal};

#[derive(CandidType)]
struct Rate {
    timestamp_seconds: u64,
    xdr_permyriad_per_icp: u64,
}
#[derive(CandidType)]
struct RateResponse {
    data: Rate,
}
#[ic_cdk::update]
fn get_icp_xdr_conversion_rate() -> RateResponse {
    RateResponse {
        data: Rate {
            timestamp_seconds: ic_cdk::api::time() / 1_000_000_000,
            xdr_permyriad_per_icp: 40_000,
        },
    }
}
#[derive(CandidType, Deserialize)]
struct DepositSolArgs {
    owner: Option<Principal>,
    subaccount: Option<[u8; 32]>,
}
#[derive(CandidType, Deserialize)]
struct InsufficientCyclesError {
    expected: Nat,
    received: Nat,
}
#[derive(CandidType, Deserialize)]
struct ValueTooSmallError {
    balance: u64,
    minimum_deposit_amount: u64,
}
#[derive(CandidType, Deserialize)]
enum DepositSolError {
    InsufficientCycles(InsufficientCyclesError),
    TemporarilyUnavailable(String),
    AlreadyProcessing,
    ValueTooSmall(ValueTooSmallError),
    Quarantined { deposit_id: u64 },
}
#[ic_cdk::update]
fn deposit_sol(_args: DepositSolArgs) -> Result<u64, DepositSolError> {
    ic_cdk::api::call::msg_cycles_accept128(100_000_000_000);
    Ok(42)
}
