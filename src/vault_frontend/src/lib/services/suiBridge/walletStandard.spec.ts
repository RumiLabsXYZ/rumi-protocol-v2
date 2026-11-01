import type { WalletAccount } from '@wallet-standard/base';
import { get } from 'svelte/store';
import { describe, expect, it, vi } from 'vitest';
import { makeAccount as account, makeRegistry as registry, makeStandardWallet as wallet, KEYS, signLikeAWallet } from './testSupport';
import {
  STANDARD_CONNECT,
  STANDARD_EVENTS,
  SUI_SIGN_AND_EXECUTE_TRANSACTION,
  SUI_SIGN_TRANSACTION,
  createSuiWalletStore,
  isExplicitUserRejection,
  isUsableSuiWallet,
  isWalletRejection,
} from './walletStandard';
import { buildReturnTransaction } from './transaction';
import { Principal } from '@dfinity/principal';

const sample = () =>
  buildReturnTransaction({
    packageId: KEYS.packageId,
    gateId: KEYS.gateId,
    gateInitialSharedVersion: '42',
    sender: KEYS.holder,
    coins: [{ objectId: KEYS.coinA, version: '7', digest: '4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQff4P3bkLKi', balance: 5n }],
    amountE8s: 5n,
    principalBytes: Principal.fromText('rrkah-fqaaa-aaaaa-aaaaq-cai').toUint8Array(),
    subaccount: new Uint8Array(32),
    nonce: 1n,
  });

describe('which wallets are usable', () => {
  it('needs connect, events, a Sui chain and a Sui signing feature', () => {
    expect(isUsableSuiWallet(wallet('ok', [account('0xA')]))).toBe(true);
    expect(isUsableSuiWallet(wallet('execute only', [account('0xA')], { [SUI_SIGN_TRANSACTION]: undefined, [SUI_SIGN_AND_EXECUTE_TRANSACTION]: { signAndExecuteTransaction: async () => ({}) } }))).toBe(true);
    expect(isUsableSuiWallet(wallet('no signer', [account('0xA')], { [SUI_SIGN_TRANSACTION]: undefined }))).toBe(false);
    expect(isUsableSuiWallet(wallet('solana only', [account('0xA')], {}, ['solana:mainnet']))).toBe(false);
    expect(isUsableSuiWallet(wallet('no events', [account('0xA')], { [STANDARD_EVENTS]: undefined }))).toBe(false);
  });
});

describe('the Sui wallet store', () => {
  it('discovers wallets without asking any to connect', () => {
    const w = wallet('Fake', [account('0xA')]);
    const reg = registry([w, wallet('Solana', [account('Addr')], {}, ['solana:mainnet'])]);
    const store = createSuiWalletStore({ registry: () => reg });
    expect(get(store).status).toBe('unavailable');
    store.init();
    expect(get(store)).toMatchObject({ status: 'disconnected', wallets: [{ name: 'Fake' }], address: null });
    expect(w.connects).toBe(0);
    expect(store.session()).toBeNull();
    store.destroy();
  });

  it('picks up a wallet that registers later, and drops one that goes away', () => {
    const reg = registry([]);
    const store = createSuiWalletStore({ registry: () => reg });
    store.init();
    expect(get(store).status).toBe('unavailable');
    const w = wallet('Late', [account('0xA')]);
    reg.add(w);
    expect(get(store)).toMatchObject({ status: 'disconnected', wallets: [{ name: 'Late' }] });
    reg.remove(w);
    expect(get(store).status).toBe('unavailable');
  });

  it('connects on request: one account, its chains and what the wallet can sign, handing the wallet the SDK transaction', async () => {
    const w = wallet('Fake', [account(KEYS.holder, ['sui:mainnet', 'sui:testnet'])]);
    const store = createSuiWalletStore({ registry: () => registry([w]) });
    store.init();
    await store.connect();
    expect(w.connects).toBe(1);
    expect(get(store)).toMatchObject({ status: 'connected', walletName: 'Fake', address: KEYS.holder, chains: ['sui:mainnet', 'sui:testnet'], canSign: true, canSignAndExecute: false, error: null });
    const session = store.session()!;
    const signed = await session.signTransaction(sample(), 'sui:mainnet');
    expect(typeof signed.bytes).toBe('string');
    expect(typeof signed.signature).toBe('string');
    expect(w.signs).toHaveLength(1);
    expect(w.signs[0].chain).toBe('sui:mainnet');
    expect(JSON.parse(w.signs[0].json)).toBeTruthy();
    await expect(session.signAndExecuteTransaction(sample(), 'sui:mainnet')).rejects.toThrow(/cannot sign and execute/);
  });

  it('asks the user to choose when several wallets exist, and connects only the chosen one', async () => {
    const a = wallet('A', [account('0xA')]);
    const b = wallet('B', [account('0xB')]);
    const store = createSuiWalletStore({ registry: () => registry([a, b]) });
    store.init();
    await store.connect();
    expect(get(store).error).toBe('Choose a wallet to connect.');
    expect([a.connects, b.connects]).toEqual([0, 0]);
    await store.connect('B');
    expect([a.connects, b.connects]).toEqual([0, 1]);
    expect(get(store).address).toBe('0xB');
  });

  it('reports a failed or empty connection without leaving a connected state', async () => {
    const failing = wallet('Failing', [account('0xA')]);
    (failing.features as Record<string, { connect: () => Promise<unknown> }>)[STANDARD_CONNECT].connect = async () => {
      throw new Error('User rejected the request.');
    };
    const store = createSuiWalletStore({ registry: () => registry([failing]) });
    store.init();
    await store.connect();
    expect(get(store)).toMatchObject({ status: 'disconnected', address: null, error: 'User rejected the request.' });
    const empty = wallet('Empty', []);
    const store2 = createSuiWalletStore({ registry: () => registry([empty]) });
    store2.init();
    await store2.connect();
    expect(get(store2)).toMatchObject({ status: 'disconnected', address: null });
    expect(get(store2).error).toContain('no account');
  });

  it('follows the wallet: an account switch updates the session, a disconnect clears it', async () => {
    const w = wallet('Fake', [account('0xA1'), account('0xA2')]);
    const store = createSuiWalletStore({ registry: () => registry([w]) });
    store.init();
    await store.connect();
    expect(get(store).address).toBe('0xA1');
    w.emit({ accounts: [account('0xA2'), account('0xA1')] });
    expect(get(store).address).toBe('0xA1');
    w.emit({ accounts: [account('0xA3', ['sui:testnet'])] });
    expect(get(store)).toMatchObject({ address: '0xA3', chains: ['sui:testnet'] });
    expect(store.session()?.address).toBe('0xA3');
    w.emit({ accounts: [] });
    expect(get(store)).toMatchObject({ status: 'disconnected', address: null });
    expect(store.session()).toBeNull();
    expect(w.offs).toBeGreaterThan(0);
  });

  it('disconnects and stops listening when destroyed', async () => {
    const w = wallet('Fake', [account('0xA1')]);
    const reg = registry([w]);
    const store = createSuiWalletStore({ registry: () => reg });
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
    const slow = wallet('Slow', [account('0xOld')]);
    (slow.features as Record<string, { connect: () => Promise<unknown> }>)[STANDARD_CONNECT].connect = () => new Promise((resolve) => (release = resolve as typeof release));
    const store = createSuiWalletStore({ registry: () => registry([slow]) });
    store.init();
    const first = store.connect();
    await store.disconnect();
    release({ accounts: [account('0xOld')] });
    await first;
    expect(get(store)).toMatchObject({ status: 'disconnected', address: null });
  });
});

describe('the wallet answers the page relies on', () => {
  it('refuses an answer without the fields it needs', async () => {
    const w = wallet('Odd', [account(KEYS.holder)], { [SUI_SIGN_TRANSACTION]: { version: '2.0.0', signTransaction: async () => ({ bytes: 5 }) } });
    const store = createSuiWalletStore({ registry: () => registry([w]) });
    store.init();
    await store.connect();
    await expect(store.session()!.signTransaction(sample(), 'sui:mainnet')).rejects.toThrow('no signed transaction');
  });

  it('builds and signs like a real wallet for the fakes the other specs use', async () => {
    const signed = await signLikeAWallet(sample());
    expect(signed.bytes.length).toBeGreaterThan(100);
    expect(signed.digest).toMatch(/^[1-9A-HJ-NP-Za-km-z]{43,44}$/);
  });
});

describe('what counts as the user saying no', () => {
  it('recognizes a rejection by code, name or message and nothing else (a hint only)', () => {
    expect(isWalletRejection(Object.assign(new Error('x'), { code: 4001 }))).toBe(true);
    expect(isWalletRejection(new Error('User rejected the request.'))).toBe(true);
    expect(isWalletRejection(new Error('Transaction cancelled by the user'))).toBe(true);
    expect(isWalletRejection('denied')).toBe(true);
    expect(isWalletRejection(new Error('Network request failed'))).toBe(false);
    expect(isWalletRejection(undefined)).toBe(false);
    expect(vi.isMockFunction(isWalletRejection)).toBe(false);
  });
});

describe('what counts as a reliable refusal BEFORE a wallet executes', () => {
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
