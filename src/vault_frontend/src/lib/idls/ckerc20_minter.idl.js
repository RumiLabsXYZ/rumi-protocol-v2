import { IDL } from '@dfinity/candid';

/** Minimal, hand-maintained interface for DFINITY's ckETH/ckERC20 minter. */
export const CKERC20_MINTER_IDL = ({ IDL }) => {
  const Subaccount = IDL.Vec(IDL.Nat8);
  const CkErc20Token = IDL.Record({
    ckerc20_token_symbol: IDL.Text,
    erc20_contract_address: IDL.Text,
    ledger_canister_id: IDL.Principal,
  });
  const MinimumDepositAmount = IDL.Record({
    erc20_contract_address: IDL.Text,
    minimum_deposit_amount: IDL.Nat,
  });
  const MinterInfo = IDL.Record({
    deposit_with_subaccount_helper_contract_address: IDL.Opt(IDL.Text),
    minimum_deposit_amounts: IDL.Opt(IDL.Vec(MinimumDepositAmount)),
    supported_ckerc20_tokens: IDL.Opt(IDL.Vec(CkErc20Token)),
    cketh_ledger_id: IDL.Opt(IDL.Principal),
  });
  const PriceArg = IDL.Record({ ckerc20_ledger_id: IDL.Principal });
  const TransactionPrice = IDL.Record({
    gas_limit: IDL.Nat,
    max_fee_per_gas: IDL.Nat,
    max_priority_fee_per_gas: IDL.Nat,
    max_transaction_fee: IDL.Nat,
    timestamp: IDL.Opt(IDL.Nat64),
  });
  const WithdrawErc20Arg = IDL.Record({
    amount: IDL.Nat,
    ckerc20_ledger_id: IDL.Principal,
    recipient: IDL.Text,
    from_cketh_subaccount: IDL.Opt(Subaccount),
    from_ckerc20_subaccount: IDL.Opt(Subaccount),
  });
  const LedgerError = IDL.Variant({
    InsufficientFunds: IDL.Record({ balance: IDL.Nat, failed_burn_amount: IDL.Nat, token_symbol: IDL.Text, ledger_id: IDL.Principal }),
    InsufficientAllowance: IDL.Record({ allowance: IDL.Nat, failed_burn_amount: IDL.Nat, token_symbol: IDL.Text, ledger_id: IDL.Principal }),
    AmountTooLow: IDL.Record({ minimum_burn_amount: IDL.Nat, failed_burn_amount: IDL.Nat, token_symbol: IDL.Text, ledger_id: IDL.Principal }),
    TemporarilyUnavailable: IDL.Text,
  });
  const WithdrawError = IDL.Variant({
    TokenNotSupported: IDL.Record({ supported_tokens: IDL.Vec(CkErc20Token) }),
    RecipientAddressBlocked: IDL.Record({ address: IDL.Text }),
    CkEthLedgerError: IDL.Record({ error: LedgerError }),
    CkErc20LedgerError: IDL.Record({ cketh_block_index: IDL.Nat, error: LedgerError }),
    TemporarilyUnavailable: IDL.Text,
  });
  return IDL.Service({
    get_minter_info: IDL.Func([], [MinterInfo], ['query']),
    eip_1559_transaction_price: IDL.Func([IDL.Opt(PriceArg)], [TransactionPrice], ['query']),
    withdraw_erc20: IDL.Func(
      [WithdrawErc20Arg],
      [IDL.Variant({ Ok: IDL.Record({ cketh_block_index: IDL.Nat, ckerc20_block_index: IDL.Nat }), Err: WithdrawError })],
      [],
    ),
  });
};

export const idlFactory = CKERC20_MINTER_IDL;
export default CKERC20_MINTER_IDL;
