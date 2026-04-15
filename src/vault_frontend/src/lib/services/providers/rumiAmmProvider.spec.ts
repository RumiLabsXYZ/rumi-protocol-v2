import { describe, it, expect, vi, beforeEach } from 'vitest';
import { Principal } from '@dfinity/principal';

// Mock ammService before importing the provider
vi.mock('../ammService', () => ({
  ammService: {
    getPools: vi.fn(),
    getQuote: vi.fn(),
    swap: vi.fn(),
  },
  AMM_TOKENS: [],
}));

import { ammService } from '../ammService';
import { RumiAmmProvider } from './rumiAmmProvider';
import type { AmmToken } from '../ammService';

const tokenThreeUsd: AmmToken = {
  symbol: '3USD', ledgerId: 'fohh4-yyaaa-aaaap-qtkpa-cai',
  decimals: 8, threePoolIndex: -1, is3USD: true,
} as AmmToken;
const tokenIcp: AmmToken = {
  symbol: 'ICP', ledgerId: 'ryjl3-tyaaa-aaaaa-aaaba-cai',
  decimals: 8, threePoolIndex: -1, is3USD: false,
} as AmmToken;

describe('RumiAmmProvider', () => {
  let provider: RumiAmmProvider;
  beforeEach(() => {
    vi.clearAllMocks();
    provider = new RumiAmmProvider();
  });

  it('supports 3USD <-> ICP', () => {
    expect(provider.supports(tokenThreeUsd, tokenIcp)).toBe(true);
    expect(provider.supports(tokenIcp, tokenThreeUsd)).toBe(true);
  });

  it('does not support stable <-> stable', () => {
    const stable: AmmToken = { ...tokenIcp, symbol: 'ckUSDT', threePoolIndex: 1 };
    expect(provider.supports(stable, tokenIcp)).toBe(false);
  });

  it('returns a quote with pool ID cached in meta', async () => {
    vi.mocked(ammService.getPools).mockResolvedValue([
      { pool_id: 'pool-abc',
        token_a: Principal.fromText(tokenThreeUsd.ledgerId),
        token_b: Principal.fromText(tokenIcp.ledgerId),
      } as any,
    ]);
    vi.mocked(ammService.getQuote).mockResolvedValue(500_000_000n);

    const q = await provider.quote(tokenThreeUsd, tokenIcp, 100_000_000n);

    expect(q.provider).toBe('rumi_amm');
    expect(q.amountOut).toBe(500_000_000n);
    expect(q.meta.poolId).toBe('pool-abc');
    expect(q.feeDisplay).toBe('0.30%');
  });
});
