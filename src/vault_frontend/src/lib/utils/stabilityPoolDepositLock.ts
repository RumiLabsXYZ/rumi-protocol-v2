export interface PendingStabilityPoolDeposit {
  owner: string;
  ledger: string;
  amount: string;
  status: 'submitting' | 'accepted';
  attemptId: string;
  createdAt: number;
}

export interface StabilityPoolLockManager {
  request<T>(
    name: string,
    options: { mode: 'exclusive'; ifAvailable: true },
    callback: (lock: unknown | null) => Promise<T>,
  ): Promise<T>;
}

export class PendingStabilityPoolDepositError extends Error {
  constructor(message = 'A prior Stability Pool deposit has no confirmed result. Reconcile its ledger activity and pool position before retrying.') {
    super(message);
    this.name = 'PendingStabilityPoolDepositError';
  }
}

const storageKey = (owner: string, ledger: string) =>
  `rumi:stability-pool:pending-deposit:${owner}:${ledger}`;

const lockName = (owner: string, ledger: string) =>
  `rumi:stability-pool:deposit:${owner}:${ledger}`;

function localStorageOrThrow(): Storage {
  if (typeof localStorage === 'undefined') {
    throw new Error('Persistent browser storage is unavailable; the deposit was not submitted.');
  }
  return localStorage;
}

export function readPendingStabilityPoolDeposit(
  owner: string,
  ledger: string,
): PendingStabilityPoolDeposit | null {
  const serialized = localStorageOrThrow().getItem(storageKey(owner, ledger));
  if (!serialized) return null;
  try {
    const record = JSON.parse(serialized) as Partial<PendingStabilityPoolDeposit>;
    if (record.owner === owner && record.ledger === ledger
      && typeof record.amount === 'string' && /^\d+$/.test(record.amount)
      && (record.status === 'submitting' || record.status === 'accepted')
      && typeof record.attemptId === 'string' && typeof record.createdAt === 'number') {
      return record as PendingStabilityPoolDeposit;
    }
  } catch {
    // A malformed record is still a lock. Never turn damaged storage into permission to retry.
  }
  throw new PendingStabilityPoolDepositError(
    'A Stability Pool deposit recovery record is unreadable. Reconcile the ledger and pool activity before clearing it.',
  );
}

export function savePendingStabilityPoolDeposit(record: PendingStabilityPoolDeposit): void {
  const storage = localStorageOrThrow();
  if (readPendingStabilityPoolDeposit(record.owner, record.ledger)) {
    throw new PendingStabilityPoolDepositError();
  }
  const serialized = JSON.stringify(record);
  storage.setItem(storageKey(record.owner, record.ledger), serialized);
  if (storage.getItem(storageKey(record.owner, record.ledger)) !== serialized) {
    throw new Error('Could not persist the Stability Pool deposit recovery record; the deposit was not submitted.');
  }
}

export function markPendingStabilityPoolDepositAccepted(
  record: PendingStabilityPoolDeposit,
): void {
  const storage = localStorageOrThrow();
  const current = readPendingStabilityPoolDeposit(record.owner, record.ledger);
  if (!current || current.attemptId !== record.attemptId) return;
  const accepted = { ...current, status: 'accepted' as const };
  const key = storageKey(record.owner, record.ledger);
  const serialized = JSON.stringify(accepted);
  storage.setItem(key, serialized);
  if (storage.getItem(key) !== serialized) {
    throw new Error('Could not persist the confirmed Stability Pool deposit result.');
  }
}

export function clearPendingStabilityPoolDeposit(
  owner: string,
  ledger: string,
  attemptId?: string,
): void {
  const storage = localStorageOrThrow();
  const current = readPendingStabilityPoolDeposit(owner, ledger);
  if (current && attemptId && current.attemptId !== attemptId) return;
  storage.removeItem(storageKey(owner, ledger));
  if (storage.getItem(storageKey(owner, ledger)) !== null) {
    throw new Error('Could not clear the Stability Pool deposit recovery record.');
  }
}

export async function clearPendingStabilityPoolDepositAfterReconciliation(
  owner: string,
  ledger: string,
  attemptId: string,
  locks: StabilityPoolLockManager | undefined,
): Promise<void> {
  return withStabilityPoolDepositLock(locks, owner, ledger, async () => {
    const current = readPendingStabilityPoolDeposit(owner, ledger);
    if (!current) return;
    if (current.attemptId !== attemptId) return;
    clearPendingStabilityPoolDeposit(owner, ledger, attemptId);
  });
}

export async function withStabilityPoolDepositLock<T>(
  locks: StabilityPoolLockManager | undefined,
  owner: string,
  ledger: string,
  callback: (lock: unknown) => Promise<T>,
): Promise<T> {
  if (!locks) {
    throw new Error('This browser cannot safely coordinate Stability Pool deposits across tabs. No deposit was submitted.');
  }
  return locks.request(lockName(owner, ledger), { mode: 'exclusive', ifAvailable: true }, async lock => {
    if (!lock) {
      throw new PendingStabilityPoolDepositError(
        'A Stability Pool deposit is already being submitted in another tab. Wait for its result before retrying.',
      );
    }
    return callback(lock);
  });
}

/** These explicit Candid replies are emitted before a transfer, or by the ledger
 * itself when it confirms that transfer_from did not succeed. Other replies,
 * especially SystemBusy and InterCanisterCallFailed, can follow an inter-canister
 * transfer and must remain locked until the user reconciles. */
export function isDefinitiveNoDeposit(error: unknown): boolean {
  if (!error || typeof error !== 'object') return false;
  const tags = [
    'LedgerTransferFailed',
    'TokenNotAccepted',
    'TokenNotActive',
    'AmountTooLow',
    'EmergencyPaused',
  ];
  return tags.some(tag => Object.prototype.hasOwnProperty.call(error, tag));
}

/** ICRC-25 ACTION_ABORTED_ERROR means the signer explicitly canceled before it
 * submitted the canister request. Do not classify generic transport failures as
 * wallet rejection: those can occur after dispatch. */
export function isKnownSignerAbort(error: unknown): boolean {
  return !!error && typeof error === 'object'
    && (error as { code?: number }).code === 3001;
}
