import { afterEach, describe, expect, it } from 'vitest';
import { toastStore, type ToastData } from './toast';

function readToasts(): ToastData[] {
  let value: ToastData[] = [];
  const unsubscribe = toastStore.subscribe((next) => { value = next; });
  unsubscribe();
  return value;
}

afterEach(() => {
  for (const toast of readToasts()) toastStore.remove(toast.id);
});

describe('toast lifecycle', () => {
  it('retires only the exact obsolete token-funds error', () => {
    const obsolete = 'Insufficient token funds. Your balance is too low for this amount.';
    const unrelatedError = toastStore.error('A different current error', 60_000);
    const obsoleteError = toastStore.error(obsolete, 60_000);
    const success = toastStore.success('Vault opened', 60_000);

    toastStore.removeError(obsolete);

    const remaining = readToasts();
    expect(remaining.map((toast) => toast.id)).toEqual([unrelatedError, success]);
    expect(remaining.some((toast) => toast.id === obsoleteError)).toBe(false);
  });
});
