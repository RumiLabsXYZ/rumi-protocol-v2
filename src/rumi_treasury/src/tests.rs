#[cfg(test)]
mod tests {
    use crate::types::*;
    use candid::Principal;

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

    #[test]
    fn test_treasury_initialization() {
        init_test_treasury();

        let status = crate::state::with_state(|s| {
            let config = s.get_config();
            let balances = s
                .balances
                .iter()
                .map(|(asset_type, balance)| (asset_type.clone(), balance.clone()))
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
        assert_eq!(history[0].deposit_type, DepositType::BorrowingFee);
        assert_eq!(history[1].deposit_type, DepositType::RedemptionFee);
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

    fn deposit_record(
        deposit_type: DepositType,
        asset_type: AssetType,
        amount: u64,
        block_index: u64,
        memo: Option<&str>,
    ) -> DepositRecord {
        DepositRecord {
            id: 0,
            deposit_type,
            asset_type,
            amount,
            block_index,
            timestamp: 1_000,
            memo: memo.map(str::to_string),
        }
    }

    #[test]
    fn borrowing_fee_icusd_block_is_idempotent_only_for_exact_payload() {
        init_test_treasury();
        let first = crate::record_deposit_with_event(
            deposit_record(
                DepositType::BorrowingFee,
                AssetType::ICUSD,
                12_345,
                9,
                Some("borrow fee"),
            ),
            mock_principal(),
            100,
        )
        .unwrap();
        assert_eq!(first, 1);

        // Timestamp is generated by the receiver and therefore may differ on
        // retry; the canonical transfer payload is the block/type/asset,
        // amount and memo.
        let mut retry = deposit_record(
            DepositType::BorrowingFee,
            AssetType::ICUSD,
            12_345,
            9,
            Some("borrow fee"),
        );
        retry.timestamp = 2_000;
        let duplicate = crate::record_deposit_with_event(retry, mock_principal(), 200).unwrap();
        assert_eq!(duplicate, 1);

        let (count, balance, event_count) = crate::state::with_state(|s| {
            (
                s.get_deposits_count(),
                s.balances[&AssetType::ICUSD].total,
                s.get_events_count(),
            )
        });
        assert_eq!(count, 1);
        assert_eq!(balance, 12_345);
        assert_eq!(event_count, 1, "an exact retry must not emit another event");

        for conflicting in [
            deposit_record(
                DepositType::BorrowingFee,
                AssetType::ICUSD,
                12_346,
                9,
                Some("borrow fee"),
            ),
            deposit_record(
                DepositType::BorrowingFee,
                AssetType::ICUSD,
                12_345,
                9,
                Some("different memo"),
            ),
            deposit_record(
                DepositType::InterestRevenue,
                AssetType::ICUSD,
                12_345,
                9,
                Some("borrow fee"),
            ),
        ] {
            assert!(crate::record_deposit_with_event(conflicting, mock_principal(), 300).is_err());
        }
        crate::state::with_state(|s| {
            assert_eq!(s.get_deposits_count(), 1);
            assert_eq!(s.balances[&AssetType::ICUSD].total, 12_345);
            assert_eq!(s.get_events_count(), 1);
        });
    }

    #[test]
    fn fresh_treasury_accepts_real_block_zero_and_deduplicates_it() {
        init_test_treasury();

        let first = crate::record_deposit_with_event(
            deposit_record(
                DepositType::BorrowingFee,
                AssetType::ICUSD,
                77,
                0,
                Some("first ledger block"),
            ),
            mock_principal(),
            100,
        )
        .unwrap();
        assert_eq!(first, 1, "zero is the block key, not a deposit ID");

        let retry = crate::record_deposit_with_event(
            deposit_record(
                DepositType::BorrowingFee,
                AssetType::ICUSD,
                77,
                0,
                Some("first ledger block"),
            ),
            mock_principal(),
            200,
        )
        .unwrap();
        assert_eq!(retry, first);

        crate::state::with_state(|s| {
            assert_eq!(s.icusd_deposit_blocks.get(&0), Some(first));
            assert_eq!(s.get_deposits_count(), 1);
            assert_eq!(s.balances[&AssetType::ICUSD].total, 77);
            assert_eq!(s.get_events_count(), 1);
        });
    }

    #[test]
    fn rust_candid_service_exports_the_bounded_backfill_endpoint() {
        let service = crate::__export_service();
        assert!(service.contains("continue_icusd_deposit_block_backfill"));
        assert!(service.contains("deposit_borrowing_fee_once"));
        assert!(service.contains("DepositArgs"));
    }

    #[test]
    fn borrowing_fee_endpoint_rejects_non_fee_or_non_icusd_payloads() {
        let fee_icusd = crate::types::DepositArgs {
            deposit_type: DepositType::BorrowingFee,
            asset_type: AssetType::ICUSD,
            amount: 1,
            block_index: 1,
            memo: None,
        };
        assert!(crate::validate_borrowing_fee_args(&fee_icusd).is_ok());

        let interest_icusd = crate::types::DepositArgs {
            deposit_type: DepositType::InterestRevenue,
            ..fee_icusd.clone()
        };
        assert!(crate::validate_borrowing_fee_args(&interest_icusd).is_err());

        let fee_icp = crate::types::DepositArgs {
            asset_type: AssetType::ICP,
            ..fee_icusd
        };
        assert!(crate::validate_borrowing_fee_args(&fee_icp).is_err());
    }

    #[test]
    fn icusd_block_backfill_is_bounded_resumable_and_holds_ingress_until_complete() {
        init_test_treasury();

        // Seed old-format deposits without the new shared index, including
        // cross-type blocks, a duplicated block, and the legacy-zero sentinel.
        crate::state::with_state_mut(|s| {
            s.add_deposit(deposit_record(
                DepositType::BorrowingFee,
                AssetType::ICUSD,
                100,
                10,
                Some("old fee"),
            ));
            s.add_deposit(deposit_record(
                DepositType::InterestRevenue,
                AssetType::ICUSD,
                200,
                20,
                Some("stability-pool unallocated interest"),
            ));
            s.add_deposit(deposit_record(
                DepositType::LiquidationFee,
                AssetType::ICUSD,
                300,
                30,
                None,
            ));
            s.add_deposit(deposit_record(
                DepositType::BorrowingFee,
                AssetType::ICUSD,
                400,
                40,
                Some("duplicate one"),
            ));
            s.add_deposit(deposit_record(
                DepositType::BorrowingFee,
                AssetType::ICUSD,
                401,
                40,
                Some("duplicate two"),
            ));
            s.add_deposit(deposit_record(
                DepositType::BorrowingFee,
                AssetType::ICUSD,
                500,
                0,
                Some("legacy zero"),
            ));
            for block in 6..=205 {
                s.add_deposit(deposit_record(
                    DepositType::InterestRevenue,
                    AssetType::ICP,
                    1,
                    block,
                    None,
                ));
            }
            s.icusd_deposit_block_backfill
                .set(crate::state::IcusdDepositBlockBackfill::Uninitialized)
                .unwrap();
            s.icusd_deposit_indexed_through.set(0).unwrap();
        });

        // Reopening stable state emulates the post-upgrade path from a version
        // that had deposits but no receipt index or migration cell.
        crate::state::restore_state();
        assert!(
            crate::state::with_state_mut(|s| s.record_icusd_deposit_once(deposit_record(
                DepositType::BorrowingFee,
                AssetType::ICUSD,
                100,
                10,
                Some("old fee"),
            )))
            .is_err()
        );
        assert!(crate::state::with_state_mut(|s| {
            s.record_sp_unallocated_interest_once_at(10, 300, &[900], 2_000)
        })
        .is_err());

        let (processed, next_id, complete) =
            crate::state::with_state_mut(|s| s.continue_icusd_deposit_block_backfill().unwrap());
        assert_eq!(
            processed,
            crate::state::ICUSD_BLOCK_BACKFILL_BATCH_SIZE as u64
        );
        assert_eq!(
            next_id,
            crate::state::ICUSD_BLOCK_BACKFILL_BATCH_SIZE as u64 + 1
        );
        assert!(!complete);

        // The cursor and partial index survive another restore. No second
        // batch can exceed the fixed work bound.
        crate::state::restore_state();
        let mut batch_count = 1;
        let mut total_processed = crate::state::ICUSD_BLOCK_BACKFILL_BATCH_SIZE as u64;
        loop {
            let (processed, _, complete) = crate::state::with_state_mut(|s| {
                s.continue_icusd_deposit_block_backfill().unwrap()
            });
            assert!(processed <= crate::state::ICUSD_BLOCK_BACKFILL_BATCH_SIZE as u64);
            batch_count += 1;
            total_processed += processed;
            if complete {
                break;
            }
            assert!(
                processed > 0,
                "a non-complete batch must advance the cursor"
            );
            crate::state::restore_state();
        }
        assert_eq!(batch_count, 3);
        assert_eq!(total_processed, 206);

        let exact_retry = crate::state::with_state_mut(|s| {
            let mut retry = deposit_record(
                DepositType::BorrowingFee,
                AssetType::ICUSD,
                100,
                10,
                Some("old fee"),
            );
            retry.timestamp = 9_999;
            s.record_icusd_deposit_once(retry)
        })
        .unwrap();
        assert_eq!(exact_retry, (1, false));

        for block in [0, 40] {
            let result = crate::state::with_state_mut(|s| {
                s.record_icusd_deposit_once(deposit_record(
                    DepositType::BorrowingFee,
                    AssetType::ICUSD,
                    100,
                    block,
                    Some("old fee"),
                ))
            });
            assert!(result.is_err(), "legacy block {block} must be ambiguous");
        }

        // A physical block already used by another ICUSD deposit type cannot
        // become a BorrowingFee receipt, and the SP-specific path shares the
        // same physical block index for new records.
        let sp_collision = crate::state::with_state_mut(|s| {
            s.record_sp_unallocated_interest_once_at(999, 20, &[700], 3_000)
        });
        assert!(sp_collision.is_err());
        let (sp_id, is_new) = crate::state::with_state_mut(|s| {
            s.record_sp_unallocated_interest_once_at(999, 70, &[700], 3_000)
        })
        .unwrap();
        assert!(is_new);
        let (sp_retry_id, is_new) = crate::state::with_state_mut(|s| {
            s.record_sp_unallocated_interest_once_at(999, 70, &[700], 4_000)
        })
        .unwrap();
        assert_eq!(sp_retry_id, sp_id);
        assert!(!is_new);
        crate::state::with_state(|s| {
            assert_eq!(
                s.icusd_deposit_blocks.get(&70),
                Some(sp_id),
                "SP's physical transfer block must share the ICUSD index"
            );
            assert_eq!(s.get_deposits_count(), 207);
            assert_eq!(s.balances[&AssetType::ICUSD].total, 2_900);
        });
        assert!(crate::state::with_state_mut(|s| {
            s.record_icusd_deposit_once(deposit_record(
                DepositType::BorrowingFee,
                AssetType::ICUSD,
                999,
                70,
                Some("stability-pool unallocated interest"),
            ))
        })
        .is_err());
    }

    #[test]
    fn rollback_reupgrade_backfills_after_completed_and_in_progress_watermarks() {
        use crate::state::IcusdDepositBlockBackfill;

        init_test_treasury();
        let first_id = crate::state::with_state_mut(|s| {
            s.record_icusd_deposit_once(deposit_record(
                DepositType::BorrowingFee,
                AssetType::ICUSD,
                10,
                100,
                Some("indexed before downgrade"),
            ))
            .unwrap()
            .0
        });
        assert_eq!(first_id, 1);
        crate::state::with_state(|s| {
            assert_eq!(*s.icusd_deposit_indexed_through.get(), 1);
        });

        // Old code appends directly to the shared deposit map and knows
        // nothing about the new index or watermark.
        crate::state::with_state_mut(|s| {
            s.deposits.insert(
                2,
                deposit_record(
                    DepositType::BorrowingFee,
                    AssetType::ICUSD,
                    20,
                    200,
                    Some("written by downgraded Treasury"),
                ),
            );
        });
        crate::state::restore_state();
        crate::state::with_state(|s| {
            assert_eq!(
                s.icusd_deposit_block_backfill.get(),
                &IcusdDepositBlockBackfill::InProgress {
                    next_deposit_id: 2,
                    snapshot_max_id: 2,
                }
            );
        });
        assert!(crate::state::with_state_mut(|s| {
            s.record_icusd_deposit_once(deposit_record(
                DepositType::BorrowingFee,
                AssetType::ICUSD,
                20,
                200,
                Some("written by downgraded Treasury"),
            ))
        })
        .is_err());
        assert_eq!(
            crate::state::with_state_mut(|s| s.continue_icusd_deposit_block_backfill().unwrap()),
            (1, 3, true)
        );
        crate::state::with_state(|s| {
            assert_eq!(s.icusd_deposit_blocks.get(&200), Some(2));
            assert_eq!(*s.icusd_deposit_indexed_through.get(), 2);
        });

        // A rollback during an existing migration can add rows beyond its
        // saved snapshot. Re-upgrade must extend that snapshot before ingress
        // is released, while keeping the saved cursor.
        crate::state::with_state_mut(|s| {
            s.icusd_deposit_block_backfill
                .set(IcusdDepositBlockBackfill::InProgress {
                    next_deposit_id: 3,
                    snapshot_max_id: 3,
                })
                .unwrap();
            s.deposits.insert(
                3,
                deposit_record(DepositType::InterestRevenue, AssetType::ICP, 1, 0, None),
            );
        });
        crate::state::restore_state();
        crate::state::with_state_mut(|s| {
            s.deposits.insert(
                4,
                deposit_record(
                    DepositType::BorrowingFee,
                    AssetType::ICUSD,
                    40,
                    400,
                    Some("written during in-progress downgrade"),
                ),
            );
        });
        crate::state::restore_state();
        crate::state::with_state(|s| {
            assert_eq!(
                s.icusd_deposit_block_backfill.get(),
                &IcusdDepositBlockBackfill::InProgress {
                    next_deposit_id: 3,
                    snapshot_max_id: 4,
                }
            );
        });
        assert_eq!(
            crate::state::with_state_mut(|s| s.continue_icusd_deposit_block_backfill().unwrap()),
            (2, 5, true)
        );
        crate::state::with_state(|s| {
            assert_eq!(s.icusd_deposit_blocks.get(&400), Some(4));
            assert_eq!(*s.icusd_deposit_indexed_through.get(), 4);
        });
    }

    #[test]
    fn backfill_reports_incomplete_when_new_rows_extend_completed_snapshot() {
        use crate::state::IcusdDepositBlockBackfill;

        init_test_treasury();
        crate::state::with_state_mut(|s| {
            s.add_deposit(deposit_record(
                DepositType::BorrowingFee,
                AssetType::ICUSD,
                10,
                500,
                Some("backfill row"),
            ));
            s.icusd_deposit_block_backfill
                .set(IcusdDepositBlockBackfill::InProgress {
                    next_deposit_id: 1,
                    snapshot_max_id: 1,
                })
                .unwrap();
            // Non-ICUSD ingress remains available during migration and may
            // extend the deposit log after this batch's original snapshot.
            s.add_deposit(deposit_record(
                DepositType::InterestRevenue,
                AssetType::ICP,
                1,
                0,
                None,
            ));
        });

        let first_batch = crate::state::with_state_mut(|s| {
            s.continue_icusd_deposit_block_backfill().unwrap()
        });
        assert_eq!(first_batch, (1, 2, false));
        crate::state::with_state(|s| {
            assert_eq!(
                s.icusd_deposit_block_backfill.get(),
                &IcusdDepositBlockBackfill::InProgress {
                    next_deposit_id: 2,
                    snapshot_max_id: 2,
                }
            );
        });

        let final_batch = crate::state::with_state_mut(|s| {
            s.continue_icusd_deposit_block_backfill().unwrap()
        });
        assert_eq!(final_batch, (1, 3, true));
        crate::state::with_state(|s| {
            assert_eq!(
                s.icusd_deposit_block_backfill.get(),
                &IcusdDepositBlockBackfill::Complete
            );
            assert_eq!(*s.icusd_deposit_indexed_through.get(), 2);
        });
    }
}
