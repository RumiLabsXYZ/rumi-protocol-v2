import { afterEach, describe, expect, it, vi } from 'vitest';
import { readSolDepositBalance } from './cksolDepositBalance';
const address = 'GXVewvv6HehcLFYCmwpqh5CrMjqN9zJ8rqzGq3HEt9Ax';
afterEach(() => vi.unstubAllGlobals());
describe('advisory SOL deposit balance', () => {
  it('reads finalized lamports without a wallet or transaction', async () => {
    const fetcher = vi.fn(async (_input: RequestInfo | URL, _init?: RequestInit) => ({ ok: true, json: async () => ({ result: { value: 1_920_000_000 } }) }));
    vi.stubGlobal('fetch', fetcher);
    expect(await readSolDepositBalance(address)).toBe(1_920_000_000n);
    expect(JSON.parse(String(fetcher.mock.calls[0]?.[1]?.body))).toMatchObject({ method:'getBalance', params:[address,{commitment:'finalized'}] });
  });
  it.each([null, -1, 0.5, Number.MAX_SAFE_INTEGER + 1])('rejects invalid lamports: %s', async value => {
    vi.stubGlobal('fetch', vi.fn(async () => ({ok:true,json:async()=>({result:{value}})})));
    await expect(readSolDepositBalance(address)).rejects.toThrow('invalid response');
  });
  it('reports unavailable RPC without inventing a zero balance', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => ({ok:false})));
    await expect(readSolDepositBalance(address)).rejects.toThrow('temporarily unavailable');
  });
});
