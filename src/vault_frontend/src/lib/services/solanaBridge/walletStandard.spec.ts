import type { Wallet, WalletAccount } from '@wallet-standard/base';
import { get } from 'svelte/store';
import { describe, expect, it, vi } from 'vitest';
import { makeAccount as account, makeRegistry as registry, makeStandardWallet as wallet } from './testSupport';
import {
  SOLANA_SIGN_AND_SEND_TRANSACTION,
  SOLANA_SIGN_TRANSACTION,
  STANDARD_CONNECT,
  STANDARD_EVENTS,
  createSolanaWalletStore,
  isUsableSolanaWallet,
  isExplicitUserRejection,
  isWalletRejection,
} from './walletStandard';

describe('which wallets are usable', () => {
  it('needs connect, events, a Solana chain and a Solana signing feature', () => {
    expect(isUsableSolanaWallet(wallet('ok', [account('A')]))).toBe(true);
    expect(isUsableSolanaWallet(wallet('sign and send only', [account('A')], { [SOLANA_SIGN_TRANSACTION]: undefined, [SOLANA_SIGN_AND_SEND_TRANSACTION]: { signAndSendTransaction: async () => [] } }))).toBe(true);
    expect(isUsableSolanaWallet(wallet('no signer', [account('A')], { [SOLANA_SIGN_TRANSACTION]: undefined }))).toBe(false);
    expect(isUsableSolanaWallet(wallet('evm only', [account('A')], {}, ['eip155:1']))).toBe(false);
    expect(isUsableSolanaWallet(wallet('no events', [account('A')], { [STANDARD_EVENTS]: undefined }))).toBe(false);
  });
});

describe('the Solana wallet store', () => {
  it('discovers wallets without asking any to connect', () => {
    const w = wallet('Fake', [account('Addr1')]);
    const reg = registry([w, wallet('EVM', [account('0x')], {}, ['eip155:1'])]);
    const store = createSolanaWalletStore({ registry: () => reg });
    expect(get(store).status).toBe('unavailable');
    store.init();
    expect(get(store)).toMatchObject({ status: 'disconnected', wallets: [{ name: 'Fake' }], address: null });
    expect(w.connects).toBe(0);
    expect(store.session()).toBeNull();
    store.destroy();
  });

  it('picks up a wallet that registers later, and drops one that goes away', () => {
    const reg = registry([]);
    const store = createSolanaWalletStore({ registry: () => reg });
    store.init();
    expect(get(store).status).toBe('unavailable');
    const w = wallet('Late', [account('Addr1')]);
    reg.add(w);
    expect(get(store)).toMatchObject({ status: 'disconnected', wallets: [{ name: 'Late' }] });
    reg.remove(w);
    expect(get(store).status).toBe('unavailable');
  });

  it('connects on request: one account, its chains and what the wallet can sign', async () => {
    const w = wallet('Fake', [account('Addr1', ['solana:mainnet', 'solana:devnet'])]);
    const store = createSolanaWalletStore({ registry: () => registry([w]) });
    store.init();
    await store.connect();
    expect(w.connects).toBe(1);
    expect(get(store)).toMatchObject({ status: 'connected', walletName: 'Fake', address: 'Addr1', chains: ['solana:mainnet', 'solana:devnet'], canSign: true, canSignAndSend: false, error: null });
    const session = store.session()!;
    expect(session.address).toBe('Addr1');
    const signed = await session.signTransaction(Uint8Array.of(1, 2, 3), 'solana:mainnet');
    expect(Array.from(signed)).toEqual([1, 2, 3]);
    expect(w.signs).toEqual([{ account: expect.anything(), transaction: Uint8Array.of(1, 2, 3), chain: 'solana:mainnet' }]);
    await expect(session.signAndSendTransaction(Uint8Array.of(1), 'solana:mainnet')).rejects.toThrow(/cannot sign and send/);
  });

  it('asks the user to choose when several wallets exist, and connects only the chosen one', async () => {
    const a = wallet('A', [account('AddrA')]);
    const b = wallet('B', [account('AddrB')]);
    const store = createSolanaWalletStore({ registry: () => registry([a, b]) });
    store.init();
    await store.connect();
    expect(get(store).error).toBe('Choose a wallet to connect.');
    expect([a.connects, b.connects]).toEqual([0, 0]);
    await store.connect('B');
    expect([a.connects, b.connects]).toEqual([0, 1]);
    expect(get(store).address).toBe('AddrB');
  });

  it('reports a failed or empty connection without leaving a connected state', async () => {
    const failing = wallet('Failing', [account('A')]);
    (failing.features as Record<string, { connect: () => Promise<unknown> }>)[STANDARD_CONNECT].connect = async () => {
      throw new Error('User rejected the request.');
    };
    const store = createSolanaWalletStore({ registry: () => registry([failing]) });
    store.init();
    await store.connect();
    expect(get(store)).toMatchObject({ status: 'disconnected', address: null, error: 'User rejected the request.' });
    const empty = wallet('Empty', []);
    const store2 = createSolanaWalletStore({ registry: () => registry([empty]) });
    store2.init();
    await store2.connect();
    expect(get(store2)).toMatchObject({ status: 'disconnected', address: null });
    expect(get(store2).error).toContain('no account');
  });

  it('follows the wallet: an account switch updates the session, a disconnect clears it', async () => {
    const w = wallet('Fake', [account('Addr1'), account('Addr2')]);
    const store = createSolanaWalletStore({ registry: () => registry([w]) });
    store.init();
    await store.connect();
    expect(get(store).address).toBe('Addr1');
    // The wallet reports the other account first: the page follows the wallet's choice.
    w.emit({ accounts: [account('Addr2'), account('Addr1')] });
    expect(get(store).address).toBe('Addr1');
    w.emit({ accounts: [account('Addr3', ['solana:devnet'])] });
    expect(get(store)).toMatchObject({ address: 'Addr3', chains: ['solana:devnet'] });
    expect(store.session()?.address).toBe('Addr3');
    w.emit({ accounts: [] });
    expect(get(store)).toMatchObject({ status: 'disconnected', address: null });
    expect(store.session()).toBeNull();
    expect(w.offs).toBeGreaterThan(0);
  });

  it('disconnects and stops listening when destroyed', async () => {
    const w = wallet('Fake', [account('Addr1')]);
    const reg = registry([w]);
    const store = createSolanaWalletStore({ registry: () => reg });
    store.init();
    await store.connect();
    await store.disconnect();
    expect(w.disconnects).toBe(1);
    expect(get(store)).toMatchObject({ status: 'disconnected', address: null });
    await store.connect();
    store.destroy();
    expect(w.offs).toBeGreaterThan(0);
  });

  it('ignores a connection that finished after a newer request', async () => {
    let release: (value: { accounts: WalletAccount[] }) => void = () => {};
    const slow = wallet('Slow', [account('Old')]);
    (slow.features as Record<string, { connect: () => Promise<unknown> }>)[STANDARD_CONNECT].connect = () => new Promise((resolve) => (release = resolve as typeof release));
    const store = createSolanaWalletStore({ registry: () => registry([slow]) });
    store.init();
    const first = store.connect();
    await store.disconnect();
    release({ accounts: [account('Old')] });
    await first;
    expect(get(store)).toMatchObject({ status: 'disconnected', address: null });
  });
});

describe('what counts as the user saying no', () => {
  it('recognizes a rejection by code, name or message and nothing else', () => {
    expect(isWalletRejection(Object.assign(new Error('x'), { code: 4001 }))).toBe(true);
    expect(isWalletRejection(new Error('User rejected the request.'))).toBe(true);
    expect(isWalletRejection(new Error('Transaction cancelled by the user'))).toBe(true);
    expect(isWalletRejection('denied')).toBe(true);
    expect(isWalletRejection(new Error('Network request failed'))).toBe(false);
    expect(isWalletRejection(new Error('Unexpected error'))).toBe(false);
    expect(isWalletRejection(undefined)).toBe(false);
    expect(vi.isMockFunction(isWalletRejection)).toBe(false);
  });
});

describe('what counts as a reliable refusal BEFORE a wallet broadcasts', () => {
  it('accepts only the explicit user-rejection code, never message text or names', () => {
    expect(isExplicitUserRejection(Object.assign(new Error('x'), { code: 4001 }))).toBe(true);
    expect(isExplicitUserRejection(Object.assign(new Error('connection closed'), { code: 4001 }))).toBe(true);
    for (const text of ['User rejected the request.', 'connection closed', 'cancelled', 'denied', 'popup closed', 'dismissed', 'declined', 'not approved']) {
      expect(isExplicitUserRejection(new Error(text)), text).toBe(false);
    }
    expect(isExplicitUserRejection('rejected')).toBe(false);
    expect(isExplicitUserRejection(Object.assign(new Error('x'), { name: 'UserRejectedRequestError' }))).toBe(false);
    expect(isExplicitUserRejection(Object.assign(new Error('x'), { code: '4001' }))).toBe(false);
    expect(isExplicitUserRejection(Object.assign(new Error('x'), { code: -32603 }))).toBe(false);
    expect(isExplicitUserRejection(undefined)).toBe(false);
    expect(isExplicitUserRejection(null)).toBe(false);
  });
});
