import { IDL } from '@dfinity/candid';

/**
 * Local IDL for the ckDOGE minter (mainnet `eqltq-xqaaa-aaaar-qb3vq-cai`).
 * No dfx-generated declarations exist for this external canister, so this
 * factory is reviewed and maintained by hand — same pattern as
 * `ledger.idl.js` for the ckUSDT/ckUSDC ledgers.
 *
 * @param {{ IDL: typeof import('@dfinity/candid').IDL }} param0
 */
export const CKDOGE_MINTER_IDL = ({ IDL }) => {
  const AccountArg = IDL.Record({
    owner: IDL.Opt(IDL.Principal),
    subaccount: IDL.Opt(IDL.Vec(IDL.Nat8)),
  });

  // Distinct from AccountArg: status responses report a concrete (non-opt) owner.
  const Account = IDL.Record({
    owner: IDL.Principal,
    subaccount: IDL.Opt(IDL.Vec(IDL.Nat8)),
  });

  const Outpoint = IDL.Record({
    txid: IDL.Vec(IDL.Nat8),
    vout: IDL.Nat32,
  });

  const Utxo = IDL.Record({
    outpoint: Outpoint,
    value: IDL.Nat64,
    height: IDL.Nat32,
  });

  const UtxoStatus = IDL.Variant({
    ValueTooSmall: Utxo,
    Tainted: Utxo,
    Checked: Utxo,
    Minted: IDL.Record({
      block_index: IDL.Nat64,
      minted_amount: IDL.Nat64,
      utxo: Utxo,
    }),
  });

  const PendingUtxo = IDL.Record({
    outpoint: Outpoint,
    value: IDL.Nat64,
    confirmations: IDL.Nat32,
  });

  const SuspendedReason = IDL.Variant({
    ValueTooSmall: IDL.Null,
    Quarantined: IDL.Null,
  });

  const SuspendedUtxo = IDL.Record({
    utxo: Utxo,
    reason: SuspendedReason,
    earliest_retry: IDL.Nat64,
  });

  const UpdateBalanceError = IDL.Variant({
    NoNewUtxos: IDL.Record({
      current_confirmations: IDL.Opt(IDL.Nat32),
      required_confirmations: IDL.Nat32,
      pending_utxos: IDL.Opt(IDL.Vec(PendingUtxo)),
      suspended_utxos: IDL.Opt(IDL.Vec(SuspendedUtxo)),
    }),
    AlreadyProcessing: IDL.Null,
    TemporarilyUnavailable: IDL.Text,
    GenericError: IDL.Record({ error_message: IDL.Text, error_code: IDL.Nat64 }),
  });

  const MinterInfo = IDL.Record({
    min_confirmations: IDL.Nat32,
    deposit_doge_min_amount: IDL.Nat64,
    retrieve_doge_min_amount: IDL.Nat64,
  });

  const WithdrawalFeeOk = IDL.Record({
    dogecoin_fee: IDL.Nat64,
    minter_fee: IDL.Nat64,
  });

  const WithdrawalFeeError = IDL.Variant({
    AmountTooLow: IDL.Record({ min_amount: IDL.Nat64 }),
    AmountTooHigh: IDL.Null,
  });

  const RetrieveDogeWithApprovalArgs = IDL.Record({
    address: IDL.Text,
    amount: IDL.Nat64,
    from_subaccount: IDL.Opt(IDL.Vec(IDL.Nat8)),
  });

  const RetrieveDogeOk = IDL.Record({ block_index: IDL.Nat64 });

  const RetrieveDogeWithApprovalError = IDL.Variant({
    MalformedAddress: IDL.Text,
    AlreadyProcessing: IDL.Null,
    AmountTooLow: IDL.Nat64,
    InsufficientFunds: IDL.Record({ balance: IDL.Nat64 }),
    InsufficientAllowance: IDL.Record({ allowance: IDL.Nat64 }),
    TemporarilyUnavailable: IDL.Text,
    GenericError: IDL.Record({ error_message: IDL.Text, error_code: IDL.Nat64 }),
  });

  const TxId = IDL.Vec(IDL.Nat8);

  const ReimbursementReason = IDL.Variant({
    CallFailed: IDL.Null,
    TaintedDestination: IDL.Record({
      kyt_fee: IDL.Nat64,
      kyt_provider: IDL.Principal,
    }),
  });

  const RetrieveDogeStatus = IDL.Variant({
    Unknown: IDL.Null,
    Pending: IDL.Null,
    Signing: IDL.Null,
    Sending: IDL.Record({ txid: TxId }),
    Submitted: IDL.Record({ txid: TxId }),
    AmountTooLow: IDL.Null,
    Confirmed: IDL.Record({ txid: TxId }),
    Reimbursed: IDL.Record({
      account: Account,
      mint_block_index: IDL.Nat64,
      amount: IDL.Nat64,
      reason: ReimbursementReason,
    }),
    WillReimburse: IDL.Record({
      account: Account,
      amount: IDL.Nat64,
      reason: ReimbursementReason,
    }),
  });

  return IDL.Service({
    get_doge_address: IDL.Func([AccountArg], [IDL.Text], []),
    update_balance: IDL.Func(
      [AccountArg],
      [IDL.Variant({ Ok: IDL.Vec(UtxoStatus), Err: UpdateBalanceError })],
      [],
    ),
    get_minter_info: IDL.Func([], [MinterInfo], ['query']),
    estimate_withdrawal_fee: IDL.Func(
      [IDL.Record({ amount: IDL.Opt(IDL.Nat64) })],
      [IDL.Variant({ Ok: WithdrawalFeeOk, Err: WithdrawalFeeError })],
      ['query'],
    ),
    retrieve_doge_with_approval: IDL.Func(
      [RetrieveDogeWithApprovalArgs],
      [IDL.Variant({ Ok: RetrieveDogeOk, Err: RetrieveDogeWithApprovalError })],
      [],
    ),
    retrieve_doge_status: IDL.Func(
      [IDL.Record({ block_index: IDL.Nat64 })],
      [RetrieveDogeStatus],
      ['query'],
    ),
  });
};

export const idlFactory = CKDOGE_MINTER_IDL;

export default CKDOGE_MINTER_IDL;
