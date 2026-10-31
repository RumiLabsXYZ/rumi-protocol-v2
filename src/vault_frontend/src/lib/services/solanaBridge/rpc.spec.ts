import { describe, expect, it } from 'vitest';
import { createFetchSolanaRpc, SolanaRpcError } from './rpc';

/** A mocked `fetch`: the adapter is never pointed at a real node by a spec. */
function node(handler: (method: string, params: unknown[]) => unknown) {
  const requests: Array<{ url: string; body: { method: string; params: unknown[] } }> = [];
  const fetchImpl = async (url: string, init: { body: string }) => {
    const body = JSON.parse(init.body);
    requests.push({ url, body });
    const result = handler(body.method, body.params);
    const response = result instanceof Error ? { error: { code: -32000, message: result.message } } : { result };
    return { ok: true, status: 200, json: async () => ({ jsonrpc: '2.0', id: body.id, ...response }) };
  };
  return { requests, rpc: createFetchSolanaRpc('https://node.example.invalid', fetchImpl as never) };
}

describe('the keyless Solana adapter', () => {
  it('reads the blockhash, the height and the holder token accounts with the shapes the node expects', async () => {
    const { rpc, requests } = node((method) => {
      if (method === 'getLatestBlockhash') return { value: { blockhash: 'BH', lastValidBlockHeight: 1234 } };
      if (method === 'getBlockHeight') return 1000;
      return {
        value: [
          { pubkey: 'AccA', account: { owner: 'TokenProg', data: { parsed: { info: { tokenAmount: { amount: '500' } } } } } },
          { pubkey: 'AccB', account: { owner: 'OtherProg', data: { parsed: { info: { tokenAmount: { amount: '9' } } } } } },
        ],
      };
    });
    expect(await rpc.getLatestBlockhash()).toEqual({ blockhash: 'BH', lastValidBlockHeight: 1234n });
    expect(await rpc.getBlockHeight()).toBe(1000n);
    // Only accounts of the configured token program are listed.
    expect(await rpc.getTokenAccounts('Owner', 'Mint', 'TokenProg')).toEqual([{ address: 'AccA', amountE8s: 500n }]);
    expect(requests[0].body).toMatchObject({ jsonrpc: '2.0', method: 'getLatestBlockhash', params: [{ commitment: 'confirmed' }] });
    expect(requests[2].body.params).toEqual(['Owner', { mint: 'Mint' }, { encoding: 'jsonParsed', commitment: 'confirmed' }]);
  });

  it('broadcasts base64 wire bytes and looks a signature up', async () => {
    const { rpc, requests } = node((method) => {
      if (method === 'sendTransaction') return 'SigFromNode';
      if (method === 'getSignatureStatuses') return { value: [{ confirmationStatus: 'finalized', err: null, slot: 77 }] };
      if (method === 'getSignaturesForAddress') return [{ signature: 'S1', err: null }, { signature: 'S2', err: { InstructionError: [0, 'x'] } }];
      return { transaction: ['AQID', 'base64'] };
    });
    expect(await rpc.sendTransaction(Uint8Array.of(1, 2, 3))).toBe('SigFromNode');
    expect(requests[0].body.params).toEqual(['AQID', { encoding: 'base64', preflightCommitment: 'confirmed' }]);
    expect(await rpc.getSignatureStatus('S1')).toEqual({ confirmation: 'finalized', failed: false, slot: 77n });
    expect(requests[1].body.params).toEqual([['S1'], { searchTransactionHistory: true }]);
    expect(await rpc.getSignatures('Addr', 5)).toEqual([{ signature: 'S1', failed: false }, { signature: 'S2', failed: true }]);
    expect(Array.from((await rpc.getTransactionWire('S1'))!)).toEqual([1, 2, 3]);
    expect(requests[2].body.params).toEqual(['Addr', { limit: 5, commitment: 'confirmed' }]);
    expect(requests[3].body.params).toEqual(['S1', { encoding: 'base64', maxSupportedTransactionVersion: 0, commitment: 'confirmed' }]);
  });

  it('answers null for a signature the node does not know, and says what failed otherwise', async () => {
    const { rpc } = node((method) => (method === 'getSignatureStatuses' ? { value: [null] } : method === 'getTransaction' ? null : new Error('Minimum context slot has not been reached')));
    expect(await rpc.getSignatureStatus('S')).toBeNull();
    expect(await rpc.getTransactionWire('S')).toBeNull();
    await expect(rpc.getBlockHeight()).rejects.toThrow('Minimum context slot has not been reached');
    await expect(rpc.getBlockHeight()).rejects.toBeInstanceOf(SolanaRpcError);
  });

  it('turns a network failure and an HTTP error into a plain error', async () => {
    const down = createFetchSolanaRpc('https://node.example.invalid', (async () => {
      throw new Error('offline');
    }) as never);
    await expect(down.getBlockHeight()).rejects.toThrow('could not be reached (offline)');
    const bad = createFetchSolanaRpc('https://node.example.invalid', (async () => ({ ok: false, status: 503, json: async () => ({}) })) as never);
    await expect(bad.getBlockHeight()).rejects.toThrow('HTTP 503');
  });
});
