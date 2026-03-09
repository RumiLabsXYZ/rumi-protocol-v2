export const idlFactory = ({ IDL }) => {
  const TreasuryInitArgs = IDL.Record({
    'controller' : IDL.Principal,
    'icp_ledger' : IDL.Principal,
    'ckbtc_ledger' : IDL.Opt(IDL.Principal),
    'icusd_ledger' : IDL.Principal,
  });
  const AssetType = IDL.Variant({
    'ICP' : IDL.Null,
    'ICUSD' : IDL.Null,
    'CKBTC' : IDL.Null,
  });
  const DepositType = IDL.Variant({
    'RedemptionFee' : IDL.Null,
    'MintingFee' : IDL.Null,
    'StabilityFee' : IDL.Null,
    'LiquidationSurplus' : IDL.Null,
  });
  const DepositArgs = IDL.Record({
    'asset_type' : AssetType,
    'block_index' : IDL.Nat64,
    'deposit_type' : DepositType,
    'memo' : IDL.Opt(IDL.Text),
    'amount' : IDL.Nat64,
  });
  const DepositRecord = IDL.Record({
    'id' : IDL.Nat64,
    'asset_type' : AssetType,
    'block_index' : IDL.Nat64,
    'deposit_type' : DepositType,
    'memo' : IDL.Opt(IDL.Text),
    'timestamp' : IDL.Nat64,
    'amount' : IDL.Nat64,
  });
  const AssetBalance = IDL.Record({
    'total' : IDL.Nat64,
    'reserved' : IDL.Nat64,
    'available' : IDL.Nat64,
  });
  const TreasuryStatus = IDL.Record({
    'controller' : IDL.Principal,
    'total_deposits' : IDL.Nat64,
    'is_paused' : IDL.Bool,
    'balances' : IDL.Vec(IDL.Tuple(AssetType, AssetBalance)),
  });
  const WithdrawArgs = IDL.Record({
    'to' : IDL.Principal,
    'asset_type' : AssetType,
    'memo' : IDL.Opt(IDL.Text),
    'amount' : IDL.Nat64,
  });
  const WithdrawResult = IDL.Record({
    'fee' : IDL.Nat64,
    'block_index' : IDL.Nat64,
    'amount_transferred' : IDL.Nat64,
  });
  return IDL.Service({
    'deposit' : IDL.Func(
        [DepositArgs],
        [IDL.Variant({ 'Ok' : IDL.Nat64, 'Err' : IDL.Text })],
        [],
      ),
    'get_deposits' : IDL.Func(
        [IDL.Opt(IDL.Nat64), IDL.Opt(IDL.Nat64)],
        [IDL.Vec(DepositRecord)],
        ['query'],
      ),
    'get_status' : IDL.Func([], [TreasuryStatus], ['query']),
    'set_controller' : IDL.Func(
        [IDL.Principal],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : IDL.Text })],
        [],
      ),
    'set_paused' : IDL.Func(
        [IDL.Bool],
        [IDL.Variant({ 'Ok' : IDL.Null, 'Err' : IDL.Text })],
        [],
      ),
    'withdraw' : IDL.Func(
        [WithdrawArgs],
        [IDL.Variant({ 'Ok' : WithdrawResult, 'Err' : IDL.Text })],
        [],
      ),
  });
};
export const init = ({ IDL }) => {
  const TreasuryInitArgs = IDL.Record({
    'controller' : IDL.Principal,
    'icp_ledger' : IDL.Principal,
    'ckbtc_ledger' : IDL.Opt(IDL.Principal),
    'icusd_ledger' : IDL.Principal,
  });
  return [TreasuryInitArgs];
};
