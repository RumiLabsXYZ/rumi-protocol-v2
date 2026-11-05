import { IDL as CandidIDL } from '@dfinity/candid';

/** Minimal, hand-maintained interface for DFINITY's ckETH/ckERC20 minter. */
/** @param {{ IDL: typeof CandidIDL }} factory */
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
  // Official cketh_minter.did: WithdrawalSearchParameter/WithdrawalDetail.
  // https://github.com/dfinity/ic/blob/master/rs/ethereum/cketh/minter/cketh_minter.did
  const EthTransaction = IDL.Record({ transaction_hash: IDL.Text });
  const FinalizedStatus = IDL.Variant({
    Success: IDL.Record({ transaction_hash: IDL.Text, effective_transaction_fee: IDL.Opt(IDL.Nat) }),
    Reimbursed: IDL.Record({ transaction_hash: IDL.Text, reimbursed_amount: IDL.Nat, reimbursed_in_block: IDL.Nat }),
    PendingReimbursement: EthTransaction,
  });
  const WithdrawalStatus = IDL.Variant({ Pending: IDL.Null, TxCreated: IDL.Null, TxSent: EthTransaction, TxFinalized: FinalizedStatus });
  const WithdrawalDetail = IDL.Record({
    token_symbol: IDL.Text, withdrawal_amount: IDL.Nat, max_transaction_fee: IDL.Opt(IDL.Nat),
    withdrawal_id: IDL.Nat64, from: IDL.Principal, from_subaccount: IDL.Opt(Subaccount),
    recipient_address: IDL.Text, status: WithdrawalStatus,
  });
  // Variant names verified against deployed mainnet Candid metadata (2026-10-31).
  // Reserved intentionally discards unrelated event payloads while preserving
  // variant compatibility. New unknown variants fail closed at the decoder.
  const EventSource = IDL.Record({ transaction_hash: IDL.Text, log_index: IDL.Nat });
  const eventPayloads = {
    Init: IDL.Reserved,
    Upgrade: IDL.Reserved,
    AcceptedDeposit: IDL.Reserved,
    InvalidDeposit: IDL.Reserved,
    MintedCkEth: IDL.Reserved,
    SyncedToBlock: IDL.Reserved,
    SyncedErc20ToBlock: IDL.Reserved,
    SyncedDepositWithSubaccountToBlock: IDL.Reserved,
    AcceptedEthWithdrawalRequest: IDL.Reserved,
    AcceptedSweeperFundingRequest: IDL.Reserved,
    CreatedTransaction: IDL.Reserved,
    SignedTransaction: IDL.Reserved,
    ReplacedTransaction: IDL.Reserved,
    FinalizedTransaction: IDL.Reserved,
    AcceptedSweepRequest: IDL.Reserved,
    CreatedSweeperTransaction: IDL.Reserved,
    SignedSweeperTransaction: IDL.Reserved,
    ReplacedSweeperTransaction: IDL.Reserved,
    FinalizedSweeperTransaction: IDL.Reserved,
    ReimbursedEthWithdrawal: IDL.Reserved,
    ReimbursedErc20Withdrawal: IDL.Reserved,
    SkippedBlock: IDL.Reserved,
    AddedCkErc20Token: IDL.Reserved,
    AcceptedErc20WithdrawalRequest: IDL.Reserved,
    FailedErc20WithdrawalRequest: IDL.Reserved,
    QuarantinedDeposit: IDL.Reserved,
    QuarantinedReimbursement: IDL.Reserved,
    RegisteredDepositAddresses: IDL.Reserved,
    AutomaticDepositReceived: IDL.Reserved,
    AttestedDepositAddress: IDL.Reserved,
    AuthorizedDepositAddress: IDL.Reserved,
    AcceptedErc20Deposit: IDL.Record({
      transaction_hash: IDL.Text, block_number: IDL.Nat, log_index: IDL.Nat,
      from_address: IDL.Text, value: IDL.Nat, principal: IDL.Principal,
      erc20_contract_address: IDL.Text, subaccount: IDL.Opt(Subaccount),
    }),
    MintedCkErc20: IDL.Record({ event_source: EventSource, erc20_contract_address: IDL.Text, mint_block_index: IDL.Nat, ckerc20_token_symbol: IDL.Text }),
  };
  return IDL.Service({
    get_events: IDL.Func([IDL.Record({ start: IDL.Nat64, length: IDL.Nat64 })], [IDL.Record({
      events: IDL.Vec(IDL.Record({ timestamp: IDL.Nat64, payload: IDL.Variant(eventPayloads) })), total_event_count: IDL.Nat64,
    })], ['query']),
    withdrawal_status: IDL.Func([IDL.Variant({
      ByRecipient: IDL.Text, BySenderAccount: IDL.Record({ owner: IDL.Principal, subaccount: IDL.Opt(Subaccount) }), ByWithdrawalId: IDL.Nat64,
    })], [IDL.Vec(WithdrawalDetail)], ['query']),
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
