/** ckETH is shared across every ckERC20 redemption for an owner, so the lock
 * must not include the selected token ledger. Keep the optional token argument
 * in the signature to make that invariant explicit at route call sites. */
export function ckErc20WithdrawalLockName(ownerText: string, _tokenLedgerId?: string): string {
  return `rumi:ckerc20:withdrawal:${ownerText}`;
}

export type CkErc20LockManager = {
  request<T>(
    name: string,
    options: { mode: 'exclusive'; ifAvailable: true },
    callback: (lock: unknown | null) => Promise<T>,
  ): Promise<T>;
};

export function withCkErc20WithdrawalLock<T>(
  locks: CkErc20LockManager,
  ownerText: string,
  tokenLedgerId: string,
  callback: (lock: unknown | null) => Promise<T>,
): Promise<T> {
  return locks.request(
    ckErc20WithdrawalLockName(ownerText, tokenLedgerId),
    { mode: 'exclusive', ifAvailable: true },
    callback,
  );
}
