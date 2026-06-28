use candid::{CandidType, Nat, Principal};
use serde::{Deserialize, Serialize};

pub const DEFAULT_LOW_WATERMARK_CYCLES: u128 = 1_000_000_000_000;
pub const DEFAULT_TOPUP_CYCLES: u128 = 5_000_000_000_000;
pub const DEFAULT_FREEZE_THRESHOLD_SECS: u64 = 2_592_000;
pub const METRICS_SCHEMA_VERSION: u32 = 1;

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct CycleManagerCyclesStatus {
    pub balance: Nat,
    pub low_watermark: Nat,
    pub healthy: bool,
    pub freeze_threshold_secs: u64,
    pub stable_memory_bytes: Option<u64>,
    pub heap_memory_bytes: Option<u64>,
    pub idle_burn_cycles_per_day: Option<Nat>,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct CycleManagerMetric {
    pub key: String,
    pub count: u64,
    pub value: Nat,
    pub label: Option<String>,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub enum CycleManagerTargetKind {
    SelfReport,
    Controlled,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub enum CycleManagerEnvironment {
    Production,
    Staging,
    Test,
    Local,
    Archived,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub enum CycleManagerCriticality {
    Critical,
    Important,
    Standard,
    Experimental,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct CycleManagerTarget {
    pub canister_id: Principal,
    pub name: String,
    pub project: String,
    pub environment: CycleManagerEnvironment,
    pub criticality: CycleManagerCriticality,
    pub kind: CycleManagerTargetKind,
    pub low_threshold_cycles: Nat,
    pub topup_cycles: Nat,
    pub owner: Option<String>,
    pub tags: Vec<String>,
    pub expected_controllers: Vec<Principal>,
    pub expected_freeze_threshold_secs: Option<u64>,
    pub metrics_schema_version: u32,
}

pub fn cycles_status_from_parts(
    balance: u128,
    low_watermark: u128,
    operational: bool,
    freeze_threshold_secs: u64,
) -> CycleManagerCyclesStatus {
    CycleManagerCyclesStatus {
        balance: Nat::from(balance),
        low_watermark: Nat::from(low_watermark),
        healthy: operational && balance > low_watermark,
        freeze_threshold_secs,
        stable_memory_bytes: None,
        heap_memory_bytes: None,
        idle_burn_cycles_per_day: None,
    }
}

pub fn self_cycles_status(
    low_watermark: u128,
    operational: bool,
    freeze_threshold_secs: u64,
) -> CycleManagerCyclesStatus {
    let mut status = cycles_status_from_parts(
        ic_cdk::api::canister_balance128(),
        low_watermark,
        operational,
        freeze_threshold_secs,
    );
    status.stable_memory_bytes = Some(ic_cdk::api::stable::stable64_size().saturating_mul(65_536));
    status
}

pub fn metric(
    key: &str,
    count: u64,
    value: impl Into<Nat>,
    label: Option<&str>,
) -> CycleManagerMetric {
    CycleManagerMetric {
        key: key.to_string(),
        count,
        value: value.into(),
        label: label.map(str::to_string),
    }
}

pub fn target(
    canister_id: Principal,
    name: &str,
    environment: CycleManagerEnvironment,
    criticality: CycleManagerCriticality,
    low_threshold_cycles: u128,
    topup_cycles: u128,
    tags: &[&str],
) -> CycleManagerTarget {
    CycleManagerTarget {
        canister_id,
        name: name.to_string(),
        project: "Rumi Protocol".to_string(),
        environment,
        criticality,
        kind: CycleManagerTargetKind::SelfReport,
        low_threshold_cycles: Nat::from(low_threshold_cycles),
        topup_cycles: Nat::from(topup_cycles),
        owner: Some("Rumi Labs".to_string()),
        tags: tags.iter().map(|tag| (*tag).to_string()).collect(),
        expected_controllers: Vec::new(),
        expected_freeze_threshold_secs: Some(DEFAULT_FREEZE_THRESHOLD_SECS),
        metrics_schema_version: METRICS_SCHEMA_VERSION,
    }
}

#[cfg(test)]
mod tests {
    use candid::Nat;

    #[test]
    fn status_marks_balance_at_low_watermark_unhealthy() {
        let status = crate::cycles_status_from_parts(999, 1_000, true, 2_592_000);

        assert_eq!(status.balance, Nat::from(999u64));
        assert_eq!(status.low_watermark, Nat::from(1_000u64));
        assert!(!status.healthy);
        assert_eq!(status.freeze_threshold_secs, 2_592_000);
    }

    #[test]
    fn metric_helper_uses_stable_key_count_value_and_label() {
        let metric = crate::metric("op:swap:cycles", 7, 123_456u64, Some("3pool swaps"));

        assert_eq!(metric.key, "op:swap:cycles");
        assert_eq!(metric.count, 7);
        assert_eq!(metric.value, Nat::from(123_456u64));
        assert_eq!(metric.label.as_deref(), Some("3pool swaps"));
    }
}
