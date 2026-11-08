# Vault 158 execution receipt — 2026-11-07

Owner: Adrian. Rob directly approved the one execution call and subsequent readbacks described in the [pushed approval packet](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/433576e32d8ddbdb0a314c4b4fd0a421f1b38850/docs/coordination/vault158-execution-approval-2026-11-07.md), SHA-256 `d6c514cab74121504462d6f4f7a11ca268442ae4178ea476448bbc549db3fd4a`.

## Outcome

The single approved update returned `Ok`. Recovery is `TreasurySettled`; history record229 is `Completed`; backend and bot claim lists and payment journals are empty; treasury intent229 is null; processing remains paused. No second execution call, reconciliation mutation, unpause, deployment, config change, manual ledger transfer or unrelated liquidation was invoked. No unknown operation was retried.

## Preconditions

Immediately before execution the packet SHA matched; controller readback was Running/module `db8931273a83bb3dfbd8c516b85808b24ad5bd9daed0df9f7265b440658fe098`. Paused true, generation1 EvidencePrepared without payment timestamp/receipt, original PaymentShortfall claim158, no return attempts, no payment/treasury intent, backend active IDs `[158]`. Admin configuration matched backend tfesu-vyaaa-aaaap-qrd7a-cai, treasury tlg74-oiaaa-aaaap-qrd6a-cai, ICP ledger ryjl3-tyaaa-aaaaa-aaaba-cai and ckUSDC ledger xevnm-gaaaa-aaaar-qafnq-cai. Both live fees10000raw. Default balances ICP51412714e8 and ckUSDC31736304e6. Latest account transactions remained ICP39835236 and ckUSDC806697. ICP index/ledger both40158979 blocks; ckUSDC index/ledger both810573. Thus receipt attribution had no later account movement and up-to-date index evidence.

## Exact update command

```sh
icp canister call nygob-3qaaa-aaaap-qttcq-cai admin_execute_vault158_recovery '()' --network ic --identity rumi_identity --candid /private/tmp/v158-current-approved.did
```

Exit 0

```text
(variant { Ok })
```

The CLI provided no request ID. The retained recovery record, backend delta and exact ledger blocks below independently establish the outcome.

## Verified transfers and conservation

| Leg | Block | Recipient | Net | Fee | Gross debit |
| --- | --- | --- | ---: | ---: | ---: |
| ckUSDC | 810573 | Backend default tfesu-vyaaa-aaaap-qrd7a-cai | 5138419 e6 | 10000 e6 | 5148419 e6 |
| ICP | 40159014 | Treasury default tlg74-oiaaa-aaaap-qrd6a-cai | 10732902 e8 | 10000 e8 | 10742902 e8 |

ckUSDC memo is `RUMI-BPAY-V2:` plus big-endian u64 vault158/generation1; its saved created_at_time is `1794081122434828506`. ICP memo is `RUMI:TB1:` plus big-endian u64 record229/generation1; its created_at_time is `1794081133118479397`. Both senders are the bot default account. ckUSDC get_transactions fetched the exact block directly (no archive); ICP query_blocks fetched the exact block directly (no archive). Amounts, fees, identities, memos and timestamps match the installed recovery semantics. Returning a certificate in query_blocks is not a claim of independent offline certificate verification.

Final bot balances: ckUSDC26587885 e6 (26.190995 pre-swap baseline +0.396890 held surplus) and ICP40669812e8 (0.40669812 pre-claim funds). Equations:31736304-5148419=26587885;51412714-10742902=40669812. Neither shared baseline subsidized this recovery.

Vault158 before: collateral499899999e8 ICP, borrowed934763429e8 icUSD. After: collateral263116164e8 (2.63116164 ICP), borrowed420921614e8 (4.20921614 icUSD), accrued_interest0. Deltas are236783835e8 collateral and513841815e8 debt exactly. The claim is cleared. This is a completed partial liquidation, not a debt-free vault or a current health/price verdict.

## Recovery tombstone readback

```text
(
  opt record {
    status = variant { TreasurySettled };
    payment_memo = blob "\52\55\4d\49\2d\42\50\41\59\2d\56\32\3a\00\00\00\00\00\00\00\9e\00\00\00\00\00\00\00\01";
    backend_principal = principal "tfesu-vyaaa-aaaap-qrd7a-cai";
    swap_input_receipt = record {
      block_index = 39_835_236 : nat64;
      created_at_time = 1_793_306_981_079_625_901 : nat64;
      amount = 226_020_933 : nat64;
    };
    swap_output_amount_e6 = 5_545_309 : nat64;
    ckusdc_ledger_principal = principal "xevnm-gaaaa-aaaar-qafnq-cai";
    swap_output_receipt = record {
      block_index = 806_697 : nat64;
      created_at_time = 0 : nat64;
      amount = 5_545_309 : nat64;
    };
    icp_ledger_principal = principal "ryjl3-tyaaa-aaaaa-aaaba-cai";
    swap_input_fee_e8s = 10_000 : nat64;
    payment_amount_e6 = 5_138_419 : nat64;
    collateral_amount_e8s = 236_783_835 : nat64;
    swap_input_amount_e8s = 226_020_933 : nat64;
    payment_receipt = opt record {
      block_index = 810_573 : nat64;
      created_at_time = 1_794_081_122_434_828_506 : nat64;
      amount = 5_138_419 : nat64;
    };
    claim_generation = 1 : nat64;
    vault_id = 158 : nat64;
    payment_gross_e6 = 5_148_419 : nat64;
    held_surplus_e6 = 396_890 : nat64;
    pre_swap_ckusdc_balance_e6 = 26_190_995 : nat64;
    payment_fee_e6 = 10_000 : nat64;
    pool_principal = principal "mohjv-bqaaa-aaaag-qjyia-cai";
    debt_covered_e8s = 513_841_815 : nat64;
    record_id = 229 : nat64;
    payment_created_at_time = opt (1_794_081_122_434_828_506 : nat64);
    collateral_received_e8s = 236_773_835 : nat64;
  },
)
```

## Other authoritative readbacks

### get_processing_paused

Command:
```sh
icp canister call nygob-3qaaa-aaaap-qttcq-cai get_processing_paused '()' --query --network ic --identity rumi_identity --candid /private/tmp/v158-current-approved.did
```

Completed paused readback: `(true)` (separately polled exit0).

### get_recovery_config

```sh
icp canister call nygob-3qaaa-aaaap-qttcq-cai get_recovery_config '()' --query --network ic --identity rumi_identity --candid /private/tmp/v158-current-approved.did
```

Exit 0
```text
(
  opt record {
    icp_fee_e8s = opt (10_000 : nat64);
    backend_principal = principal "tfesu-vyaaa-aaaap-qrd7a-cai";
    ckusdc_ledger = principal "xevnm-gaaaa-aaaar-qafnq-cai";
    icp_ledger = principal "ryjl3-tyaaa-aaaaa-aaaba-cai";
    treasury_principal = principal "tlg74-oiaaa-aaaap-qrd6a-cai";
    ckusdc_fee_e6 = opt (10_000 : nat64);
  },
)
```

### get_pending_claim_journals

```sh
icp canister call nygob-3qaaa-aaaap-qttcq-cai get_pending_claim_journals '()' --query --network ic --identity rumi_identity --candid /private/tmp/v158-current-approved.did
```

Exit 0
```text
(vec {})
```

### get_pending_payment_journals

```sh
icp canister call nygob-3qaaa-aaaap-qttcq-cai get_pending_payment_journals '()' --query --network ic --identity rumi_identity --candid /private/tmp/v158-current-approved.did
```

Exit 0
```text
(vec {})
```

### treasury

```sh
icp canister call nygob-3qaaa-aaaap-qttcq-cai get_pending_treasury_intent '(229 : nat64)' --query --network ic --identity rumi_identity --candid /private/tmp/v158-current-approved.did
```

Exit 0
```text
(null)
```

### record229

```sh
icp canister call nygob-3qaaa-aaaap-qttcq-cai get_liquidation '(229 : nat64)' --query --network ic --identity anonymous --candid /private/tmp/v158-current-approved.did
```

Exit 0
```text
(
  opt variant {
    V1 = record {
      id = 229 : nat64;
      status = variant { Completed };
      ckusdc_transferred_e6 = 5_138_419 : nat64;
      oracle_price_e8s = 238_709_705 : nat64;
      ckusdc_received_e6 = 5_545_309 : nat64;
      error_message = null;
      icp_to_treasury_e8s = 10_742_902 : nat64;
      collateral_claimed_e8s = 236_783_835 : nat64;
      vault_id = 158 : nat64;
      slippage_bps = 0 : int32;
      timestamp = 1_794_081_122_434_828_506 : nat64;
      confirm_retry_count = 0 : nat8;
      debt_to_cover_e8s = 513_841_815 : nat64;
      effective_price_e8s = 238_709_705 : nat64;
      icp_swapped_e8s = 226_020_933 : nat64;
    }
  },
)
```

### backend_claim

```sh
icp canister call tfesu-vyaaa-aaaap-qrd7a-cai get_bot_claim_vault_ids '()' --query --network ic --identity anonymous --candid src/rumi_protocol_backend/rumi_protocol_backend.did
```

Exit 0
```text
(vec {})
```

### vault158

```sh
icp canister call tfesu-vyaaa-aaaap-qrd7a-cai get_vaults_page '(158 : nat64, 1 : nat64)' --query --network ic --identity anonymous --candid src/rumi_protocol_backend/rumi_protocol_backend.did
```

Exit 0
```text
(
  record {
    vaults = vec {
      record {
        collateral_amount = 263_116_164 : nat64;
        owner = principal "ivhvf-a2nxr-t436u-6t3g3-eckj2-gd6ky-q575b-37ciq-7r43a-62nav-wqe";
        vault_id = 158 : nat64;
        collateral_type = principal "ryjl3-tyaaa-aaaaa-aaaba-cai";
        accrued_interest = 0 : nat64;
        icp_margin_amount = 263_116_164 : nat64;
        borrowed_icusd_amount = 420_921_614 : nat64;
      };
    };
    next_start_id = opt (172 : nat64);
  },
)
```

## Ledger receipt readbacks

### icp_history_after

```sh
icp canister call qhbym-qaaaa-aaaaa-aaafq-cai get_account_transactions '(record { account = record { owner = principal "nygob-3qaaa-aaaap-qttcq-cai"; subaccount = null }; start = null; max_results = 2 : nat })' --query --network ic --identity anonymous --candid /private/tmp/v158-icp-index-20261107.did
```

Exit 0
```text
(
  variant {
    Ok = record {
      balance = 40_669_812 : nat64;
      transactions = vec {
        record {
          id = 40_159_014 : nat64;
          transaction = record {
            memo = 0 : nat64;
            icrc1_memo = opt blob "\52\55\4d\49\3a\54\42\31\3a\00\00\00\00\00\00\00\e5\00\00\00\00\00\00\00\01";
            operation = variant {
              Transfer = record {
                to = "e321f2966ff15e699b52187213762cb16d187b66c8d58e73ed337d02c6a5cdc6";
                fee = record { e8s = 10_000 : nat64 };
                from = "f36cb61f612f5acda94f229ea3f65f9e5ca35b5cc1f0fb56abb01ce4fba0633c";
                amount = record { e8s = 10_732_902 : nat64 };
                spender = null;
              }
            };
            timestamp = opt record {
              timestamp_nanos = 1_794_081_134_625_969_945 : nat64;
            };
            created_at_time = opt record {
              timestamp_nanos = 1_794_081_133_118_479_397 : nat64;
            };
          };
        };
        record {
          id = 39_835_236 : nat64;
          transaction = record {
            memo = 0 : nat64;
            icrc1_memo = opt blob "\00\00\00\00\00\11\be\56";
            operation = variant {
              Transfer = record {
                to = "bca85666f3d2de1d135d399a9ccc96d38090601e784e1e5467b87fb3334ec1e0";
                fee = record { e8s = 10_000 : nat64 };
                from = "f36cb61f612f5acda94f229ea3f65f9e5ca35b5cc1f0fb56abb01ce4fba0633c";
                amount = record { e8s = 226_020_933 : nat64 };
                spender = opt "bca85666f3d2de1d135d399a9ccc96d38090601e784e1e5467b87fb3334ec1e0";
              }
            };
            timestamp = opt record {
              timestamp_nanos = 1_793_306_981_079_625_901 : nat64;
            };
            created_at_time = null;
          };
        };
      };
      oldest_tx_id = opt (34_921_553 : nat64);
    }
  },
)
```

### ck_receipt

```sh
icp canister call xevnm-gaaaa-aaaar-qafnq-cai get_transactions '(record { start = 810573 : nat; length = 1 : nat })' --query --network ic --identity anonymous --candid src/declarations/icusd_ledger/icusd_ledger.did
```

Exit 0
```text
(
  record {
    first_index = 810_573 : nat;
    log_length = 810_574 : nat;
    transactions = vec {
      record {
        burn = null;
        kind = "transfer";
        mint = null;
        approve = null;
        timestamp = 1_794_081_123_813_192_987 : nat64;
        transfer = opt record {
          to = record {
            owner = principal "tfesu-vyaaa-aaaap-qrd7a-cai";
            subaccount = null;
          };
          fee = opt (10_000 : nat);
          from = record {
            owner = principal "nygob-3qaaa-aaaap-qttcq-cai";
            subaccount = null;
          };
          memo = opt blob "\52\55\4d\49\2d\42\50\41\59\2d\56\32\3a\00\00\00\00\00\00\00\9e\00\00\00\00\00\00\00\01";
          created_at_time = opt (1_794_081_122_434_828_506 : nat64);
          amount = 5_138_419 : nat;
          spender = null;
        };
      };
    };
    archived_transactions = vec {};
  },
)
```

### balance_ryjl3-tyaaa-aaaaa-aaaba-cai

```sh
icp canister call ryjl3-tyaaa-aaaaa-aaaba-cai icrc1_balance_of '(record { owner = principal "nygob-3qaaa-aaaap-qttcq-cai"; subaccount = null })' --query --network ic --identity anonymous --candid src/declarations/icusd_ledger/icusd_ledger.did
```

Exit 0
```text
(40_669_812 : nat)
```

### balance_xevnm-gaaaa-aaaar-qafnq-cai

```sh
icp canister call xevnm-gaaaa-aaaar-qafnq-cai icrc1_balance_of '(record { owner = principal "nygob-3qaaa-aaaap-qttcq-cai"; subaccount = null })' --query --network ic --identity anonymous --candid src/declarations/icusd_ledger/icusd_ledger.did
```

Exit 0
```text
(26_587_885 : nat)
```

### Direct ICP ledger block40159014

```sh
icp canister call ryjl3-tyaaa-aaaaa-aaaba-cai query_blocks '(record { start = 40159014 : nat64; length = 1 : nat64 })' --query --network ic --identity anonymous --candid /private/tmp/v158-native-ledger-receipt.did
```

Exit0
```text
(
  record {
    certificate = opt blob "\d9\d9\f7\a2\64\74\72\65\65\83\01\83\01\83\01\82\04\58\20\cb\16\92\1e\0f\07\f1\c9\a9\a2\2d\f7\d9\bf\55\ba\00\93\7e\52\18\b9\94\8b\37\58\c0\87\ab\88\ae\d5\83\02\48\63\61\6e\69\73\74\65\72\83\01\83\01\83\01\83\01\82\04\58\20\8a\bc\74\22\ae\71\ce\c7\2c\96\ca\b8\c7\2a\c4\24\09\8c\a8\90\7f\5e\e7\68\dd\4f\6a\6c\2c\39\a3\a9\83\01\83\02\4a\00\00\00\00\00\00\00\02\01\01\83\01\83\01\83\02\4e\63\65\72\74\69\66\69\65\64\5f\64\61\74\61\82\03\58\20\1a\c7\be\68\52\1a\ee\e8\b3\b9\51\ae\44\27\b0\6f\6b\50\6c\a9\f5\48\35\a4\f1\d1\1a\8d\a6\d9\71\2d\82\04\58\20\d8\f6\4f\7a\fc\a6\a5\5d\4e\e6\de\d9\b0\20\0b\ac\66\51\ca\f4\c7\a1\92\02\12\b5\a0\3c\9b\f1\df\36\82\04\58\20\3a\cf\24\66\87\99\5b\71\2c\35\ae\03\a7\0b\d0\2d\af\2c\84\36\60\a7\cd\87\4b\42\4d\d4\23\2f\6c\f3\82\04\58\20\f8\73\91\f3\bc\59\20\04\47\c6\67\a0\a9\4f\22\27\a5\0a\6d\da\a0\ed\70\ab\7d\d0\c1\4a\35\9c\e9\6a\82\04\58\20\2b\3f\32\1e\d6\41\36\9a\73\cb\14\a5\c0\71\f6\69\68\b8\fd\88\6c\83\0e\f0\3f\49\a2\1c\5a\9a\bf\6b\82\04\58\20\13\51\04\ac\fb\21\df\0a\85\a4\28\a0\54\31\fd\10\30\b0\2e\63\80\4f\fe\38\d7\55\65\7e\d7\86\d1\ab\82\04\58\20\9f\7c\3a\2a\6d\25\41\66\f0\84\38\fe\75\32\e3\5a\e8\0d\fd\ba\01\6d\f7\ad\8e\7b\bc\60\7d\19\86\53\82\04\58\20\cf\c2\6d\6d\25\36\03\29\9b\ad\01\8c\da\50\df\a7\33\21\45\9b\bf\63\ea\4d\f5\97\9f\b5\49\14\ef\e7\83\01\82\04\58\20\a0\06\f1\fb\84\8f\21\30\73\92\bc\05\16\d8\f7\55\be\33\7d\32\98\2c\5e\83\c7\87\a0\af\da\e0\4e\29\83\01\82\04\58\20\7f\47\18\06\26\cd\62\c1\67\04\44\f4\cc\59\f1\05\62\fc\6d\43\70\1f\76\4b\9c\b1\81\20\fe\04\74\95\83\02\44\74\69\6d\65\82\03\49\90\e1\85\ed\bf\ea\f6\f2\18\69\73\69\67\6e\61\74\75\72\65\58\30\a8\ea\21\81\de\72\da\50\11\9e\9a\ca\36\1e\bf\f8\5f\ee\b2\f0\08\ed\24\d3\ed\57\ee\3a\68\7f\40\5f\8e\07\a7\77\bc\f7\19\ab\ff\13\8f\6d\99\f7\76\78";
    blocks = vec {
      record {
        transaction = record {
          memo = 0 : nat64;
          icrc1_memo = opt blob "\52\55\4d\49\3a\54\42\31\3a\00\00\00\00\00\00\00\e5\00\00\00\00\00\00\00\01";
          operation = opt variant {
            Transfer = record {
              to = blob "\e3\21\f2\96\6f\f1\5e\69\9b\52\18\72\13\76\2c\b1\6d\18\7b\66\c8\d5\8e\73\ed\33\7d\02\c6\a5\cd\c6";
              fee = record { e8s = 10_000 : nat64 };
              from = blob "\f3\6c\b6\1f\61\2f\5a\cd\a9\4f\22\9e\a3\f6\5f\9e\5c\a3\5b\5c\c1\f0\fb\56\ab\b0\1c\e4\fb\a0\63\3c";
              amount = record { e8s = 10_732_902 : nat64 };
              spender = null;
            }
          };
          created_at_time = record {
            timestamp_nanos = 1_794_081_133_118_479_397 : nat64;
          };
        };
        timestamp = record {
          timestamp_nanos = 1_794_081_134_625_969_945 : nat64;
        };
        parent_hash = opt blob "\92\00\27\8f\08\b1\c1\f4\04\52\4e\0d\b2\21\0c\4e\c3\ab\9a\21\19\79\3d\d9\ca\28\54\f4\2f\61\38\38";
      };
    };
    chain_length = 40_159_052 : nat64;
    first_block_index = 40_159_014 : nat64;
    archived_blocks = vec {};
  },
)
```

## Stop boundary

Stop with processing paused=true. No unpause is authorized by the execution approval. The worker's future resumption and broader automated-reconciliation improvements remain separate decisions; do not replay this recovery. No source or artifact changed. Worktree cleanup removed zero eligible worktrees; protected/unmerged work and branch refs were preserved.
