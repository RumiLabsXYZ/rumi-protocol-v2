import { beforeEach, describe, expect, it, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
  createAgent: vi.fn(),
  createActor: vi.fn(),
  fromText: vi.fn((text: string) => ({ principalText: text })),
}));

vi.mock('@icp-sdk/signer', () => ({ Signer: vi.fn() }));
vi.mock('@icp-sdk/signer/agent', () => ({ SignerAgent: { create: mocks.createAgent } }));
vi.mock('@icp-sdk/signer/web', () => ({ PostMessageTransport: vi.fn() }));
vi.mock('@icp-sdk/core/principal', () => ({ Principal: { fromText: mocks.fromText } }));
vi.mock('@dfinity/agent', () => ({ Actor: { createActor: mocks.createActor } }));

import { clearOisySigner, getOisySignerAgent } from './oisySigner';

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

function principal(principalText: string) {
  return { toText: () => principalText } as any;
}

describe('Oisy signer agent cache', () => {
  beforeEach(() => {
    clearOisySigner();
    vi.clearAllMocks();
  });

  it('keeps the newest principal cached when an older prewarm resolves last', async () => {
    const accountA = deferred<any>();
    const accountB = deferred<any>();
    const agentA = { account: 'A' };
    const agentB = { account: 'B' };
    mocks.createAgent.mockImplementation(({ account }: any) =>
      account.principalText === 'account-A' ? accountA.promise : accountB.promise,
    );

    const warmingA = getOisySignerAgent(principal('account-A'));
    const warmingB = getOisySignerAgent(principal('account-B'));
    accountB.resolve(agentB);
    await expect(warmingB).resolves.toBe(agentB);
    accountA.resolve(agentA);
    await expect(warmingA).resolves.toBe(agentA);

    await expect(getOisySignerAgent(principal('account-B'))).resolves.toBe(agentB);
    expect(mocks.createAgent).toHaveBeenCalledTimes(2);
  });

  it('shares an in-flight create for concurrent requests by the same principal', async () => {
    const pending = deferred<any>();
    const agent = { account: 'A' };
    mocks.createAgent.mockReturnValue(pending.promise);

    const first = getOisySignerAgent(principal('account-A'));
    const second = getOisySignerAgent(principal('account-A'));
    expect(mocks.createAgent).toHaveBeenCalledTimes(1);
    pending.resolve(agent);
    await expect(first).resolves.toBe(agent);
    await expect(second).resolves.toBe(agent);
  });

  it('does not repopulate the cache after disconnect clears pending creates', async () => {
    const oldAttempt = deferred<any>();
    const freshAgent = { account: 'A-fresh' };
    mocks.createAgent.mockReturnValueOnce(oldAttempt.promise).mockResolvedValueOnce(freshAgent);

    const oldRequest = getOisySignerAgent(principal('account-A'));
    clearOisySigner();
    oldAttempt.resolve({ account: 'A-old' });
    await oldRequest;

    await expect(getOisySignerAgent(principal('account-A'))).resolves.toBe(freshAgent);
    expect(mocks.createAgent).toHaveBeenCalledTimes(2);
  });
});
