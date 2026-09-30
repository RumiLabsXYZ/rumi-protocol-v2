import { IDL as CandidIDL } from '@dfinity/candid';

/** Reviewed subset of DFINITY's mainnet ckBTC minter Candid interface. */
/** @param {{ IDL: typeof CandidIDL }} factory */
export const CKBTC_MINTER_IDL = ({ IDL }) => {
  const AccountArg = IDL.Record({
    owner: IDL.Opt(IDL.Principal),
    subaccount: IDL.Opt(IDL.Vec(IDL.Nat8)),
  });
  const Utxo = IDL.Record({
    outpoint: IDL.Record({ txid: IDL.Vec(IDL.Nat8), vout: IDL.Nat32 }),
    value: IDL.Nat64,
    height: IDL.Nat32,
  });
  const PendingUtxo = IDL.Record({
    outpoint: IDL.Record({ txid: IDL.Vec(IDL.Nat8), vout: IDL.Nat32 }),
    value: IDL.Nat64,
    confirmations: IDL.Nat32,
  });
  const SuspendedUtxo = IDL.Record({
    utxo: Utxo,
    reason: IDL.Variant({ ValueTooSmall: IDL.Null, Quarantined: IDL.Null }),
    earliest_retry: IDL.Nat64,
  });
  const UtxoStatus = IDL.Variant({
    ValueTooSmall: Utxo,
    Tainted: Utxo,
    Checked: Utxo,
    Minted: IDL.Record({ block_index: IDL.Nat64, minted_amount: IDL.Nat64, utxo: Utxo }),
  });
  const Account = IDL.Record({ owner: IDL.Principal, subaccount: IDL.Opt(IDL.Vec(IDL.Nat8)) });
  const ReimbursementReason = IDL.Variant({
    CallFailed: IDL.Null,
    TaintedDestination: IDL.Record({ kyt_fee: IDL.Nat64, kyt_provider: IDL.Principal }),
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
  const RetrieveStatus = IDL.Variant({
    Unknown: IDL.Null,
    Pending: IDL.Null,
    Signing: IDL.Null,
    Sending: IDL.Record({ txid: IDL.Vec(IDL.Nat8) }),
    Submitted: IDL.Record({ txid: IDL.Vec(IDL.Nat8) }),
    AmountTooLow: IDL.Null,
    Confirmed: IDL.Record({ txid: IDL.Vec(IDL.Nat8) }),
    Reimbursed: IDL.Record({ account: Account, mint_block_index: IDL.Nat64, amount: IDL.Nat64, reason: ReimbursementReason }),
    WillReimburse: IDL.Record({ account: Account, amount: IDL.Nat64, reason: ReimbursementReason }),
  });
  const RetrieveError = IDL.Variant({
    MalformedAddress: IDL.Text,
    AlreadyProcessing: IDL.Null,
    AmountTooLow: IDL.Nat64,
    InsufficientFunds: IDL.Record({ balance: IDL.Nat64 }),
    InsufficientAllowance: IDL.Record({ allowance: IDL.Nat64 }),
    TemporarilyUnavailable: IDL.Text,
    GenericError: IDL.Record({ error_message: IDL.Text, error_code: IDL.Nat64 }),
  });
  return IDL.Service({
    get_btc_address: IDL.Func([AccountArg], [IDL.Text], []),
    update_balance: IDL.Func([AccountArg], [IDL.Variant({ Ok: IDL.Vec(UtxoStatus), Err: UpdateBalanceError })], []),
    get_minter_info: IDL.Func([], [IDL.Record({
      min_confirmations: IDL.Nat32,
      retrieve_btc_min_amount: IDL.Nat64,
      kyt_fee: IDL.Nat64,
      deposit_btc_min_amount: IDL.Opt(IDL.Nat64),
    })], ['query']),
    get_deposit_fee: IDL.Func([], [IDL.Nat64], ['query']),
    estimate_withdrawal_fee: IDL.Func([IDL.Record({ amount: IDL.Opt(IDL.Nat64) })], [IDL.Record({ bitcoin_fee: IDL.Nat64, minter_fee: IDL.Nat64 })], ['query']),
    retrieve_btc_with_approval: IDL.Func([IDL.Record({ address: IDL.Text, amount: IDL.Nat64, from_subaccount: IDL.Opt(IDL.Vec(IDL.Nat8)) })], [IDL.Variant({ Ok: IDL.Record({ block_index: IDL.Nat64 }), Err: RetrieveError })], []),
    retrieve_btc_status_v2: IDL.Func([IDL.Record({ block_index: IDL.Nat64 })], [RetrieveStatus], ['query']),
    retrieve_btc_status_v2_by_account: IDL.Func([IDL.Opt(Account)], [IDL.Vec(IDL.Record({ block_index: IDL.Nat64, status_v2: IDL.Opt(RetrieveStatus) }))], ['query']),
  });
};

export const idlFactory = CKBTC_MINTER_IDL;
export default CKBTC_MINTER_IDL;
