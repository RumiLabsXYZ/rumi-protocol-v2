#[cfg(test)]
mod tests {
    use crate::state::{WithdrawalRequestRecord, WithdrawalRequestStatus, WithdrawalStart};
    use crate::types::*;
    use crate::{
        Icrc3ValueV2, NativeIcpBlockV2, NativeIcpOperationV2, NativeIcpTimestampV2,
        NativeIcpTokensV2,
    };
    use candid::Principal;

    fn icrc_account(p: Principal) -> Icrc3ValueV2 {
        Icrc3ValueV2::Array(vec![Icrc3ValueV2::Blob(p.as_slice().to_vec())])
    }

    fn icrc3_receipt(r: &WithdrawalRequestRecord, request_id: u64) -> Icrc3ValueV2 {
        Icrc3ValueV2::Map(vec![
            ("btype".into(), Icrc3ValueV2::Text("1xfer".into())),
            (
                "tx".into(),
                Icrc3ValueV2::Map(vec![
                    ("op".into(), Icrc3ValueV2::Text("xfer".into())),
                    ("from".into(), icrc_account(Principal::from_slice(&[8]))),
                    ("to".into(), icrc_account(r.to)),
                    ("amt".into(), Icrc3ValueV2::Nat(r.send_amount.into())),
                    ("fee".into(), Icrc3ValueV2::Nat(r.fee.into())),
                    ("ts".into(), Icrc3ValueV2::Nat(r.created_at_time.into())),
                    (
                        "memo".into(),
                        Icrc3ValueV2::Blob(
                            r.memo
                                .as_ref()
                                .map(|m| m.as_bytes().to_vec())
                                .unwrap_or_else(|| request_id.to_be_bytes().to_vec()),
                        ),
                    ),
                ]),
            ),
        ])
    }

    #[test]
    fn icrc3_receipt_reconciliation_requires_exact_tuple_and_known_fields() {
        let r = withdrawal_record(Principal::from_slice(&[9]));
        let block = icrc3_receipt(&r, 123);
        assert!(
            crate::verify_icrc3_withdrawal(&block, &r, 123, Principal::from_slice(&[8])).is_ok()
        );
        let mut wrong = r.clone();
        wrong.to = Principal::from_slice(&[10]);
        assert!(
            crate::verify_icrc3_withdrawal(&block, &wrong, 123, Principal::from_slice(&[8]))
                .is_err()
        );
        let mut wrong_time = r.clone();
        wrong_time.created_at_time += 1;
        assert!(crate::verify_icrc3_withdrawal(
            &block,
            &wrong_time,
            123,
            Principal::from_slice(&[8])
        )
        .is_err());
        let mut unknown = block.clone();
        if let Icrc3ValueV2::Map(root) = &mut unknown {
            if let Some((_, Icrc3ValueV2::Map(tx))) = root.iter_mut().find(|(key, _)| key == "tx") {
                tx.push(("future_field".into(), Icrc3ValueV2::Text("unknown".into())));
            }
        }
        assert!(
            crate::verify_icrc3_withdrawal(&unknown, &r, 123, Principal::from_slice(&[8])).is_err()
        );
        let mut oversized = block;
        if let Icrc3ValueV2::Map(root) = &mut oversized {
            if let Some((_, Icrc3ValueV2::Map(tx))) = root.iter_mut().find(|(key, _)| key == "tx") {
                if let Some((_, value)) = tx.iter_mut().find(|(key, _)| key == "amt") {
                    *value =
                        Icrc3ValueV2::Nat(candid::Nat::from(u128::MAX) * candid::Nat::from(2u8));
                }
            }
        }
        assert!(
            crate::verify_icrc3_withdrawal(&oversized, &r, 123, Principal::from_slice(&[8]))
                .is_err()
        );
    }

    #[test]
    fn native_icp_legacy_transfer_variant_is_exact_and_unknown_variants_hold() {
        let r = withdrawal_record(Principal::from_slice(&[9]));
        let source_principal = Principal::from_slice(&[8]);
        let source = crate::native_account_identifier(source_principal).to_vec();
        let destination = crate::native_account_identifier(r.to).to_vec();
        let block = NativeIcpBlockV2 {
            parent_hash: None,
            transaction: crate::NativeIcpTransactionV2 {
                memo: 0,
                icrc1_memo: Some(b"stable memo".to_vec()),
                operation: Some(NativeIcpOperationV2::Transfer {
                    from: source,
                    to: destination,
                    spender: None,
                    amount: NativeIcpTokensV2 { e8s: r.send_amount },
                    fee: NativeIcpTokensV2 { e8s: r.fee },
                }),
                created_at_time: NativeIcpTimestampV2 {
                    timestamp_nanos: r.created_at_time,
                },
            },
            timestamp: NativeIcpTimestampV2 {
                timestamp_nanos: 999,
            },
        };
        assert!(crate::verify_native_icp_withdrawal(&block, &r, 123, source_principal).is_ok());
        let mut wrong = block.clone();
        if let Some(NativeIcpOperationV2::Transfer { amount, .. }) =
            &mut wrong.transaction.operation
        {
            amount.e8s += 1;
        }
        assert!(crate::verify_native_icp_withdrawal(&wrong, &r, 123, source_principal).is_err());
        let mut unknown = block;
        unknown.transaction.operation = Some(NativeIcpOperationV2::TransferFrom {
            from: vec![],
            to: vec![],
            spender: vec![],
            amount: NativeIcpTokensV2 { e8s: 1 },
            fee: NativeIcpTokensV2 { e8s: 1 },
        });
        assert!(crate::verify_native_icp_withdrawal(&unknown, &r, 123, source_principal).is_err());
    }

    #[test]
    fn too_old_pending_request_completes_only_after_exact_native_receipt_proof() {
        init_test_treasury();
        let request_id = 123;
        let record = withdrawal_record(Principal::from_slice(&[9]));
        crate::state::with_state_mut(|s| {
            s.add_deposit(DepositRecord {
                id: 0,
                deposit_type: DepositType::InterestRevenue,
                asset_type: AssetType::ICP,
                amount: 10_000,
                block_index: 500,
                timestamp: 1,
                memo: None,
            });
            assert!(matches!(
                s.begin_withdrawal(request_id, record.clone()).unwrap(),
                WithdrawalStart::Transfer(_)
            ));
            assert_eq!(
                s.mark_withdrawal_dispatch_attempt(request_id).unwrap(),
                Some(1)
            );
            // A TooOld result alone supplies no receipt and must leave the
            // already-debited reservation pending.
            assert_eq!(
                s.withdrawal_requests.get(&request_id).unwrap().status,
                WithdrawalRequestStatus::Pending
            );
            assert_eq!(s.balances[&AssetType::ICP].total, 8_000);
        });

        let source_principal = Principal::from_slice(&[8]);
        let block = NativeIcpBlockV2 {
            parent_hash: None,
            transaction: crate::NativeIcpTransactionV2 {
                memo: 0,
                icrc1_memo: Some(b"stable memo".to_vec()),
                operation: Some(NativeIcpOperationV2::Transfer {
                    from: crate::native_account_identifier(source_principal).to_vec(),
                    to: crate::native_account_identifier(record.to).to_vec(),
                    spender: None,
                    amount: NativeIcpTokensV2 {
                        e8s: record.send_amount,
                    },
                    fee: NativeIcpTokensV2 { e8s: record.fee },
                }),
                created_at_time: NativeIcpTimestampV2 {
                    timestamp_nanos: record.created_at_time,
                },
            },
            timestamp: NativeIcpTimestampV2 {
                timestamp_nanos: 999,
            },
        };
        assert!(
            crate::verify_native_icp_withdrawal(&block, &record, request_id, source_principal)
                .is_ok()
        );
        crate::state::with_state_mut(|s| {
            assert!(s.complete_withdrawal(request_id, 777).unwrap());
            assert!(!s.complete_withdrawal(request_id, 777).unwrap());
            assert_eq!(s.balances[&AssetType::ICP].total, 8_000);
        });
    }

    #[test]
    fn archived_history_ranges_are_exact_and_overflow_fails_closed() {
        assert_eq!(crate::icrc3_archive_covers(50, 50, 1), Ok(true));
        assert_eq!(crate::icrc3_archive_covers(51, 50, 1), Ok(false));
        assert!(crate::icrc3_archive_covers(u64::MAX, u64::MAX, 1).is_err());
        assert_eq!(crate::native_icp_archive_covers(50, 40, 20), Ok(true));
        assert_eq!(crate::native_icp_archive_covers(60, 40, 20), Ok(false));
        assert!(crate::native_icp_archive_covers(u64::MAX, u64::MAX, 1).is_err());
    }

    fn mock_principal() -> Principal {
        Principal::anonymous()
    }

    fn init_test_treasury() {
        let args = TreasuryInitArgs {
            controller: mock_principal(),
            icusd_ledger: mock_principal(),
            icp_ledger: mock_principal(),
            ckbtc_ledger: Some(mock_principal()),
            ckusdt_ledger: Some(mock_principal()),
            ckusdc_ledger: Some(mock_principal()),
        };
        crate::state::init_state(args);
    }

    fn withdrawal_record(to: Principal) -> WithdrawalRequestRecord {
        WithdrawalRequestRecord {
            caller: Principal::from_slice(&[7]),
            asset_type: AssetType::ICP,
            ledger: Principal::from_slice(&[8]),
            amount: 2_000,
            to,
            memo: Some("stable memo".into()),
            created_at_time: 100,
            send_amount: 1_900,
            fee: 100,
            status: WithdrawalRequestStatus::Pending,
            dispatch_attempts: Some(0),
        }
    }

    #[test]
    fn withdrawal_request_retries_debit_once_and_completed_retry_returns_same_receipt() {
        init_test_treasury();
        crate::state::with_state_mut(|s| {
            s.add_deposit(DepositRecord {
                id: 0,
                deposit_type: DepositType::InterestRevenue,
                asset_type: AssetType::ICP,
                amount: 10_000,
                block_index: 501,
                timestamp: 1,
                memo: None,
            });
            assert!(matches!(
                s.begin_withdrawal(900, withdrawal_record(Principal::from_slice(&[9])))
                    .unwrap(),
                WithdrawalStart::Transfer(_)
            ));
            let mut retry = withdrawal_record(Principal::from_slice(&[9]));
            retry.created_at_time = 999;
            retry.send_amount = 0;
            retry.fee = 0;
            let persisted = match s.begin_withdrawal(900, retry).unwrap() {
                WithdrawalStart::Retry(record) => record,
                other => panic!("expected exact retry to reuse pending row: {other:?}"),
            };
            assert_eq!(persisted.created_at_time, 100);
            assert_eq!(persisted.send_amount, 1_900);
            assert_eq!(s.balances[&AssetType::ICP].total, 8_000);
            assert!(s.complete_withdrawal(900, 777).unwrap());

            let complete = match s
                .begin_withdrawal(900, withdrawal_record(Principal::from_slice(&[9])))
                .unwrap()
            {
                WithdrawalStart::Complete {
                    block_index,
                    record,
                } => {
                    assert_eq!(record.send_amount, 1_900);
                    block_index
                }
                other => panic!("expected prior completed result: {other:?}"),
            };
            assert_eq!(complete, 777);
            assert_eq!(s.balances[&AssetType::ICP].total, 8_000);
        });
    }

    #[test]
    fn withdrawal_request_id_cannot_be_reused_for_a_different_tuple() {
        init_test_treasury();
        crate::state::with_state_mut(|s| {
            s.add_deposit(DepositRecord {
                id: 0,
                deposit_type: DepositType::InterestRevenue,
                asset_type: AssetType::ICP,
                amount: 10_000,
                block_index: 502,
                timestamp: 1,
                memo: None,
            });
            s.begin_withdrawal(901, withdrawal_record(Principal::from_slice(&[9])))
                .unwrap();
            let conflict = s.begin_withdrawal(901, withdrawal_record(Principal::from_slice(&[10])));
            assert!(conflict.is_err());
            assert_eq!(s.balances[&AssetType::ICP].total, 8_000);
        });
    }

    #[test]
    fn later_typed_rejection_keeps_prior_ambiguous_withdrawal_held() {
        init_test_treasury();
        crate::state::with_state_mut(|s| {
            s.add_deposit(DepositRecord {
                id: 0,
                deposit_type: DepositType::InterestRevenue,
                asset_type: AssetType::ICP,
                amount: 10_000,
                block_index: 508,
                timestamp: 1,
                memo: None,
            });
            s.begin_withdrawal(908, withdrawal_record(Principal::from_slice(&[9])))
                .unwrap();
            assert_eq!(s.mark_withdrawal_dispatch_attempt(908).unwrap(), Some(1));
            // A typed rejection on the proven first dispatch establishes that
            // no earlier attempt could have committed, so restoring is safe.
            s.abort_withdrawal(908).unwrap();
            assert_eq!(s.balances[&AssetType::ICP].total, 10_000);

            s.begin_withdrawal(908, withdrawal_record(Principal::from_slice(&[9])))
                .unwrap();
            assert_eq!(s.mark_withdrawal_dispatch_attempt(908).unwrap(), Some(1));
        });
        crate::state::restore_state();
        crate::state::with_state_mut(|s| {
            // Simulate a lost callback after possible commit, then a later
            // ledger rejection (including TooOld): it cannot prove absence of
            // the original transfer and must not release the reservation.
            assert_eq!(s.mark_withdrawal_dispatch_attempt(908).unwrap(), Some(2));
            assert!(s.abort_withdrawal(908).is_err());
            assert_eq!(s.balances[&AssetType::ICP].total, 8_000);
            assert_eq!(
                s.withdrawal_requests.get(&908).unwrap().dispatch_attempts,
                Some(2)
            );
        });
    }

    #[test]
    fn old_request_without_attempt_count_stays_held_after_typed_rejection() {
        init_test_treasury();
        crate::state::with_state_mut(|s| {
            s.add_deposit(DepositRecord {
                id: 0,
                deposit_type: DepositType::InterestRevenue,
                asset_type: AssetType::ICP,
                amount: 10_000,
                block_index: 510,
                timestamp: 1,
                memo: None,
            });
            s.begin_withdrawal(910, withdrawal_record(Principal::from_slice(&[9])))
                .unwrap();
            let mut record = s.withdrawal_requests.get(&910).unwrap();
            record.dispatch_attempts = None;
            s.withdrawal_requests.insert(910, record);
            assert_eq!(s.mark_withdrawal_dispatch_attempt(910).unwrap(), None);
            assert!(s.abort_withdrawal(910).is_err());
            assert_eq!(s.balances[&AssetType::ICP].total, 8_000);
            assert!(s.withdrawal_requests.get(&910).is_some());
        });
    }

    #[test]
    fn only_allowlisted_definitive_ledger_errors_can_release_first_attempt() {
        use icrc_ledger_types::icrc1::transfer::{NumTokens, TransferError};

        for error in [
            TransferError::BadFee {
                expected_fee: NumTokens::from(1u64),
            },
            TransferError::BadBurn {
                min_burn_amount: NumTokens::from(1u64),
            },
            TransferError::InsufficientFunds {
                balance: NumTokens::from(1u64),
            },
            TransferError::TooOld,
            TransferError::CreatedInFuture { ledger_time: 1 },
        ] {
            assert!(crate::is_definitive_no_effect_transfer_error(&error));
        }
        assert!(!crate::is_definitive_no_effect_transfer_error(
            &TransferError::TemporarilyUnavailable
        ));
        assert!(!crate::is_definitive_no_effect_transfer_error(
            &TransferError::GenericError {
                error_code: candid::Nat::from(1u64),
                message: "uncertain commit".into(),
            }
        ));
    }

    #[test]
    fn bad_fee_refresh_keeps_ambiguous_request_pinned_for_reconciliation() {
        use icrc_ledger_types::icrc1::transfer::TransferError;

        let ledger = Principal::from_slice(&[42]);
        crate::LEDGER_FEES.with(|fees| {
            fees.borrow_mut().insert(ledger, 10_000);
        });
        let bad_fee = TransferError::BadFee {
            expected_fee: candid::Nat::from(25_000u64),
        };

        let TransferError::BadFee { expected_fee } = &bad_fee else {
            unreachable!();
        };
        crate::refresh_ledger_fee_from_bad_fee(ledger, Some(1), expected_fee);
        assert_eq!(
            crate::LEDGER_FEES.with(|fees| fees.borrow().get(&ledger).copied()),
            Some(25_000)
        );

        // A later BadFee can refresh future request IDs only. The ambiguous
        // request's ledger tuple stays pinned and its reservation remains held.
        init_test_treasury();
        crate::state::with_state_mut(|s| {
            s.add_deposit(DepositRecord {
                id: 0,
                deposit_type: DepositType::InterestRevenue,
                asset_type: AssetType::ICP,
                amount: 10_000,
                block_index: 902,
                timestamp: 1,
                memo: None,
            });
            s.begin_withdrawal(777, withdrawal_record(Principal::from_slice(&[9])))
                .unwrap();
            assert_eq!(s.mark_withdrawal_dispatch_attempt(777).unwrap(), Some(1));
            assert_eq!(s.mark_withdrawal_dispatch_attempt(777).unwrap(), Some(2));
        });
        let before = crate::state::with_state(|s| s.withdrawal_requests.get(&777).unwrap());
        crate::refresh_ledger_fee_from_bad_fee(ledger, Some(2), expected_fee);
        assert_eq!(
            crate::LEDGER_FEES.with(|fees| fees.borrow().get(&ledger).copied()),
            None
        );
        let after = crate::state::with_state(|s| s.withdrawal_requests.get(&777).unwrap());
        assert_eq!(before.status, after.status);
        assert_eq!(before.dispatch_attempts, after.dispatch_attempts);
        assert_eq!(before.ledger, after.ledger);
        assert_eq!(before.created_at_time, after.created_at_time);
        assert_eq!(before.send_amount, after.send_amount);
        assert_eq!(before.fee, after.fee);
        assert_eq!(before.memo, after.memo);
        assert_eq!(before.to, after.to);
        assert_eq!(before.amount, after.amount);

        // A cache miss allows the next ID to use a fresh query result.
        assert_eq!(
            crate::remember_queried_ledger_fee(ledger, Some(25_000)),
            25_000
        );
    }

    #[test]
    fn fee_query_fallback_is_not_cached_and_a_later_success_recovers() {
        let ledger = Principal::from_slice(&[43]);
        assert_eq!(crate::remember_queried_ledger_fee(ledger, None), 10_000);
        assert_eq!(
            crate::LEDGER_FEES.with(|fees| fees.borrow().get(&ledger).copied()),
            None
        );

        assert_eq!(
            crate::remember_queried_ledger_fee(ledger, Some(17_500)),
            17_500
        );
        assert_eq!(
            crate::LEDGER_FEES.with(|fees| fees.borrow().get(&ledger).copied()),
            Some(17_500)
        );
    }

    #[test]
    fn oversized_ledger_duplicate_index_is_ambiguous_never_block_zero() {
        let oversized = candid::Nat::from(u64::MAX) + candid::Nat::from(1u8);
        let duplicate = crate::parse_duplicate_block(oversized);
        assert!(matches!(duplicate, Err(crate::LedgerError::Transport(_))));

        assert_eq!(
            crate::parse_duplicate_block(candid::Nat::from(42u64)).unwrap(),
            42
        );
    }

    #[test]
    fn oversized_success_index_maps_to_ambiguous_non_releasable_error() {
        let oversized = candid::Nat::from(u64::MAX) + candid::Nat::from(1u8);
        let success = crate::parse_success_block(oversized);
        let Err(crate::LedgerError::Ledger(error)) = success else {
            panic!("oversized success index must not become a successful zero block");
        };
        assert!(!crate::is_definitive_no_effect_transfer_error(&error));
    }

    #[test]
    fn pending_and_completed_withdrawal_requests_survive_state_restore() {
        init_test_treasury();
        crate::state::with_state_mut(|s| {
            s.add_deposit(DepositRecord {
                id: 0,
                deposit_type: DepositType::InterestRevenue,
                asset_type: AssetType::ICP,
                amount: 10_000,
                block_index: 509,
                timestamp: 1,
                memo: None,
            });
            s.begin_withdrawal(909, withdrawal_record(Principal::from_slice(&[9])))
                .unwrap();
        });

        crate::state::restore_state();
        crate::state::with_state_mut(|s| {
            let retry = s
                .begin_withdrawal(909, withdrawal_record(Principal::from_slice(&[9])))
                .unwrap();
            assert!(matches!(retry, WithdrawalStart::Retry(_)));
            assert_eq!(s.balances[&AssetType::ICP].total, 8_000);
            assert!(s.complete_withdrawal(909, 779).unwrap());
        });

        crate::state::restore_state();
        crate::state::with_state_mut(|s| {
            let replay = s
                .begin_withdrawal(909, withdrawal_record(Principal::from_slice(&[9])))
                .unwrap();
            assert!(matches!(
                replay,
                WithdrawalStart::Complete {
                    block_index: 779,
                    ..
                }
            ));
            assert_eq!(s.balances[&AssetType::ICP].total, 8_000);
        });
    }

    #[test]
    fn pending_withdrawal_inventory_pages_by_bounded_raw_request_keys() {
        init_test_treasury();
        crate::state::with_state_mut(|s| {
            s.add_deposit(DepositRecord {
                id: 0,
                deposit_type: DepositType::InterestRevenue,
                asset_type: AssetType::ICP,
                amount: 10_000,
                block_index: 511,
                timestamp: 1,
                memo: None,
            });
            s.begin_withdrawal(5, withdrawal_record(Principal::from_slice(&[9])))
                .unwrap();
            s.complete_withdrawal(5, 800).unwrap();
            s.begin_withdrawal(12, withdrawal_record(Principal::from_slice(&[10])))
                .unwrap();
            s.begin_withdrawal(u64::MAX, withdrawal_record(Principal::from_slice(&[11])))
                .unwrap();
        });

        crate::state::with_state(|s| {
            let first = s.get_pending_withdrawals_v2(Some(5), 1);
            assert!(first.withdrawals.is_empty());
            assert_eq!(first.next_start, Some(6));

            let second = s.get_pending_withdrawals_v2(first.next_start, 1);
            assert_eq!(second.withdrawals.len(), 1);
            assert_eq!(second.withdrawals[0].request_id, 12);
            assert_eq!(second.withdrawals[0].caller, Principal::from_slice(&[7]));
            assert_eq!(second.withdrawals[0].ledger, Principal::from_slice(&[8]));
            assert_eq!(second.withdrawals[0].amount, 2_000);
            assert_eq!(second.withdrawals[0].to, Principal::from_slice(&[10]));
            assert_eq!(second.withdrawals[0].created_at_time, 100);
            assert_eq!(second.withdrawals[0].dispatch_attempts, Some(0));
            assert_eq!(second.withdrawals[0].status, "pending");
            assert_eq!(second.next_start, Some(13));

            let terminal = s.get_pending_withdrawals_v2(second.next_start, 1);
            assert_eq!(terminal.withdrawals[0].request_id, u64::MAX);
            assert_eq!(terminal.next_start, None);
        });
    }

    #[test]
    fn old_timestamp_only_withdrawal_ids_are_quarantined_on_upgrade() {
        init_test_treasury();
        crate::state::with_state_mut(|s| {
            s.add_deposit(DepositRecord {
                id: 0,
                deposit_type: DepositType::InterestRevenue,
                asset_type: AssetType::ICP,
                amount: 10_000,
                block_index: 503,
                timestamp: 1,
                memo: None,
            });
            s.withdrawal_created_at.insert(902, 99);
        });
        crate::state::reset_withdrawal_request_migration_for_legacy_test();
        crate::state::restore_state();
        crate::state::with_state_mut(|s| {
            let retry = s.begin_withdrawal(902, withdrawal_record(Principal::from_slice(&[9])));
            assert!(retry.is_err());
            assert_eq!(s.balances[&AssetType::ICP].total, 10_000);
            assert!(matches!(
                s.withdrawal_requests.get(&902).unwrap().status,
                WithdrawalRequestStatus::LegacyUnknown
            ));
        });
    }

    #[test]
    fn init_rejects_asset_ledger_aliases() {
        let args = TreasuryInitArgs {
            controller: Principal::from_slice(&[1]),
            icusd_ledger: Principal::from_slice(&[2]),
            icp_ledger: Principal::from_slice(&[3]),
            ckbtc_ledger: Some(Principal::from_slice(&[6])),
            ckusdt_ledger: Some(Principal::from_slice(&[5])),
            ckusdc_ledger: Some(Principal::from_slice(&[3])),
        };
        assert!(crate::validate_distinct_ledgers(&args).is_err());
    }

    #[test]
    fn restored_configuration_holds_withdrawals_for_any_asset_ledger_alias() {
        let config = crate::state::TreasuryConfig {
            icusd_ledger: Principal::from_slice(&[2]),
            icp_ledger: Principal::from_slice(&[2]),
            ckbtc_ledger: Some(Principal::from_slice(&[6])),
            ckusdt_ledger: None,
            ckusdc_ledger: None,
            is_paused: false,
            stability_pool_reporter: None,
        };
        assert!(crate::validate_configured_asset_ledger(&config, &AssetType::ICUSD).is_err());
        assert!(crate::validate_configured_asset_ledger(&config, &AssetType::ICP).is_err());
        assert!(crate::validate_configured_asset_ledger(&config, &AssetType::CKBTC).is_ok());
    }

    #[test]
    fn test_treasury_initialization() {
        init_test_treasury();

        let status = crate::state::with_state(|s| {
            let config = s.get_config();
            let balances = s
                .balances
                .iter()
                .filter_map(|(asset_type, balance)| {
                    AssetTypeV1::try_from(asset_type.clone())
                        .ok()
                        .map(|asset_type| (asset_type, balance.clone()))
                })
                .collect();

            TreasuryStatus {
                total_deposits: s.get_deposits_count(),
                balances,
                controller: config.icusd_ledger, // just use any principal for display
                is_paused: config.is_paused,
            }
        });

        assert!(!status.is_paused);
        assert_eq!(status.total_deposits, 0);
        assert_eq!(status.balances.len(), 5); // ICUSD, ICP, CKBTC, CKUSDT, CKUSDC
    }

    #[test]
    fn test_deposit_functionality() {
        init_test_treasury();

        let deposit_record = DepositRecord {
            id: 0, // Will be set by add_deposit
            deposit_type: DepositType::BorrowingFee,
            asset_type: AssetType::ICUSD,
            amount: 1_000_000, // 0.01 icUSD in e8s
            block_index: 12345,
            timestamp: 1234567890,
            memo: Some("Test minting fee".to_string()),
        };

        let deposit_id = crate::state::with_state_mut(|s| s.add_deposit(deposit_record));

        assert_eq!(deposit_id, 1);

        // Check balance was updated
        let balance =
            crate::state::with_state(|s| s.balances.get(&AssetType::ICUSD).unwrap().clone());

        assert_eq!(balance.total, 1_000_000);
        assert_eq!(balance.available, 1_000_000);
        assert_eq!(balance.reserved, 0);
    }

    #[test]
    fn deposit_receipt_retry_is_idempotent() {
        init_test_treasury();
        let record = DepositRecord {
            id: 0,
            deposit_type: DepositType::BorrowingFee,
            asset_type: AssetType::ICUSD,
            amount: 1_000,
            block_index: 44,
            timestamp: 10,
            memo: None,
        };

        let (first_id, first_new) =
            crate::state::with_state_mut(|s| s.add_deposit_once(record.clone()).unwrap());
        let (retry_id, retry_new) =
            crate::state::with_state_mut(|s| s.add_deposit_once(record).unwrap());

        assert!(first_new);
        assert_eq!(retry_id, first_id);
        assert!(!retry_new);
        crate::state::with_state(|s| {
            assert_eq!(s.balances[&AssetType::ICUSD].total, 1_000);
            assert_eq!(s.get_deposits_count(), 1);
        });
    }

    #[test]
    fn other_ledger_deposits_are_distinct_withdrawable_assets() {
        init_test_treasury();
        let first_ledger = Principal::from_slice(&[31]);
        let second_ledger = Principal::from_slice(&[32]);
        let make_record = |ledger| DepositRecord {
            id: 0,
            deposit_type: DepositType::LiquidationFee,
            asset_type: AssetType::Other(ledger),
            amount: 2_000,
            block_index: 77,
            timestamp: 10,
            memo: None,
        };

        let (_, first_new) = crate::state::with_state_mut(|s| {
            s.add_deposit_once(make_record(first_ledger)).unwrap()
        });
        let (_, second_new) = crate::state::with_state_mut(|s| {
            s.add_deposit_once(make_record(second_ledger)).unwrap()
        });
        assert!(first_new && second_new);
        crate::state::with_state_mut(|s| {
            for ledger in [first_ledger, second_ledger] {
                let id = s.next_event_id;
                s.events.insert(
                    id,
                    TreasuryEvent {
                        id,
                        timestamp: 10,
                        caller: Principal::from_slice(&[9]),
                        action: TreasuryAction::Deposit {
                            deposit_type: DepositType::LiquidationFee,
                            asset_type: AssetType::Other(ledger),
                            amount: 2_000,
                        },
                    },
                );
                s.next_event_id += 1;
            }
        });

        let first_asset = AssetType::Other(first_ledger);
        let second_asset = AssetType::Other(second_ledger);
        crate::state::with_state(|s| {
            assert_eq!(s.balances[&first_asset].available, 2_000);
            assert_eq!(s.balances[&second_asset].available, 2_000);
        });
        assert!(crate::get_deposits(None, None).is_empty());
        assert_eq!(crate::get_deposits_v2(None, None).len(), 2);
        let all_balances = crate::state::with_state(|s| s.balances.clone());
        let legacy_balances = crate::legacy_asset_balances(&all_balances);
        assert_eq!(legacy_balances.len(), 5);
        assert!(legacy_balances
            .iter()
            .any(|(asset, _)| matches!(asset, AssetTypeV1::ICP)));
        assert!(all_balances.contains_key(&first_asset));
        assert!(crate::get_events(None, None).is_empty());
        assert_eq!(crate::get_events_v2(None, None).len(), 2);

        let config = crate::state::with_state(|s| s.get_config());
        assert_eq!(
            crate::configured_asset_ledger(&config, &first_asset),
            Some(first_ledger)
        );
        assert!(crate::validate_configured_asset_ledger(&config, &first_asset).is_ok());
        crate::state::with_state_mut(|s| s.withdraw(first_asset.clone(), 1_000).unwrap());
        crate::state::with_state(|s| {
            assert_eq!(s.balances[&first_asset].available, 1_000);
            assert_eq!(s.balances[&second_asset].available, 2_000);
        });
    }

    #[test]
    fn legacy_identical_duplicate_receipts_are_ambiguous_and_hold_withdrawals_after_restore() {
        init_test_treasury();
        let legacy = DepositRecord {
            id: 0,
            deposit_type: DepositType::BorrowingFee,
            asset_type: AssetType::ICUSD,
            amount: 1_000,
            block_index: 47,
            timestamp: 10,
            memo: None,
        };
        crate::state::with_state_mut(|s| {
            s.add_deposit(legacy.clone());
            s.add_deposit(legacy.clone());
        });

        // Model stable state written by the version before receipt indexing.
        crate::state::clear_receipt_index_for_legacy_test();
        crate::state::restore_state();

        let retry = crate::state::with_state_mut(|s| s.add_deposit_once(legacy));
        assert!(
            retry.is_err(),
            "duplicate legacy block must not look replay-safe"
        );
        let withdrawal = crate::state::with_state_mut(|s| s.withdraw(AssetType::ICUSD, 1));
        assert!(
            withdrawal.is_err(),
            "ambiguous historical credit must not be spendable"
        );
        let mut new_request = withdrawal_record(Principal::from_slice(&[9]));
        new_request.asset_type = AssetType::ICUSD;
        new_request.ledger = Principal::from_slice(&[2]);
        new_request.amount = 1;
        new_request.send_amount = 1;
        new_request.fee = 0;
        let new_withdrawal = crate::state::with_state_mut(|s| s.begin_withdrawal(999, new_request));
        assert!(
            new_withdrawal.is_err(),
            "public-path reservation must honor migrated duplicate receipt holds"
        );
        crate::state::with_state(|s| {
            assert_eq!(s.get_deposits_count(), 2, "migration preserves the old log");
            assert_eq!(s.balances[&AssetType::ICUSD].total, 2_000);
            assert_eq!(s.balances[&AssetType::ICUSD].available, 2_000);
        });
    }

    #[test]
    fn deposit_receipt_rejects_conflicting_amount() {
        init_test_treasury();
        let record = DepositRecord {
            id: 0,
            deposit_type: DepositType::BorrowingFee,
            asset_type: AssetType::ICUSD,
            amount: 1_000,
            block_index: 45,
            timestamp: 10,
            memo: None,
        };
        crate::state::with_state_mut(|s| s.add_deposit_once(record.clone()).unwrap());

        let mut conflicting = record;
        conflicting.amount = 1_001;
        let result = crate::state::with_state_mut(|s| s.add_deposit_once(conflicting));

        assert!(result.is_err());
        crate::state::with_state(|s| {
            assert_eq!(s.balances[&AssetType::ICUSD].total, 1_000);
            assert_eq!(s.get_deposits_count(), 1);
        });
    }

    #[test]
    fn deposit_receipt_rejects_conflicting_type_or_memo() {
        init_test_treasury();
        let record = DepositRecord {
            id: 0,
            deposit_type: DepositType::BorrowingFee,
            asset_type: AssetType::ICUSD,
            amount: 1_000,
            block_index: 46,
            timestamp: 10,
            memo: Some("fee".to_string()),
        };
        crate::state::with_state_mut(|s| s.add_deposit_once(record.clone()).unwrap());

        let mut changed_type = record.clone();
        changed_type.deposit_type = DepositType::RedemptionFee;
        let type_result = crate::state::with_state_mut(|s| s.add_deposit_once(changed_type));

        let mut changed_memo = record;
        changed_memo.memo = Some("different fee".to_string());
        let memo_result = crate::state::with_state_mut(|s| s.add_deposit_once(changed_memo));

        assert!(type_result.is_err());
        assert!(memo_result.is_err());
        crate::state::with_state(|s| {
            assert_eq!(s.balances[&AssetType::ICUSD].total, 1_000);
            assert_eq!(s.get_deposits_count(), 1);
        });
    }

    #[test]
    fn stability_pool_unallocated_interest_is_recorded_once_per_source_receipt() {
        init_test_treasury();
        let (deposit_id, new) = crate::state::with_state_mut(|s| {
            s.record_sp_unallocated_interest_once_at(900, 77, &[31, 32], 1)
                .expect("first report records")
        });
        assert!(new);
        let (duplicate_id, duplicate_new) = crate::state::with_state_mut(|s| {
            s.record_sp_unallocated_interest_once_at(900, 77, &[31, 32], 2)
                .expect("retry is idempotent")
        });
        assert_eq!(duplicate_id, deposit_id);
        assert!(!duplicate_new);
        let balance = crate::state::with_state(|s| s.balances[&AssetType::ICUSD].clone());
        assert_eq!(balance.total, 900);
        assert_eq!(
            crate::state::with_state(|s| s.get_deposits_count()),
            1,
            "a duplicate report must not add treasury balance or a second log"
        );
    }

    #[test]
    fn stability_pool_unallocated_interest_rejects_partial_replay() {
        init_test_treasury();
        crate::state::with_state_mut(|s| {
            s.record_sp_unallocated_interest_once_at(900, 77, &[31, 32], 1)
                .expect("first report records")
        });
        let result = crate::state::with_state_mut(|s| {
            s.record_sp_unallocated_interest_once_at(1_000, 78, &[32, 33], 2)
        });
        assert!(
            result.is_err(),
            "partial replay cannot create a new deposit"
        );
    }

    #[test]
    fn test_withdraw_functionality() {
        init_test_treasury();

        // First add some balance
        let deposit_record = DepositRecord {
            id: 0,
            deposit_type: DepositType::LiquidationFee,
            asset_type: AssetType::ICP,
            amount: 5_000_000, // 0.05 ICP in e8s
            block_index: 54321,
            timestamp: 1234567890,
            memo: None,
        };

        crate::state::with_state_mut(|s| s.add_deposit(deposit_record));

        // Now try to withdraw less than available
        let result = crate::state::with_state_mut(|s| s.withdraw(AssetType::ICP, 2_000_000));

        assert!(result.is_ok());

        // Check remaining balance
        let balance =
            crate::state::with_state(|s| s.balances.get(&AssetType::ICP).unwrap().clone());

        assert_eq!(balance.total, 3_000_000);
        assert_eq!(balance.available, 3_000_000);
    }

    #[test]
    fn test_withdraw_insufficient_funds() {
        init_test_treasury();

        // Try to withdraw from empty treasury
        let result = crate::state::with_state_mut(|s| s.withdraw(AssetType::CKBTC, 1_000_000));

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Insufficient balance"));
    }

    #[test]
    fn test_restore_balance_after_failed_transfer() {
        init_test_treasury();

        // Add some balance
        let deposit_record = DepositRecord {
            id: 0,
            deposit_type: DepositType::InterestRevenue,
            asset_type: AssetType::ICUSD,
            amount: 10_000_000,
            block_index: 1,
            timestamp: 1000,
            memo: None,
        };
        crate::state::with_state_mut(|s| s.add_deposit(deposit_record));

        // Withdraw (simulating pre-transfer deduction)
        crate::state::with_state_mut(|s| s.withdraw(AssetType::ICUSD, 3_000_000)).unwrap();

        let balance_after_withdraw =
            crate::state::with_state(|s| s.balances.get(&AssetType::ICUSD).unwrap().clone());
        assert_eq!(balance_after_withdraw.available, 7_000_000);

        // Simulate transfer failure → restore
        crate::state::with_state_mut(|s| s.restore_balance(&AssetType::ICUSD, 3_000_000));

        let balance_after_restore =
            crate::state::with_state(|s| s.balances.get(&AssetType::ICUSD).unwrap().clone());
        assert_eq!(balance_after_restore.available, 10_000_000);
        assert_eq!(balance_after_restore.total, 10_000_000);
    }

    #[test]
    fn test_pause_functionality() {
        init_test_treasury();

        // Pause treasury
        let result = crate::state::with_state_mut(|s| s.set_paused(true));
        assert!(result.is_ok());

        let config = crate::state::with_state(|s| s.get_config());
        assert!(config.is_paused);

        // Unpause treasury
        let result = crate::state::with_state_mut(|s| s.set_paused(false));
        assert!(result.is_ok());

        let config = crate::state::with_state(|s| s.get_config());
        assert!(!config.is_paused);
    }

    #[test]
    fn test_deposit_history() {
        init_test_treasury();

        // Add multiple deposits
        let deposits = vec![
            DepositRecord {
                id: 0,
                deposit_type: DepositType::BorrowingFee,
                asset_type: AssetType::ICUSD,
                amount: 1_000_000,
                block_index: 1,
                timestamp: 1000,
                memo: Some("First deposit".to_string()),
            },
            DepositRecord {
                id: 0,
                deposit_type: DepositType::RedemptionFee,
                asset_type: AssetType::ICP,
                amount: 2_000_000,
                block_index: 2,
                timestamp: 2000,
                memo: Some("Second deposit".to_string()),
            },
        ];

        for deposit in deposits {
            crate::state::with_state_mut(|s| s.add_deposit(deposit));
        }

        // Get deposit history
        let history = crate::state::with_state(|s| s.get_deposits(None, 10));

        assert_eq!(history.len(), 2);
        assert_eq!(history[0].id, 1);
        assert_eq!(history[1].id, 2);
        assert_eq!(
            history[0].deposit_type,
            crate::types::DepositTypeV1::BorrowingFee
        );
        assert_eq!(
            history[1].deposit_type,
            crate::types::DepositTypeV1::RedemptionFee
        );
    }

    #[test]
    fn test_icrc_002_tracked_balance_matches_onchain_debit() {
        init_test_treasury();

        let deposit = DepositRecord {
            id: 0,
            deposit_type: DepositType::LiquidationFee,
            asset_type: AssetType::ICP,
            amount: 10_000_000,
            block_index: 1,
            timestamp: 1000,
            memo: None,
        };
        crate::state::with_state_mut(|s| s.add_deposit(deposit));

        // Withdraw flow: bookkeeping is debited `amount`, the wire carries
        // `amount - fee`, and the ledger debits `sent + fee` from the account.
        let amount = 2_000_000u64;
        let fee = 10_000u64;
        crate::state::with_state_mut(|s| s.withdraw(AssetType::ICP, amount)).unwrap();
        let sent = crate::withdrawal_send_amount(amount, fee).unwrap();

        let balance =
            crate::state::with_state(|s| s.balances.get(&AssetType::ICP).unwrap().clone());
        let tracked_drop = 10_000_000 - balance.total;
        let onchain_drop = sent + fee;

        assert_eq!(sent, 1_990_000);
        assert_eq!(tracked_drop, amount);
        // The whole finding: what leaves the canister account must equal
        // what leaves the books, with no per-withdrawal fee drift.
        assert_eq!(tracked_drop, onchain_drop);
    }

    #[test]
    fn test_icrc_002_withdraw_rejects_amount_not_exceeding_fee() {
        // Nothing transferable once the fee is covered, so the withdrawal is
        // rejected before any bookkeeping debit (no drift in either direction).
        assert!(crate::withdrawal_send_amount(10_000, 10_000).is_err());
        assert!(crate::withdrawal_send_amount(9_999, 10_000).is_err());
        assert!(crate::withdrawal_send_amount(0, 10_000).is_err());
        assert_eq!(crate::withdrawal_send_amount(10_001, 10_000), Ok(1));
    }

    #[test]
    fn test_icrc_003_created_at_time_persisted_and_reused() {
        init_test_treasury();

        let t1 = 1_700_000_000_000_000_000u64;
        let first = crate::state::with_state_mut(|s| s.created_at_time_for_request(42, t1));
        assert_eq!(first, t1);

        // A retry minutes later must reuse the first attempt's timestamp so
        // the ledger's dedup window can catch a re-submitted transfer.
        let t2 = t1 + 5 * 60 * 1_000_000_000;
        let retried = crate::state::with_state_mut(|s| s.created_at_time_for_request(42, t2));
        assert_eq!(retried, t1);

        // A different request gets its own timestamp.
        let other = crate::state::with_state_mut(|s| s.created_at_time_for_request(43, t2));
        assert_eq!(other, t2);

        // After a TooOld/CreatedInFuture rejection the entry is cleared and
        // the next attempt gets a fresh timestamp.
        crate::state::with_state_mut(|s| s.clear_request_created_at(42));
        let t3 = t2 + 1_000_000_000;
        let fresh = crate::state::with_state_mut(|s| s.created_at_time_for_request(42, t3));
        assert_eq!(fresh, t3);
    }

    #[test]
    fn test_icrc_003_expired_created_at_time_replaced_and_pruned() {
        init_test_treasury();

        let t1 = 1_700_000_000_000_000_000u64;
        crate::state::with_state_mut(|s| {
            s.created_at_time_for_request(7, t1);
            s.created_at_time_for_request(8, t1);
        });

        // Past the 24h ledger dedup window the original transaction can no
        // longer dedup, so the request gets a fresh timestamp and stale
        // entries are pruned.
        let later = t1 + 24 * 60 * 60 * 1_000_000_000 + 1;
        let replaced = crate::state::with_state_mut(|s| s.created_at_time_for_request(7, later));
        assert_eq!(replaced, later);

        let pruned = crate::state::with_state(|s| s.withdrawal_created_at.get(&8));
        assert_eq!(pruned, None);
    }

    #[test]
    fn test_balances_persisted_in_stable_cell() {
        init_test_treasury();

        // Add a deposit
        let deposit = DepositRecord {
            id: 0,
            deposit_type: DepositType::BorrowingFee,
            asset_type: AssetType::ICP,
            amount: 5_000_000,
            block_index: 1,
            timestamp: 1000,
            memo: None,
        };
        crate::state::with_state_mut(|s| s.add_deposit(deposit));

        // Verify the StableCell snapshot matches in-memory balances
        let (in_memory, snapshot) = crate::state::with_state(|s| {
            let mem = s.balances.get(&AssetType::ICP).unwrap().clone();
            let snap = s.balances_cell.get().clone();
            (mem, snap)
        });

        assert_eq!(in_memory.total, 5_000_000);
        // Find ICP in snapshot
        let icp_snap = snapshot
            .entries
            .iter()
            .find(|(a, _)| *a == AssetType::ICP)
            .map(|(_, b)| b.clone())
            .unwrap();
        assert_eq!(icp_snap.total, 5_000_000);
        assert_eq!(icp_snap.available, 5_000_000);
    }
}
