use crate::Vault;
use crate::{ICP, ICUSD};
use candid::Principal;
use ic_base_types::PrincipalId;
use proptest::prelude::*;
use std::collections::BTreeMap;
use proptest::collection::vec as pvec;

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
    1..1_000_000u64  // Reduced maximum to avoid impossible distributions
}

fn vault_vec_to_map(vaults: Vec<Vault>) -> BTreeMap<u64, Vault> {
    vaults.into_iter().enumerate().map(|(i, mut v)| {
        v.vault_id = i as u64;
        (i as u64, v)
    }).collect()
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
        let legacy = LegacyGetEventsArg { start: 0, length: 100 };
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
