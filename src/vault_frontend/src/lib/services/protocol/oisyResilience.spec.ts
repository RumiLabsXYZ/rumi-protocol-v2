import { describe, it, expect, vi, beforeEach } from 'vitest';
import {
  isOisyArrFalseNegative,
  isOisyLandedSentinel,
  callWithOisyFalseNegativeGuard,
  OISY_LANDED,
} from './oisyResilience';

describe('isOisyArrFalseNegative', () => {
  it('matches the actual Oisy error message captured from production', () => {
    // The literal payload Oisy returned in JSON-RPC error responses on 2026-05-05
    const err = new Error("Cannot read properties of undefined (reading '_arr')");
    expect(isOisyArrFalseNegative(err)).toBe(true);
  });

  it('matches even when the pattern is wrapped in a longer message', () => {
    const err = new Error(
      "Po: Cannot read properties of undefined (reading '_arr') at Va (chunks/x.js:1)"
    );
    expect(isOisyArrFalseNegative(err)).toBe(true);
  });

  it('matches plain object errors with a string .message field', () => {
    expect(
      isOisyArrFalseNegative({ message: "Cannot read properties of undefined (reading '_arr')" })
    ).toBe(true);
  });

  it('does NOT match unrelated errors mentioning undefined or _arr separately', () => {
    expect(isOisyArrFalseNegative(new Error('something is undefined'))).toBe(false);
    expect(isOisyArrFalseNegative(new Error('property _arr is missing'))).toBe(false);
    expect(isOisyArrFalseNegative(new Error('Cannot read properties of null'))).toBe(false);
  });

  it('safely handles null, undefined, and non-error values', () => {
    expect(isOisyArrFalseNegative(null)).toBe(false);
    expect(isOisyArrFalseNegative(undefined)).toBe(false);
    expect(isOisyArrFalseNegative('arbitrary string')).toBe(false);
    expect(isOisyArrFalseNegative(42)).toBe(false);
  });
});

describe('isOisyLandedSentinel', () => {
  it('recognises the OISY_LANDED constant', () => {
    expect(isOisyLandedSentinel(OISY_LANDED)).toBe(true);
  });

  it('rejects unrelated objects', () => {
    expect(isOisyLandedSentinel({ Ok: { vault_id: 1n } })).toBe(false);
    expect(isOisyLandedSentinel({ __oisyLanded: false })).toBe(false);
    expect(isOisyLandedSentinel(null)).toBe(false);
    expect(isOisyLandedSentinel(undefined)).toBe(false);
  });
});

describe('callWithOisyFalseNegativeGuard', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.spyOn(console, 'warn').mockImplementation(() => {});
    vi.spyOn(console, 'error').mockImplementation(() => {});
  });

  it('returns the call result unchanged when the call succeeds', async () => {
    const call = vi.fn().mockResolvedValue({ Ok: { block_index: 42n } });
    const verify = vi.fn();

    const result = await callWithOisyFalseNegativeGuard(call, verify, 'test op');

    expect(result).toEqual({ Ok: { block_index: 42n } });
    expect(verify).not.toHaveBeenCalled();
  });

  it('re-throws non-Oisy errors without consulting the verifier', async () => {
    const err = new Error('network down');
    const call = vi.fn().mockRejectedValue(err);
    const verify = vi.fn();

    await expect(callWithOisyFalseNegativeGuard(call, verify, 'test op')).rejects.toBe(err);
    expect(verify).not.toHaveBeenCalled();
  });

  it('returns OISY_LANDED when the call hits the _arr pattern AND verifier confirms success', async () => {
    const oisyErr = new Error("Cannot read properties of undefined (reading '_arr')");
    const call = vi.fn().mockRejectedValue(oisyErr);
    const verify = vi.fn().mockResolvedValue(true);

    const result = await callWithOisyFalseNegativeGuard(call, verify, 'borrow 1 icUSD');

    expect(isOisyLandedSentinel(result)).toBe(true);
    expect(verify).toHaveBeenCalledOnce();
  });

  it('re-throws the original error when verifier returns false', async () => {
    const oisyErr = new Error("Cannot read properties of undefined (reading '_arr')");
    const call = vi.fn().mockRejectedValue(oisyErr);
    const verify = vi.fn().mockResolvedValue(false);

    await expect(callWithOisyFalseNegativeGuard(call, verify, 'borrow 1 icUSD')).rejects.toBe(
      oisyErr
    );
  });

  it('re-throws the ORIGINAL error (not the verifier error) when verifier itself throws', async () => {
    const oisyErr = new Error("Cannot read properties of undefined (reading '_arr')");
    const verifyErr = new Error('IC fetch failed');
    const call = vi.fn().mockRejectedValue(oisyErr);
    const verify = vi.fn().mockRejectedValue(verifyErr);

    await expect(callWithOisyFalseNegativeGuard(call, verify, 'borrow 1 icUSD')).rejects.toBe(
      oisyErr
    );
  });
});
