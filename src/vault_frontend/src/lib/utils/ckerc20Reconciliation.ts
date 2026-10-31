/** Storage mutations run under the same Web Lock as submission. The raw snapshot
 * is also compared so a delayed observation can never overwrite a newer intent. */
export function compareAndSetCkErc20Journal(storage: Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>, key: string, expected: string | null, next: string | null): boolean {
  if (storage.getItem(key) !== expected) return false;
  if (next === null) storage.removeItem(key);
  else storage.setItem(key, next);
  return true;
}

export function ckErc20ReceiptOutcome(hash: string, receipt: any): 'pending' | 'success' | 'reverted' {
  if (!receipt || String(receipt.transactionHash ?? '').toLowerCase() !== hash.toLowerCase() || !receipt.blockHash || receipt.blockNumber == null) return 'pending';
  if (receipt.status === '0x1' || receipt.status === 1) return 'success';
  if (receipt.status === '0x0' || receipt.status === 0) return 'reverted';
  return 'pending';
}

export interface CkErc20MintReceipt {
  hash: string; principal: string; evmAccount: string; atomicAmount?: string;
  erc20Address?: string; tokenLedgerId?: string; tokenSymbol?: string;
  eventCursor?: string; eventLogIndex?: string; mintBlock?: string; historyLimited?: boolean;
}

/** Advance only through returned events, joining the exact accepted deposit and
 * its mint. Neither an Ethereum confirmation nor any balance delta proves mint. */
export function advanceCkErc20MintReceipt<T extends CkErc20MintReceipt>(receipt: T, events: any[]): T {
  const next = { ...receipt };
  for (const event of events) {
    const accepted = event.payload?.AcceptedErc20Deposit;
    if (accepted && accepted.transaction_hash?.toLowerCase() === receipt.hash.toLowerCase()
      && accepted.principal?.toText() === receipt.principal
      && accepted.from_address?.toLowerCase() === receipt.evmAccount.toLowerCase()
      && accepted.erc20_contract_address?.toLowerCase() === receipt.erc20Address?.toLowerCase()
      && String(accepted.value) === receipt.atomicAmount
      && (!accepted.subaccount?.length || (accepted.subaccount[0]?.length === 32 && Array.from(accepted.subaccount[0]).every((byte) => byte === 0)))) {
      next.eventLogIndex = String(accepted.log_index);
    }
    const minted = event.payload?.MintedCkErc20;
    if (minted && next.eventLogIndex !== undefined && minted.event_source?.transaction_hash?.toLowerCase() === receipt.hash.toLowerCase()
      && String(minted.event_source.log_index) === next.eventLogIndex
      && minted.erc20_contract_address?.toLowerCase() === receipt.erc20Address?.toLowerCase()
      && minted.ckerc20_token_symbol === receipt.tokenSymbol) next.mintBlock = String(minted.mint_block_index);
  }
  next.eventCursor = String(BigInt(receipt.eventCursor ?? '0') + BigInt(events.length));
  return next;
}

/** These explicit variants describe rejection before a fee burn. Unavailable,
 * unrecognized, and transport-shaped errors may hide a completed burn. */
export function ckErc20WithdrawalHasDefinitiveNoBurn(response: any): boolean {
  const error = response?.Err;
  if (!error || typeof error !== 'object' || Object.keys(error).length !== 1) return false;
  if ('RecipientAddressBlocked' in error || 'TokenNotSupported' in error) return true;
  const ledgerError = error.CkEthLedgerError?.error;
  return !!ledgerError && typeof ledgerError === 'object' && Object.keys(ledgerError).length === 1
    && ['InsufficientFunds', 'InsufficientAllowance', 'AmountTooLow'].some((tag) => tag in ledgerError);
}
