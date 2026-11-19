import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount, flushSync, tick } from 'svelte';
import { Principal } from '@dfinity/principal';
const fx = vi.hoisted(() => ({ channel: vi.fn(), deposit: vi.fn(), pending: null as any, context: vi.fn() }));
vi.mock('../../stores/wallet', async () => {
  const { writable } = await import('svelte/store');
  return { walletStore: writable({ isConnected: true, principal: null, tokenBalances: {} }) };
});
vi.mock('../../services/stabilityPoolService', () => ({
  stabilityPoolService: { deposit: fx.deposit, withdraw: vi.fn() },
  captureStabilityPoolActionContext: fx.context,
  prepareStabilityPoolOisySignerChannel: fx.channel,
  readPendingStabilityPoolDeposit: () => fx.pending,
  parseTokenAmount: (amount: string | number, decimals: number) => {
    const [whole, fraction = ''] = String(amount).split('.');
    return BigInt(whole) * 10n ** BigInt(decimals) + BigInt(fraction.padEnd(decimals, '0'));
  },
}));
vi.mock('../../config', () => ({ CANISTER_IDS: {
  ICUSD_LEDGER: 'aaaaa-aa', CKUSDT_LEDGER: 'ryjl3-tyaaa-aaaaa-aaaba-cai',
  CKUSDC_LEDGER: 'mxzaz-hqaaa-aaaar-qaada-cai', THREEPOOL: 'rrkah-fqaaa-aaaaa-aaaaq-cai', STABILITY_POOL: 'r7inp-6aaaa-aaaaa-aaabq-cai',
} }));
vi.mock('../../services/protocol/walletOperations', () => ({ isOisyWallet: () => true }));
vi.mock('../../services/ledgerFeeService', () => ({ fetchLedgerFee: vi.fn().mockResolvedValue(10_000n), getCachedLedgerFee: () => 10_000n }));
vi.mock('../../services/tokenService', () => ({ TokenService: { getTokenBalance: vi.fn() } }));
vi.mock('$lib/stores/seasonStore', async () => {
  const { writable } = await import('svelte/store');
  return { seasonStore: { ensureLoaded: vi.fn() }, earningActive: writable(false) };
});
vi.mock('../../services/stabilityPoolConversion', () => ({
  conversionMinimumLp: vi.fn(), conversionWalletRequirement: vi.fn(), formatExactTokenAmount: vi.fn(),
  parseExactTokenAmount: vi.fn(), quoteStabilityPoolConversion: vi.fn(), convertStabilityPoolStableTo3Usd: vi.fn(),
  discoverActiveStabilityPoolConversion: vi.fn(), readPendingStabilityPoolConversion: () => null,
  resumePendingStabilityPoolConversion: vi.fn(),
}));
import Deposit from './DepositInterface.svelte';
import { walletStore } from '../../stores/wallet';
const owner = Principal.selfAuthenticating(Uint8Array.of(1, 2, 3));
const ledger = Principal.fromText('aaaaa-aa');
let host: HTMLDivElement;
let component: ReturnType<typeof mount>;
let open: () => void;
let reject: (error: Error) => void;
const submit = () => host.querySelector<HTMLButtonElement>('.submit-btn')!;
function render() {
  component = mount(Deposit, { target: host, props: { poolStatus: { stablecoin_registry: [{
    ledger_id: ledger, symbol: 'icUSD', decimals: 8, is_active: true, is_lp_token: false,
  }] } as any } });
  flushSync();
}
async function enterAmount(amount = '388.2514567') {
  const input = host.querySelector<HTMLInputElement>('input.amount-input')!;
  input.value = amount; input.dispatchEvent(new Event('input', { bubbles: true })); await tick();
}
beforeEach(() => {
  vi.clearAllMocks(); fx.pending = null;
  fx.context.mockImplementation((token: Principal, action: string) => ({ principalText: owner.toText(), ledgerText: token.toText(), action, oisy: true, sessionGeneration: 7, walletIcon: 'oisy' }));
  fx.channel.mockImplementation(() => new Promise<void>((resolve, fail) => { open = resolve; reject = fail; }));
  fx.deposit.mockResolvedValue(undefined);
  (walletStore as any).set({ isConnected: true, principal: owner, tokenBalances: { ICUSD: { raw: 38_825_340_000n } } });
  host = document.createElement('div'); document.body.append(host);
});
afterEach(async () => { open?.(); if (component) await unmount(component); host.remove(); });
describe('SP deposit click-time OISY channel', () => {
  it('opens from the fresh deposit click before awaiting, then preserves exact amount and context', async () => {
    render(); await enterAmount(); submit().click();
    expect(fx.channel).toHaveBeenCalledWith(owner); expect(fx.deposit).not.toHaveBeenCalled();
    const context = fx.context.mock.results.at(-1)!.value;
    open(); await vi.waitFor(() => expect(fx.deposit).toHaveBeenCalledWith(ledger, 38_825_145_670n, context));
    expect(fx.channel.mock.invocationCallOrder[0]).toBeLessThan(fx.deposit.mock.invocationCallOrder[0]);
  });
  it('opens before resuming and preserves the saved ledger and amount instead of allocating a new request', async () => {
    const lockedLedger = Principal.fromText('ryjl3-tyaaa-aaaaa-aaaba-cai');
    fx.pending = { owner: owner.toText(), ledger: lockedLedger.toText(), amount: '123456789', intentSeq: '42' };
    render(); submit().click();
    expect(fx.channel).toHaveBeenCalledWith(owner); expect(fx.deposit).not.toHaveBeenCalled();
    const context = fx.context.mock.results.at(-1)!.value;
    open(); await vi.waitFor(() => expect(fx.deposit).toHaveBeenCalledWith(lockedLedger, 123456789n, context));
    expect(fx.context).toHaveBeenCalledWith(lockedLedger, 'deposit'); expect(fx.pending.intentSeq).toBe('42');
  });
  it('does not enter the deposit service if opening the channel fails', async () => {
    render(); await enterAmount(); submit().click();
    expect(fx.channel).toHaveBeenCalledOnce(); reject(new Error('Signer window could not be opened'));
    await vi.waitFor(() => expect(host.textContent).toContain('Signer window could not be opened'));
    expect(fx.deposit).not.toHaveBeenCalled();
  });
  it('does not start a second action while the channel is opening', async () => {
    render(); await enterAmount(); submit().click(); submit().click();
    expect(fx.channel).toHaveBeenCalledOnce(); open(); await vi.waitFor(() => expect(fx.deposit).toHaveBeenCalledOnce());
  });
  it('passes the actual SDK click guard before popup creation, without opening a real wallet', async () => {
    const { PostMessageTransport } = await import('@icp-sdk/signer/web');
    const popup = vi.spyOn(window, 'open').mockReturnValue(null);
    try {
      const transport = new PostMessageTransport({ url: 'https://oisy.com/sign' });
      fx.channel.mockImplementation(() => transport.establishChannel().then(() => undefined));
      render(); await enterAmount(); submit().click();
      // Returning null deliberately stops before handshake/signing. A lost
      // click context would reject earlier and never call window.open.
      expect(popup).toHaveBeenCalledOnce();
      await vi.waitFor(() => expect(host.textContent).toContain('Signer window could not be opened'));
      expect(host.textContent).not.toContain('outside of click handler');
      expect(fx.deposit).not.toHaveBeenCalled();
    } finally { popup.mockRestore(); }
  });
  it('rejects an invalid balance before opening any channel or starting a deposit', async () => {
    render(); await enterAmount('500'); submit().click();
    await vi.waitFor(() => expect(host.textContent).toContain('Insufficient balance'));
    expect(fx.channel).not.toHaveBeenCalled(); expect(fx.deposit).not.toHaveBeenCalled();
  });
});
