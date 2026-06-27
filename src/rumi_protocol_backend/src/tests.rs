use crate::Vault;
use crate::{ICP, ICUSD};
use candid::Principal;
use ic_base_types::PrincipalId;
use proptest::collection::vec as pvec;
use proptest::prelude::*;
use std::collections::BTreeMap;

fn arb_vault() -> impl Strategy<Value = Vault> {
    (arb_principal(), any::<u64>(), arb_amount()).prop_map(|(owner, borrowed_icusd, icp_margin)| {
        Vault {
            owner,
            borrowed_icusd_amount: ICUSD::from(borrowed_icusd),
            collateral_amount: icp_margin.max(1_000_000),
            vault_id: 0,
            collateral_type: Principal::anonymous(),
            last_accrual_time: 0,
            accrued_interest: ICUSD::new(0),
            bot_processing: false,
        }
    })
}

fn arb_principal() -> impl Strategy<Value = Principal> {
    pvec(any::<u8>(), 32).prop_map(|bytes| {
        let mut buf = [0u8; 32];
        buf.copy_from_slice(&bytes);
        PrincipalId::new_self_authenticating(&buf).0
    })
}

fn arb_usd_amount() -> impl Strategy<Value = ICUSD> {
    arb_amount().prop_map(|a| ICUSD::from(a))
}

fn arb_amount() -> impl Strategy<Value = u64> {
    1..1_000_000u64 // Reduced maximum to avoid impossible distributions
}

fn vault_vec_to_map(vaults: Vec<Vault>) -> BTreeMap<u64, Vault> {
    vaults
        .into_iter()
        .enumerate()
        .map(|(i, mut v)| {
            v.vault_id = i as u64;
            (i as u64, v)
        })
        .collect()
}

proptest! {
    #[test]
    fn test_vault_distribution(
        vaults_vec in pvec(arb_vault(), 1..10),
        target_borrowed_icusd in any::<u64>(),
        target_icp_margin in arb_amount(),
    ) {
        let vaults = vault_vec_to_map(vaults_vec.clone());
        let sum_icp_margin: u64 = vaults.values().map(|v| v.collateral_amount).sum();

        // Only test distribution if we have enough ICP margin available
        if target_icp_margin <= sum_icp_margin {
            let target_vault = Vault {
                owner: Principal::anonymous(),
                borrowed_icusd_amount: ICUSD::from(target_borrowed_icusd),
                collateral_amount: target_icp_margin,
                vault_id: vaults.last_key_value().unwrap().1.vault_id + 1,
                collateral_type: Principal::anonymous(),
                last_accrual_time: 0,
                accrued_interest: ICUSD::new(0),
                bot_processing: false,
            };

            let result = crate::state::distribute_across_vaults(&vaults, target_vault);
            let icusd_distributed: ICUSD = result.iter().map(|e| e.icusd_share_amount).sum();
            let icp_distributed: ICP = result.iter().map(|e| e.icp_share_amount).sum();

            assert_eq!(icusd_distributed, ICUSD::from(target_borrowed_icusd));
            assert_eq!(icp_distributed, ICP::from(target_icp_margin));
        }
    }
}

#[cfg(test)]
mod candid_compat {
    use crate::GetEventsArg;
    use candid::{decode_one, encode_one, CandidType, Deserialize};

    /// Pre-extension wire shape: any client compiled before the new optional
    /// filter fields were added still encodes a record with just `start` and
    /// `length`. Decoding it as the extended `GetEventsArg` must succeed and
    /// leave every new field as `None`.
    #[derive(CandidType, Deserialize)]
    struct LegacyGetEventsArg {
        start: u64,
        length: u64,
    }

    #[test]
    fn legacy_two_field_arg_decodes_into_extended_struct() {
        let legacy = LegacyGetEventsArg {
            start: 0,
            length: 100,
        };
        let bytes = encode_one(&legacy).expect("encode legacy");
        let decoded: GetEventsArg = decode_one(&bytes).expect("decode into extended");

        assert_eq!(decoded.start, 0);
        assert_eq!(decoded.length, 100);
        assert!(decoded.types.is_none());
        assert!(decoded.principal.is_none());
        assert!(decoded.collateral_token.is_none());
        assert!(decoded.time_range.is_none());
        assert!(decoded.min_size_e8s.is_none());
    }
}

#[cfg(test)]
mod validate_f64_inclusive {
    use crate::validate_f64_inclusive;

    #[test]
    fn accepts_values_inside_range_inclusive() {
        assert!(validate_f64_inclusive("x", 0.0, 0.0, 0.50).is_ok());
        assert!(validate_f64_inclusive("x", 0.25, 0.0, 0.50).is_ok());
        assert!(validate_f64_inclusive("x", 0.50, 0.0, 0.50).is_ok());
    }

    #[test]
    fn rejects_nan() {
        let err = validate_f64_inclusive("haircut", f64::NAN, 0.0, 0.50).unwrap_err();
        assert!(err.contains("haircut"), "err={}", err);
        assert!(err.contains("finite"), "err={}", err);
    }

    #[test]
    fn rejects_positive_infinity() {
        let err = validate_f64_inclusive("haircut", f64::INFINITY, 0.0, 0.50).unwrap_err();
        assert!(err.contains("finite"), "err={}", err);
    }

    #[test]
    fn rejects_negative_infinity() {
        let err = validate_f64_inclusive("haircut", f64::NEG_INFINITY, 0.0, 0.50).unwrap_err();
        assert!(err.contains("finite"), "err={}", err);
    }

    #[test]
    fn rejects_below_min() {
        assert!(validate_f64_inclusive("x", -0.01, 0.0, 0.50).is_err());
    }

    #[test]
    fn rejects_above_max() {
        assert!(validate_f64_inclusive("x", 0.51, 0.0, 0.50).is_err());
    }
}

#[test]
fn protocol_error_carries_multi_chain_variants() {
    use crate::ProtocolError;
    use candid::{Decode, Encode};
    let halt = ProtocolError::SupplyInvariantHalted;
    let admin = ProtocolError::ChainAdmin("not developer".to_string());
    let halt_bytes = Encode!(&halt).expect("encode halt");
    let admin_bytes = Encode!(&admin).expect("encode admin");
    let _: ProtocolError = Decode!(&halt_bytes, ProtocolError).expect("decode halt");
    let _: ProtocolError = Decode!(&admin_bytes, ProtocolError).expect("decode admin");
}

#[test]
fn supply_audit_round_trips_via_candid() {
    use crate::chains::config::ChainId;
    use crate::{SupplyAudit, SupplyAuditEntry};
    use candid::{Decode, Encode};

    let audit = SupplyAudit {
        total_e8s: 150_000,
        per_chain: vec![
            SupplyAuditEntry {
                chain_id: ChainId(1),
                display_name: "ICP".into(),
                supply_e8s: 100_000,
            },
            SupplyAuditEntry {
                chain_id: ChainId(2),
                display_name: "Monad".into(),
                supply_e8s: 50_000,
            },
        ],
    };
    let bytes = Encode!(&audit).expect("encode");
    let back: SupplyAudit = Decode!(&bytes, SupplyAudit).expect("decode");
    assert_eq!(back.total_e8s, 150_000);
    assert_eq!(back.per_chain.len(), 2);
}

#[test]
fn monad_event_variants_round_trip_via_candid() {
    use crate::chains::config::ChainId;
    use crate::event::Event;
    use candid::{Decode, Encode};

    let events = vec![
        Event::DepositObserved {
            chain_id: ChainId(10143),
            vault_id: 1,
            custody_address: "0xa".into(),
            amount_e18: 5,
            tx_hash: "0xh".into(),
            block_number: 100,
            timestamp: 1,
        },
        Event::ChainMintSubmitted {
            chain_id: ChainId(10143),
            vault_id: 1,
            op_id: 0,
            recipient: "0xr".into(),
            amount_e8s: 10,
            tx_hash: "0xs".into(),
            timestamp: 2,
        },
        Event::ChainMintConfirmed {
            chain_id: ChainId(10143),
            vault_id: 1,
            op_id: 0,
            amount_e8s: 10,
            tx_hash: "0xs".into(),
            block_number: 102,
            timestamp: 3,
        },
        Event::ChainBurnObserved {
            chain_id: ChainId(10143),
            vault_id: 1,
            amount_e8s: 4,
            tx_hash: "0xb".into(),
            block_number: 110,
            timestamp: 4,
        },
        Event::WithdrawalSigned {
            chain_id: ChainId(10143),
            vault_id: 1,
            op_id: 1,
            recipient: "0xw".into(),
            amount_e18: 5,
            tx_hash: "0xt".into(),
            timestamp: 5,
        },
        Event::ChainSettlementFailed {
            chain_id: ChainId(10143),
            op_id: 1,
            reason: "reverted".into(),
            timestamp: 6,
        },
        Event::ChainReorgDetected {
            chain_id: ChainId(10143),
            observed_block: 100,
            reorg_depth: 5,
            timestamp: 7,
        },
        Event::ChainHotWalletLow {
            chain_id: ChainId(10143),
            balance_e18: 1,
            threshold_e18: 100,
            timestamp: 8,
        },
        Event::ChainBadDebtCircuitThresholdSet {
            chain_id: ChainId(10143),
            threshold_e8s: Some(10),
            timestamp: 9,
        },
        Event::ChainBadDebtCircuitTripped {
            chain_id: ChainId(10143),
            bad_debt_e8s: 1,
            total_bad_debt_e8s: 10,
            threshold_e8s: 10,
            timestamp: 10,
        },
        Event::ChainBadDebtCircuitCleared {
            chain_id: ChainId(10143),
            total_bad_debt_e8s: 10,
            timestamp: 11,
        },
    ];
    for e in events {
        let bytes = Encode!(&e).expect("encode");
        let _: Event = Decode!(&bytes, Event).expect("decode");
    }
}
