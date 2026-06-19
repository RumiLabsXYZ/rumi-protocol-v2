import { IDL } from "@dfinity/candid";

// Hand-written candid factory for ONLY the four backend methods the monitor
// uses. Kept in sync with rumi_protocol_backend.did. Field order is irrelevant
// (candid records are hashed by name).

export const idlFactory: IDL.InterfaceFactory = ({ IDL }) => {
  const ManualPriceInfo = IDL.Record({ price_e8: IDL.Nat64, set_at_ns: IDL.Nat64 });

  const TransferError = IDL.Variant({
    GenericError: IDL.Record({ message: IDL.Text, error_code: IDL.Nat }),
    TemporarilyUnavailable: IDL.Null,
    BadBurn: IDL.Record({ min_burn_amount: IDL.Nat }),
    Duplicate: IDL.Record({ duplicate_of: IDL.Nat }),
    BadFee: IDL.Record({ expected_fee: IDL.Nat }),
    CreatedInFuture: IDL.Record({ ledger_time: IDL.Nat64 }),
    TooOld: IDL.Null,
    InsufficientFunds: IDL.Record({ balance: IDL.Nat }),
  });
  const TransferFromError = IDL.Variant({
    GenericError: IDL.Record({ message: IDL.Text, error_code: IDL.Nat }),
    TemporarilyUnavailable: IDL.Null,
    InsufficientAllowance: IDL.Record({ allowance: IDL.Nat }),
    BadBurn: IDL.Record({ min_burn_amount: IDL.Nat }),
    Duplicate: IDL.Record({ duplicate_of: IDL.Nat }),
    BadFee: IDL.Record({ expected_fee: IDL.Nat }),
    CreatedInFuture: IDL.Record({ ledger_time: IDL.Nat64 }),
    TooOld: IDL.Null,
    InsufficientFunds: IDL.Record({ balance: IDL.Nat }),
  });
  const ProtocolError = IDL.Variant({
    GenericError: IDL.Text,
    TemporarilyUnavailable: IDL.Text,
    TransferError: TransferError,
    AlreadyProcessing: IDL.Null,
    NotLowestCR: IDL.Null,
    SupplyInvariantHalted: IDL.Null,
    AnonymousCallerNotAllowed: IDL.Null,
    ChainAdmin: IDL.Text,
    AmountTooLow: IDL.Record({ minimum_amount: IDL.Nat64 }),
    TransferFromError: IDL.Tuple(TransferFromError, IDL.Nat64),
    CallerNotOwner: IDL.Null,
  });
  const Result = IDL.Variant({ Ok: IDL.Null, Err: ProtocolError });

  const ChainVaultStatus = IDL.Variant({
    MintPending: IDL.Null,
    Open: IDL.Null,
    Closed: IDL.Null,
    Closing: IDL.Null,
    AwaitingDeposit: IDL.Null,
  });
  const ChainVaultV1 = IDL.Record({
    status: ChainVaultStatus,
    owner: IDL.Principal,
    pending_mint_e8s: IDL.Nat,
    custody_address: IDL.Text,
    collateral_amount_e18: IDL.Nat,
    opened_at_ns: IDL.Nat64,
    vault_id: IDL.Nat64,
    collateral_chain: IDL.Nat32,
    mint_recipient: IDL.Text,
    debt_e8s: IDL.Nat,
  });

  return IDL.Service({
    get_manual_collateral_price: IDL.Func([IDL.Nat32, IDL.Text], [IDL.Opt(ManualPriceInfo)], ["query"]),
    set_manual_collateral_price: IDL.Func([IDL.Nat32, IDL.Text, IDL.Nat64], [Result], []),
    get_price_pusher_principal: IDL.Func([], [IDL.Opt(IDL.Principal)], ["query"]),
    list_chain_vaults: IDL.Func([IDL.Nat32], [IDL.Vec(ChainVaultV1)], ["query"]),
  });
};
