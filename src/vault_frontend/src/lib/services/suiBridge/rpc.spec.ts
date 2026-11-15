import { describe, expect, it, vi } from 'vitest';
import { SUI_NODE_ENDPOINTS } from './endpoints';
import { createGraphqlSuiRpc, resolveSuiRpc, SuiRpcError, type SuiNodeEndpoint } from './rpc';
import { SUI_MAINNET, SUI_TESTNET } from './networks';
import { KEYS, COIN_TYPE } from './testSupport';

/**
 * These specs run the REAL SDK transport (`SuiGraphQLClient` from `@mysten/sui`) over a mocked `fetch`. No request leaves the
 * process, no wallet is involved and no key, keystore or CLI configuration is read.
 */

type Handler = (operation: string, variables: Record<string, any>) => unknown;

function node(handler: Handler, chain = SUI_MAINNET.chainIdentifierBase58) {
  const requests: Array<{ url: string; operation: string; variables: Record<string, any> }> = [];
  const fetchImpl = (async (url: string, init: { body: string }) => {
    const body = JSON.parse(init.body);
    const operation = /(?:query|mutation)\s+(\w+)/.exec(body.query)?.[1] ?? 'unknown';
    requests.push({ url, operation, variables: body.variables ?? {} });
    const data = operation === 'getChainIdentifier' ? { checkpoint: { digest: chain } } : handler(operation, body.variables ?? {});
    return { ok: true, status: 200, statusText: 'OK', json: async () => (data instanceof Error ? { errors: [{ message: data.message }] } : { data }) };
  }) as never;
  const endpoint: SuiNodeEndpoint = { networkId: SUI_MAINNET.networkId, chainIdentifier: SUI_MAINNET.chainIdentifier, url: 'https://sui.example.invalid/graphql' };
  const rpc = resolveSuiRpc(SUI_MAINNET, [endpoint], fetchImpl);
  if (!rpc) throw new Error('the endpoint did not resolve');
  return { rpc, requests, fetchImpl, endpoint };
}

const COIN = (id: string, balance: string) => ({ address: id, version: 7, digest: '4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQff4P3bkLKi', owner: { __typename: 'AddressOwner', address: { address: KEYS.holder } }, contents: { type: { repr: `0x2::coin::Coin<${COIN_TYPE}>` }, json: { balance } } });
const TX = (overrides: Record<string, any> = {}) => ({
  digest: '4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQff4P3bkLKi',
  signatures: [{ signatureBytes: 'AAAA' }],
  effects: { status: 'SUCCESS', timestamp: '2026-10-31T00:00:00Z', checkpoint: { sequenceNumber: 500 }, executionError: null, epoch: { epochId: 123 } },
  ...overrides,
});

describe('the deployment-time endpoint seam', () => {
  it('ships EMPTY: no endpoint, no adapter, no request, and importing the SDK-backed module does nothing', () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    expect(SUI_NODE_ENDPOINTS).toEqual([]);
    expect(resolveSuiRpc(SUI_MAINNET, SUI_NODE_ENDPOINTS)).toBeNull();
    expect(resolveSuiRpc(SUI_TESTNET, SUI_NODE_ENDPOINTS)).toBeNull();
    expect(fetchSpy).not.toHaveBeenCalled();
    fetchSpy.mockRestore();
  });

  it('builds the REAL SDK adapter for an entry keyed to this network and chain identifier, and makes no request until a flow asks', () => {
    const { requests, rpc } = node(() => ({}));
    expect(rpc).toBeTruthy();
    expect(requests).toHaveLength(0);
  });

  it('never uses an entry written for another network or another chain identifier', () => {
    const entry: SuiNodeEndpoint = { networkId: SUI_MAINNET.networkId, chainIdentifier: SUI_MAINNET.chainIdentifier, url: 'https://sui.example.invalid/graphql' };
    expect(resolveSuiRpc(SUI_MAINNET, [{ ...entry, networkId: SUI_TESTNET.networkId }])).toBeNull();
    expect(resolveSuiRpc(SUI_MAINNET, [{ ...entry, chainIdentifier: SUI_TESTNET.chainIdentifier }])).toBeNull();
    expect(resolveSuiRpc(SUI_TESTNET, [entry])).toBeNull();
  });

  it('treats an empty, malformed or insecure URL as not configured', () => {
    const entry: SuiNodeEndpoint = { networkId: SUI_MAINNET.networkId, chainIdentifier: SUI_MAINNET.chainIdentifier, url: '' };
    for (const url of ['', '   ', 'not a url', 'http://sui.example.invalid', 'ftp://sui.example.invalid', 'javascript:alert(1)']) {
      expect(resolveSuiRpc(SUI_MAINNET, [{ ...entry, url }]), url).toBeNull();
    }
    expect(resolveSuiRpc(SUI_MAINNET, [{ ...entry, url: 'http://127.0.0.1:9000/graphql' }])).not.toBeNull();
    expect(resolveSuiRpc(SUI_MAINNET, [{ ...entry, url: 'http://localhost:9000/graphql' }])).not.toBeNull();
  });
});

describe('the real SDK transport over a mocked fetch', () => {
  it('asks the node for its own chain identifier before it trusts anything, and only talks to the configured URL', async () => {
    const { rpc, requests } = node((operation) => (operation === 'getReferenceGasPrice' ? { epoch: { referenceGasPrice: '1000' } } : {}));
    expect(await rpc.getReferenceGasPrice()).toBe(1000n);
    expect(requests.map((r) => r.operation)).toEqual(['getChainIdentifier', 'getReferenceGasPrice']);
    expect(new Set(requests.map((r) => r.url))).toEqual(new Set(['https://sui.example.invalid/graphql']));
    // The check is made once, not before every read.
    await rpc.getReferenceGasPrice();
    expect(requests.filter((r) => r.operation === 'getChainIdentifier')).toHaveLength(1);
  });

  it('refuses a node on another network and reads nothing else from it', async () => {
    const { rpc, requests } = node(() => ({ address: { objects: { pageInfo: { hasNextPage: false, endCursor: null }, nodes: [] } } }), SUI_TESTNET.chainIdentifierBase58);
    await expect(rpc.listCoins(KEYS.holder, COIN_TYPE)).rejects.toThrow('on another network');
    await expect(rpc.listCoins(KEYS.holder, COIN_TYPE)).rejects.toBeInstanceOf(SuiRpcError);
    expect(requests.every((r) => r.operation === 'getChainIdentifier')).toBe(true);
  });

  it('lists every page of the holder\'s icUSD coins with exact bigint balances (e8s)', async () => {
    const pages = [
      { address: { objects: { pageInfo: { hasNextPage: true, endCursor: 'c1' }, nodes: [COIN(KEYS.coinA, '18446744073709551615')] } } },
      { address: { objects: { pageInfo: { hasNextPage: false, endCursor: null }, nodes: [COIN(KEYS.coinB, '1')] } } },
    ];
    let page = 0;
    const { rpc, requests } = node((operation) => (operation === 'getCoins' ? pages[page++] : {}));
    const coins = await rpc.listCoins(KEYS.holder, COIN_TYPE);
    expect(coins.map((c) => [c.objectId, c.balance, c.version])).toEqual([
      [KEYS.coinA, 18446744073709551615n, '7'],
      [KEYS.coinB, 1n, '7'],
    ]);
    const asked = requests.filter((r) => r.operation === 'getCoins');
    expect(asked).toHaveLength(2);
    expect(asked[0].variables).toMatchObject({ owner: KEYS.holder, type: `0x2::coin::Coin<${COIN_TYPE}>` });
    expect(asked[1].variables.cursor).toBe('c1');
  });

  it('refuses an incomplete coin and a holder with too many coin objects to list', async () => {
    const incomplete = node((operation) => (operation === 'getCoins' ? { address: { objects: { pageInfo: { hasNextPage: false, endCursor: null }, nodes: [{ ...COIN(KEYS.coinA, '5'), version: null }] } } } : {}));
    await expect(incomplete.rpc.listCoins(KEYS.holder, COIN_TYPE)).rejects.toThrow('incomplete coin');
    const endless = node((operation) => (operation === 'getCoins' ? { address: { objects: { pageInfo: { hasNextPage: true, endCursor: 'c' }, nodes: [COIN(KEYS.coinA, '5')] } } } : {}));
    await expect(endless.rpc.listCoins(KEYS.holder, COIN_TYPE)).rejects.toThrow('too many icUSD coin objects');
  });

  it('reads the SUI balance in MIST (9 decimals) as a bigint, apart from icUSD', async () => {
    const { rpc, requests } = node((operation) => (operation === 'getBalance' ? { address: { balance: { coinType: { repr: '0x2::sui::SUI' }, totalBalance: '1234567890123', addressBalance: '0' } } } : {}));
    expect(await rpc.getSuiBalance(KEYS.holder)).toBe(1_234_567_890_123n);
    expect(requests.find((r) => r.operation === 'getBalance')!.variables.coinType).toMatch(/^0x0*2::sui::SUI$/);
  });

  it('reads the epoch', async () => {
    const { rpc } = node((operation) => (operation === 'getCurrentSystemState' ? { epoch: { epochId: 123, referenceGasPrice: '1000', startTimestamp: '2026-10-31T00:00:00Z', protocolConfigs: { protocolVersion: 100 }, systemState: { json: {} } } } : {}));
    expect(await rpc.getEpoch()).toBe(123n);
  });

  it('broadcasts the signed bytes with their signatures and reports the node\'s effects', async () => {
    const { rpc, requests } = node((operation) => (operation === 'executeTransaction' ? { executeTransaction: { effects: { transaction: TX({ effects: { ...TX().effects, checkpoint: null } }) } } } : {}));
    const result = await rpc.executeTransaction(Uint8Array.of(1, 2, 3), ['SIG']);
    expect(result).toEqual({ digest: '4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQff4P3bkLKi', success: true, checkpoint: null, epoch: 123n, error: null });
    const sent = requests.find((r) => r.operation === 'executeTransaction')!;
    expect(sent.variables).toMatchObject({ transactionDataBcs: 'AQID', signatures: ['SIG'] });
  });

  it('reads a transaction\'s status: in a checkpoint is final, a failure carries its error, and an unknown digest is null (never proof of absence)', async () => {
    const answers: Record<string, any> = {
      done: { transaction: TX() },
      failed: { transaction: TX({ effects: { ...TX().effects, status: 'FAILURE', executionError: { message: 'MoveAbort 6', abortCode: '6', identifier: null, constant: null, sourceLineNumber: null, instructionOffset: null, module: null, function: null } } }) },
      unknown: { transaction: null },
    };
    const { rpc } = node((operation, variables) => (operation === 'getTransactionBlock' ? answers[variables.digest] : {}));
    expect(await rpc.getTransactionStatus('done')).toEqual({ success: true, checkpoint: 500n, epoch: 123n, error: null });
    const failed = await rpc.getTransactionStatus('failed');
    expect(failed).toMatchObject({ success: false, checkpoint: 500n });
    expect(failed!.error).toContain('MoveAbort');
    expect(await rpc.getTransactionStatus('unknown')).toBeNull();
  });

  it('turns a failed request into a plain error, never into "not found"', async () => {
    const down = resolveSuiRpc(SUI_MAINNET, [{ networkId: SUI_MAINNET.networkId, chainIdentifier: SUI_MAINNET.chainIdentifier, url: 'https://sui.example.invalid/graphql' }], (async () => {
      throw new Error('offline');
    }) as never)!;
    await expect(down.getTransactionStatus('x')).rejects.toThrow('could not be read');
    const bad = resolveSuiRpc(SUI_MAINNET, [{ networkId: SUI_MAINNET.networkId, chainIdentifier: SUI_MAINNET.chainIdentifier, url: 'https://sui.example.invalid/graphql' }], (async () => ({ ok: false, status: 503, statusText: 'Service Unavailable', json: async () => ({}) })) as never)!;
    await expect(bad.getTransactionStatus('x')).rejects.toThrow('could not be read');
  });

  it('bootstraps no identity: the only effect of using it is the mocked fetch', async () => {
    const before = Object.keys(window.localStorage).length;
    const { rpc, requests } = node((operation) => (operation === 'getReferenceGasPrice' ? { epoch: { referenceGasPrice: '7' } } : {}));
    await rpc.getReferenceGasPrice();
    expect(Object.keys(window.localStorage)).toHaveLength(before);
    expect(requests.length).toBeGreaterThan(0);
  });

  it('can be built directly for a given URL too (the factory the seam wraps)', async () => {
    const { fetchImpl } = node(() => ({}));
    const rpc = createGraphqlSuiRpc({ url: 'https://other.example.invalid/graphql', network: SUI_MAINNET, fetchImpl });
    expect(await rpc.getChainIdentifier()).toBe(SUI_MAINNET.chainIdentifierBase58);
  });
});
