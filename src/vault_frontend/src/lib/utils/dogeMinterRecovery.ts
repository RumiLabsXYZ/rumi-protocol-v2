/** One default-account withdrawal lineage per wallet, minter and origin. */
export interface DogeWithdrawal {
  version: 1;
  id: string;
  owner: string;
  minter: string;
  address: string;
  amount: string;
  createdAt: number;
  phase: 'approving' | 'approved' | 'retrieving' | 'accepted' | 'terminal';
  approveBlock?: string;
  burnBlock?: string;
  status?: string;
  txid?: string;
}
export function dogeWithdrawalKey(owner: string, minter: string): string {
  return `rumi:doge-withdrawal:v1:${minter}:${owner}`;
}
export function readDogeWithdrawal(storage: Storage, owner: string, minter: string): DogeWithdrawal | null {
  const raw = storage.getItem(dogeWithdrawalKey(owner, minter));
  if (raw === null) return null;
  const value = JSON.parse(raw);
  if (value?.version !== 1 || value.owner !== owner || value.minter !== minter ||
      typeof value.id !== 'string' || !value.id || typeof value.address !== 'string' ||
      !/^\d+$/.test(value.amount) || typeof value.createdAt !== 'number' ||
      !['approving', 'approved', 'retrieving', 'accepted', 'terminal'].includes(value.phase) ||
      (value.approveBlock !== undefined && !/^\d+$/.test(value.approveBlock)) ||
      (value.burnBlock !== undefined && !/^\d+$/.test(value.burnBlock)) ||
      (['accepted', 'terminal'].includes(value.phase) && value.burnBlock === undefined) ||
      (value.phase === 'terminal' && !isDogeWithdrawalTerminal(value.status))) {
    throw new Error('Your saved DOGE withdrawal needs support review. It has been preserved.');
  }
  return value;
}
/** Must be called while holding withDogeWithdrawalLock. Never overwrite a newer lineage. */
export function replaceDogeWithdrawal(storage: Storage, expected: DogeWithdrawal | null, next: DogeWithdrawal | null, owner: string, minter: string): void {
  const current = readDogeWithdrawal(storage, owner, minter);
  if (JSON.stringify(current) !== JSON.stringify(expected)) throw new Error('Your withdrawal changed in another tab. Its receipt has been preserved.');
  const key = dogeWithdrawalKey(owner, minter);
  if (next) {
    storage.setItem(key, JSON.stringify(next));
    if (storage.getItem(key) !== JSON.stringify(next)) throw new Error('Could not preserve your withdrawal receipt.');
  } else storage.removeItem(key);
}
export function isDogeWithdrawalTerminal(status: unknown): boolean {
  return status === 'Confirmed' || status === 'Reimbursed';
}
/** Financial writes fail closed without cross-tab locking; public reads remain available. */
export async function withDogeWithdrawalLock<T>(owner: string, minter: string, run: () => Promise<T>): Promise<T> {
  if (typeof navigator === 'undefined' || !navigator.locks?.request) {
    throw new Error('This browser cannot safely coordinate withdrawals. Use a browser with Web Locks support.');
  }
  return navigator.locks.request(dogeWithdrawalKey(owner, minter), { mode: 'exclusive' }, run);
}
