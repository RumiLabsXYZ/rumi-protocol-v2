import { describe, expect, it } from 'vitest';
import {
  AmountParseError,
  E8S_PER_UNIT,
  U64_MAX,
  formatE8s,
  formatIcusd,
  formatWeiAsEth,
  parseE8s,
  tryParseE8s,
} from './amounts';

describe('parseE8s', () => {
  it('parses whole and fractional decimals exactly', () => {
    expect(parseE8s('1')).toBe(100_000_000n);
    expect(parseE8s('12.5')).toBe(1_250_000_000n);
    expect(parseE8s('0.00000001')).toBe(1n);
    expect(parseE8s('25')).toBe(2_500_000_000n);
    expect(parseE8s('0.1')).toBe(10_000_000n);
  });

  it('never uses floating point (values that break doubles stay exact)', () => {
    // 0.1 + 0.2 and 9007199254740993 are classic float traps.
    expect(parseE8s('0.3')).toBe(30_000_000n);
    expect(parseE8s('90071992.54740993')).toBe(9_007_199_254_740_993n);
    expect(parseE8s('184467440737.09551615')).toBe(U64_MAX);
  });

  it('accepts leading and trailing decimal points and surrounding whitespace', () => {
    expect(parseE8s('.5')).toBe(50_000_000n);
    expect(parseE8s('5.')).toBe(500_000_000n);
    expect(parseE8s('  7.25  ')).toBe(725_000_000n);
    expect(parseE8s('007')).toBe(700_000_000n);
  });

  it('rejects more than 8 decimal places, even when the extras are zero', () => {
    expect(() => parseE8s('1.123456789')).toThrowError(AmountParseError);
    expect(tryParseE8s('1.000000000')).toMatchObject({ ok: false, code: 'too_many_decimals' });
  });

  it('rejects empty, malformed, signed, exponent and separator inputs', () => {
    for (const bad of ['', '   ', '.', 'abc', '1e5', '-1', '+1', '1,000', '1.2.3', '0x10', '1 000', 'NaN', 'Infinity']) {
      const result = tryParseE8s(bad);
      expect(result.ok, `input ${JSON.stringify(bad)}`).toBe(false);
    }
    expect(tryParseE8s('')).toMatchObject({ ok: false, code: 'empty' });
    expect(tryParseE8s('1e5')).toMatchObject({ ok: false, code: 'invalid' });
  });

  it('rejects zero by default but allows it when asked', () => {
    expect(tryParseE8s('0')).toMatchObject({ ok: false, code: 'zero' });
    expect(tryParseE8s('0.00000000')).toMatchObject({ ok: false, code: 'zero' });
    expect(parseE8s('0', { allowZero: true })).toBe(0n);
  });

  it('rejects values above u64::MAX e8s', () => {
    expect(parseE8s('184467440737.09551615')).toBe(U64_MAX);
    expect(tryParseE8s('184467440737.09551616')).toMatchObject({ ok: false, code: 'too_large' });
    expect(tryParseE8s('99999999999999999999')).toMatchObject({ ok: false, code: 'too_large' });
  });
});

describe('formatE8s', () => {
  it('formats exactly and trims trailing zeros', () => {
    expect(formatE8s(0n)).toBe('0');
    expect(formatE8s(1n)).toBe('0.00000001');
    expect(formatE8s(100_000_000n)).toBe('1');
    expect(formatE8s(1_250_000_000n)).toBe('12.5');
    expect(formatE8s(2_500_000_000n)).toBe('25');
    expect(formatE8s(U64_MAX)).toBe('184467440737.09551615');
  });

  it('pads to a minimum fraction length without rounding', () => {
    expect(formatE8s(100_000_000n, { minFraction: 2 })).toBe('1.00');
    expect(formatE8s(1_234_567_890n, { minFraction: 2 })).toBe('12.3456789');
    expect(formatE8s(5n, { minFraction: 8 })).toBe('0.00000005');
    expect(formatE8s(100_000_000n, { minFraction: 99 })).toBe('1.00000000');
  });

  it('groups thousands only on request', () => {
    expect(formatE8s(123_456_789_000_000n)).toBe('1234567.89');
    expect(formatE8s(123_456_789_000_000n, { group: true })).toBe('1,234,567.89');
  });

  it('handles negative differences', () => {
    expect(formatE8s(-150_000_000n)).toBe('-1.5');
  });

  it('round-trips through parse', () => {
    for (const raw of ['0.00000001', '1', '12.5', '99999.12345678', '184467440737.09551615']) {
      expect(formatE8s(parseE8s(raw))).toBe(raw);
    }
    expect(E8S_PER_UNIT).toBe(100_000_000n);
  });

  it('formats with the token suffix', () => {
    expect(formatIcusd(1_250_000_000n)).toBe('12.5 icUSD');
  });
});

describe('formatWeiAsEth', () => {
  it('truncates to the requested digits without rounding up', () => {
    expect(formatWeiAsEth(0n)).toBe('0');
    expect(formatWeiAsEth(10n ** 18n)).toBe('1');
    expect(formatWeiAsEth(1_234_567_890_000_000_000n)).toBe('1.234567');
    expect(formatWeiAsEth(1_999_999_999_999_999_999n, 3)).toBe('1.999');
    expect(formatWeiAsEth(1n)).toBe('0');
    expect(formatWeiAsEth(1n, 18)).toBe('0.000000000000000001');
  });
});
