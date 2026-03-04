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
            let balances = s.balances.iter()
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
        let balance = crate::state::with_state(|s|
            s.balances.get(&AssetType::ICUSD).unwrap().clone()
        );

        assert_eq!(balance.total, 1_000_000);
        assert_eq!(balance.available, 1_000_000);
        assert_eq!(balance.reserved, 0);
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
        let result = crate::state::with_state_mut(|s|
            s.withdraw(AssetType::ICP, 2_000_000)
        );

        assert!(result.is_ok());

        // Check remaining balance
        let balance = crate::state::with_state(|s|
            s.balances.get(&AssetType::ICP).unwrap().clone()
        );

        assert_eq!(balance.total, 3_000_000);
        assert_eq!(balance.available, 3_000_000);
    }

    #[test]
    fn test_withdraw_insufficient_funds() {
        init_test_treasury();

        // Try to withdraw from empty treasury
        let result = crate::state::with_state_mut(|s|
            s.withdraw(AssetType::CKBTC, 1_000_000)
        );

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
        crate::state::with_state_mut(|s|
            s.withdraw(AssetType::ICUSD, 3_000_000)
        ).unwrap();

        let balance_after_withdraw = crate::state::with_state(|s|
            s.balances.get(&AssetType::ICUSD).unwrap().clone()
        );
        assert_eq!(balance_after_withdraw.available, 7_000_000);

        // Simulate transfer failure → restore
        crate::state::with_state_mut(|s|
            s.restore_balance(&AssetType::ICUSD, 3_000_000)
        );

        let balance_after_restore = crate::state::with_state(|s|
            s.balances.get(&AssetType::ICUSD).unwrap().clone()
        );
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
        let icp_snap = snapshot.entries.iter()
            .find(|(a, _)| *a == AssetType::ICP)
            .map(|(_, b)| b.clone())
            .unwrap();
        assert_eq!(icp_snap.total, 5_000_000);
        assert_eq!(icp_snap.available, 5_000_000);
    }
}
