import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, tick, unmount } from 'svelte';
import { Principal } from '@dfinity/principal';

const ACCOUNT = '0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa';
const OTHER_IDS = ['ethereum-mainnet', 'arbitrum-mainnet', 'robinhood-mainnet', 'hyperevm-mainnet', 'monad-mainnet'];

const mocks = vi.hoisted(() => {
  function writable<T>(initial: T) {
    let value = initial;
    const listeners = new Set<(next: T) => void>();
    return {
      get value() {
        return value;
      },
      subscribe(fn: (next: T) => void) {
        fn(value);
        listeners.add(fn);
        return () => listeners.delete(fn);
      },
      set(next: T) {
        value = next;
        for (const listener of listeners) listener(next);
      },
    };
  }
  return {
    flags: { enabled: false },
    principal: writable<any>(null),
    generation: writable(0),
    loadBridgeConfig: vi.fn(),
    getAnonymousBridge: vi.fn(),
    createSigners: vi.fn(),
    buildPrepareDeps: vi.fn(),
  };
});

vi.mock('$lib/config', () => ({
  get BRIDGE_ENABLED() {
    return mocks.flags.enabled;
  },
  CANISTER_IDS: { ICUSD_BRIDGE: 'tfesu-vyaaa-aaaap-qrd7a-cai' },
  CONFIG: { isLocal: false, host: 'https://icp0.io', currentIcusdLedgerId: 't6bor-paaaa-aaaap-qrd5q-cai' },
}));
vi.mock('$lib/services/baseBridge/keccak', () => ({ keccak256: null }));
vi.mock('$lib/stores/wallet', () => ({
  principal: mocks.principal,
  isConnected: { subscribe: (fn: (v: boolean) => void) => (fn(true), () => {}) },
  walletStore: { getActor: vi.fn() },
}));
vi.mock('$lib/services/auth', () => ({ walletSessionGeneration: mocks.generation }));
vi.mock('$lib/services/baseBridge/bridgeActor', () => ({
  getBridgeCanisterId: () => 'tfesu-vyaaa-aaaap-qrd7a-cai',
  needsSeparateApprovalClick: () => false,
  loadBridgeConfig: mocks.loadBridgeConfig,
  getAnonymousBridge: mocks.getAnonymousBridge,
  createSigners: mocks.createSigners,
  buildPrepareDeps: mocks.buildPrepareDeps,
  captureSessionGuard: () => {
    const generation = mocks.generation.value;
    const text = mocks.principal.value ? mocks.principal.value.toText() : null;
    return () =>
      text !== null &&
      mocks.generation.value === generation &&
      (mocks.principal.value ? mocks.principal.value.toText() : null) === text;
  },
  buildIcpToBaseDeps: (o: any) => ({
    owner: o.principal,
    store: o.store,
    bridgeCanister: Principal.fromText('tfesu-vyaaa-aaaap-qrd7a-cai'),
    ledger: { approve: (args: unknown) => o.signers.ledger.icrc2_approve(args) },
    bridge: {
      bridgeToBase: (args: unknown) => o.signers.bridge.bridge_to_base(args),
      requestDepositRefund: (id: bigint) => o.signers.bridge.request_deposit_refund(id),
    },
    reads: {
      getDeposit: async (id: bigint) => (await o.anonymous.get_deposit(id))[0] ?? null,
      getDepositByRequest: async (owner: unknown, requestId: unknown) => (await o.anonymous.get_deposit_by_request(owner, requestId))[0] ?? null,
    },
    separateApproval: false,
    isCurrent: o.isCurrent,
  }),
  buildBaseToIcpDeps: (o: any) => ({
    provider: o.provider,
    chain: o.config.chain,
    contract: o.config.contract,
    account: o.account,
    icpPrincipal: o.principal,
    store: o.store,
    reads: { getRelease: async (id: string) => (await o.anonymous.get_release(id))[0] ?? null },
    bridge: o.signers ? { notifyBaseBridgeOut: (hash: string) => o.signers.bridge.notify_base_bridge_out(hash) } : undefined,
    isCurrent: o.isCurrent,
  }),
}));

import Page from './+page.svelte';
import IcpToBaseForm from '$lib/components/bridge/IcpToBaseForm.svelte';
import BaseToIcpForm from '$lib/components/bridge/BaseToIcpForm.svelte';
import { SELECTORS, encodeUint256 } from '$lib/services/baseBridge/abi';
import { BASE_MAINNET, resolveBridgeConfig } from '$lib/services/baseBridge/chains';
import { baseDestination, buildDestinations, type DestinationView } from '$lib/services/baseBridge/destinations';
import type { EvmWalletState } from '$lib/services/baseBridge/evmWallet';
import { IntentStore } from '$lib/services/baseBridge/intents';
import { NOT_SIGNED_NOTE } from '$lib/services/baseBridge/signingGuard';
import { TEST_OWNER, createFakeProvider, makeBridgeInfo, makeDeposit } from '$lib/services/baseBridge/testSupport';

async function settle(rounds = 14) {
  for (let i = 0; i < rounds; i++) {
    await Promise.resolve();
    await tick();
    flushSync();
  }
}

let host: HTMLDivElement;
let instance: unknown;

function render() {
  instance = mount(Page, { target: host });
  flushSync();
}

function button(text: RegExp): HTMLButtonElement | undefined {
  return Array.from(host.querySelectorAll('button')).find((candidate) => text.test(candidate.textContent ?? ''));
}

function click(text: RegExp) {
  const target = button(text);
  expect(target, `button matching ${text}`).toBeTruthy();
  expect(target!.disabled, `button ${text} should be enabled`).toBe(false);
  target!.click();
  flushSync();
}

function type(selector: string, value: string) {
  const input = host.querySelector<HTMLInputElement>(selector);
  expect(input, selector).toBeTruthy();
  input!.value = value;
  input!.dispatchEvent(new Event('input', { bubbles: true }));
  flushSync();
}

const option = (id: string) => host.querySelector<HTMLElement>(`#destination-${id}`);
const baseConfig = (info = makeBridgeInfo()) => resolveBridgeConfig(info);

const anonymous = {
  get_deposit: vi.fn(),
  get_deposit_by_request: vi.fn(),
  get_release: vi.fn(),
  list_deposits_for_owner: vi.fn(),
  list_releases_for_owner: vi.fn(),
  list_releases_for_base_sender: vi.fn(),
};
const signers = {
  bridge: { bridge_to_base: vi.fn(), request_deposit_refund: vi.fn(), notify_base_bridge_out: vi.fn() },
  ledger: { icrc2_approve: vi.fn() },
};

beforeEach(() => {
  localStorage.clear();
  sessionStorage.clear();
  host = document.createElement('div');
  document.body.appendChild(host);
  mocks.flags.enabled = true;
  mocks.principal.set(null);
  mocks.generation.set(0);
  mocks.loadBridgeConfig.mockReset().mockImplementation(async () => baseConfig());
  mocks.getAnonymousBridge.mockReset().mockResolvedValue(anonymous);
  mocks.createSigners.mockReset().mockResolvedValue(signers);
  mocks.buildPrepareDeps.mockReset().mockImplementation(() => ({
    fetchConfig: async () => ({
      depositFeeE8s: 10_000n,
      bridgeLedgerFeeE8s: 100_000n,
      limits: { minDepositE8s: 100_000_000n, maxDepositE8s: 50_000_000_000n, depositsPaused: false, halted: null },
    }),
    fetchLedgerFee: async () => 100_000n,
    readAllowance: async () => ({ allowance: 0n, expiresAtNs: null }),
    readBalance: async () => 10_000_000_000n,
    now: () => Date.now(),
  }));
  anonymous.get_deposit.mockReset().mockResolvedValue([makeDeposit({ status: { Escrowed: null } })]);
  anonymous.get_release.mockReset().mockResolvedValue([]);
  anonymous.get_deposit_by_request.mockReset().mockResolvedValue([]);
  anonymous.list_deposits_for_owner.mockReset().mockResolvedValue([]);
  anonymous.list_releases_for_owner.mockReset().mockResolvedValue([]);
  anonymous.list_releases_for_base_sender.mockReset().mockResolvedValue([]);
  signers.bridge.bridge_to_base.mockReset().mockResolvedValue({ Ok: makeDeposit({ status: { Escrowed: null } }) });
  signers.bridge.request_deposit_refund.mockReset();
  signers.bridge.notify_base_bridge_out.mockReset().mockResolvedValue({ Ok: null });
  signers.ledger.icrc2_approve.mockReset().mockResolvedValue({ Ok: 5n });
});

afterEach(() => {
  if (instance) unmount(instance as any);
  instance = undefined;
  host.remove();
  delete (window as Window & { ethereum?: unknown }).ethereum;
  vi.restoreAllMocks();
});

/** No signature was requested. (Preparing the signing actors is not a signature: a review does that ahead of the click.) */
function expectNoSignature() {
  expect(signers.ledger.icrc2_approve).not.toHaveBeenCalled();
  expect(signers.bridge.bridge_to_base).not.toHaveBeenCalled();
  expect(signers.bridge.request_deposit_refund).not.toHaveBeenCalled();
  expect(signers.bridge.notify_base_bridge_out).not.toHaveBeenCalled();
}

/** Nothing was signed and no signing actor was even prepared. */
function expectNothingSigned() {
  expect(mocks.createSigners).not.toHaveBeenCalled();
  expectNoSignature();
}

describe('the destination selector on the page', () => {
  it('lists Base as the one live choice and the five other EVM chains as unavailable, with their reasons', async () => {
    render();
    await settle();
    const group = host.querySelector('[role="radiogroup"]')!;
    expect(group.getAttribute('aria-label')).toBe('Destination network');
    expect(Array.from(group.querySelectorAll('[role="radio"]')).map((entry) => entry.id)).toEqual(
      ['base-mainnet', ...OTHER_IDS].map((id) => `destination-${id}`),
    );

    expect(option('base-mainnet')!.getAttribute('aria-checked')).toBe('true');
    expect(option('base-mainnet')!.hasAttribute('aria-disabled')).toBe(false);
    expect(option('base-mainnet')!.tagName).toBe('BUTTON');
    expect(option('base-mainnet')!.textContent).toContain('Chain 8453');
    for (const id of OTHER_IDS) {
      const entry = option(id)!;
      expect(entry.getAttribute('aria-disabled'), id).toBe('true');
      expect(entry.getAttribute('aria-checked'), id).toBe('false');
      expect(entry.tagName, id).toBe('DIV');
      expect(entry.textContent, id).toContain('Unavailable');
      expect(entry.textContent, id).toContain('Not available yet: there is no icUSD bridge to');
    }
    // The direction tabs are a different control: the selector must not be mistaken for them.
    expect(host.querySelectorAll('[role="tablist"]')).toHaveLength(1);
  });

  it('keeps the page on Base when a disabled entry is clicked, and nothing is mounted or signed for it', async () => {
    mocks.principal.set(TEST_OWNER);
    render();
    await settle();
    for (const id of OTHER_IDS) {
      const entry = option(id)!;
      entry.click();
      entry.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true }));
      entry.focus();
      flushSync();
      expect(document.activeElement, id).not.toBe(entry);
    }
    await settle();
    expect(option('base-mainnet')!.getAttribute('aria-checked')).toBe('true');
    for (const id of OTHER_IDS) expect(option(id)!.getAttribute('aria-checked'), id).toBe('false');
    expect(host.querySelector('#icp-amount')).toBeTruthy();
    expect(host.textContent).toContain('Bridge icUSD');
    expectNothingSigned();
  });

  it('moves with the arrow keys only between enabled entries, so the keyboard cannot reach a disabled one', async () => {
    render();
    await settle();
    const group = host.querySelector<HTMLElement>('[role="radiogroup"]')!;
    for (const name of ['ArrowRight', 'ArrowDown', 'ArrowLeft', 'ArrowUp', 'End', 'Home']) {
      group.dispatchEvent(new KeyboardEvent('keydown', { key: name, bubbles: true, cancelable: true }));
      flushSync();
      expect(option('base-mainnet')!.getAttribute('aria-checked'), name).toBe('true');
      expect(document.activeElement, name).toBe(option('base-mainnet'));
    }
  });

  it('shows every destination unavailable, and none selectable, when the bridge cannot be reached', async () => {
    mocks.loadBridgeConfig.mockRejectedValue(new Error('replica offline'));
    render();
    await settle();
    expect(host.textContent).toContain('Bridge is not available yet');
    expect(host.textContent).toContain('The bridge could not be reached: replica offline');
    const radios = Array.from(host.querySelectorAll('[role="radio"]'));
    expect(radios).toHaveLength(6);
    expect(radios.every((entry) => entry.getAttribute('aria-disabled') === 'true')).toBe(true);
    expect(host.querySelectorAll('button[role="radio"]')).toHaveLength(0);
    expect(host.querySelector('[role="tablist"]')).toBeNull();
    expect(host.querySelector('#icp-amount')).toBeNull();
  });

  it('shows no selector, and calls nothing, while the bridge is not configured', async () => {
    mocks.flags.enabled = false;
    render();
    await settle();
    expect(host.textContent).toContain('Bridge is not available yet');
    expect(host.querySelector('[role="radiogroup"]')).toBeNull();
    expect(mocks.loadBridgeConfig).not.toHaveBeenCalled();
  });
});

describe('a destination that is disabled', () => {
  it('mounts no form, wallet bar or review panel, and signs nothing', async () => {
    mocks.principal.set(TEST_OWNER);
    mocks.loadBridgeConfig.mockResolvedValue(baseConfig(makeBridgeInfo({ contract: [] })));
    const provider = createFakeProvider();
    Object.defineProperty(window, 'ethereum', { configurable: true, value: provider });
    render();
    await settle();

    expect(host.textContent).toContain('Bridge is not available yet');
    expect(host.textContent).toContain('The Base contract is not bound yet.');
    expect(option('base-mainnet')!.getAttribute('aria-disabled')).toBe('true');
    expect(option('base-mainnet')!.tagName).toBe('DIV');
    expect(host.querySelector('[role="tablist"]')).toBeNull();
    expect(host.querySelector('#icp-amount')).toBeNull();
    expect(host.querySelector('#base-amount')).toBeNull();
    expect(host.querySelector('[aria-label="Base wallet"]')).toBeNull();
    expect(host.querySelector('[aria-label="Transfer summary"]')).toBeNull();
    expectNothingSigned();
    expect(provider.callsTo('eth_sendTransaction')).toHaveLength(0);
    expect(provider.callsTo('eth_requestAccounts')).toHaveLength(0);
  });
});

describe('the 60 second refresh turns the selected destination off', () => {
  /** The page's refresh timer, captured so the test can fire it without waiting a minute. */
  function captureRefreshTimer(): () => Promise<void> {
    let tick60: (() => Promise<void>) | null = null;
    const real = globalThis.setInterval.bind(globalThis);
    const spy = (handler: TimerHandler, delay?: number, ...rest: unknown[]) => {
      if (delay === 60_000) {
        tick60 = handler as () => Promise<void>;
        return 0;
      }
      return (real as (...args: unknown[]) => unknown)(handler, delay, ...rest);
    };
    vi.spyOn(globalThis, 'setInterval').mockImplementation(spy as unknown as typeof setInterval);
    return () => {
      expect(tick60, 'the page should poll every 60 seconds').toBeTruthy();
      return tick60!();
    };
  }

  async function openReview() {
    mocks.principal.set(TEST_OWNER);
    const fire = captureRefreshTimer();
    render();
    await settle();
    type('#icp-amount', '25');
    type('#icp-recipient', '0x1111111111111111111111111111111111111111');
    click(/^Review transfer$/);
    await settle();
    expect(host.querySelector('[aria-label="Transfer summary"]')).toBeTruthy();
    expect(button(/Approve and bridge/)).toBeTruthy();
    return fire;
  }

  it('returns an open review to the unavailable state, and nothing is signed', async () => {
    const fire = await openReview();
    mocks.loadBridgeConfig.mockResolvedValue(baseConfig(makeBridgeInfo({ contract: [] })));
    await fire();
    await settle();

    expect(host.textContent).toContain('Bridge is not available yet');
    expect(host.textContent).toContain('The Base contract is not bound yet.');
    expect(host.querySelector('[aria-label="Transfer summary"]')).toBeNull();
    expect(button(/Approve and bridge/)).toBeUndefined();
    expect(host.querySelector('#icp-amount')).toBeNull();
    expect(host.querySelector('[role="tablist"]')).toBeNull();
    expect(option('base-mainnet')!.getAttribute('aria-disabled')).toBe('true');
    expect(signers.ledger.icrc2_approve).not.toHaveBeenCalled();
    expect(signers.bridge.bridge_to_base).not.toHaveBeenCalled();
  });

  it('re-reads the status on "Try again", and brings the forms back, in a fresh draft, once it is enabled again', async () => {
    const fire = await openReview();
    mocks.loadBridgeConfig.mockResolvedValue(baseConfig(makeBridgeInfo({ contract: [] })));
    await fire();
    await settle();
    const reads = mocks.loadBridgeConfig.mock.calls.length;

    // Still off: the same state, one more read.
    click(/^Try again$/);
    await settle();
    expect(mocks.loadBridgeConfig.mock.calls.length).toBe(reads + 1);
    expect(host.querySelector('#icp-amount')).toBeNull();

    // Back on.
    mocks.loadBridgeConfig.mockImplementation(async () => baseConfig());
    click(/^Try again$/);
    await settle();
    expect(host.querySelector('#icp-amount')).toBeTruthy();
    expect(host.querySelector('[aria-label="Transfer summary"]')).toBeNull();
    expect(host.querySelector<HTMLInputElement>('#icp-amount')!.value).toBe('');
    expect(option('base-mainnet')!.getAttribute('aria-disabled')).toBeNull();
    expectNoSignature();
  });

  it('keeps the last good list through a failed refresh, as the page always did', async () => {
    const fire = await openReview();
    mocks.loadBridgeConfig.mockRejectedValue(new Error('replica offline'));
    await fire();
    await settle();
    expect(host.querySelector('[aria-label="Transfer summary"]')).toBeTruthy();
    expect(option('base-mainnet')!.getAttribute('aria-disabled')).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// Paused is not disabled: Base behaves as it always has
// ---------------------------------------------------------------------------

describe('a PAUSED Base keeps the behavior it always had', () => {
  const NONCE_TRANSFER = `0x${'ee'.repeat(32)}`;

  it('Base to ICP with releases paused: the held-delivery warning shows and sending is still offered', async () => {
    mocks.principal.set(TEST_OWNER);
    mocks.loadBridgeConfig.mockResolvedValue(baseConfig(makeBridgeInfo({ pauses: { mints: false, observer: false, releases: true, deposits: false } })));
    const sent: unknown[] = [];
    let used = false;
    const provider = createFakeProvider({
      chainId: 8453,
      handlers: {
        eth_call: (params) => {
          const data = (params as [{ data: string }])[0].data;
          if (data.startsWith(SELECTORS.balanceOf)) return `0x${encodeUint256(1_000_000_000n)}`;
          if (data.startsWith(SELECTORS.computeTransferId)) return NONCE_TRANSFER;
          if (data.startsWith(SELECTORS.bridgeOutUsed)) return `0x${encodeUint256(used ? 1n : 0n)}`;
          throw new Error('unexpected eth_call');
        },
        eth_getBalance: () => `0x${(10n ** 16n).toString(16)}`,
        eth_estimateGas: () => '0x13880',
        eth_gasPrice: () => '0xf4240',
        eth_sendTransaction: (params) => {
          sent.push((params as unknown[])[0]);
          used = true;
          return `0x${'12'.repeat(32)}`;
        },
        eth_getTransactionReceipt: () => null,
      },
    });
    Object.defineProperty(window, 'ethereum', { configurable: true, value: provider });
    render();
    await new Promise((resolve) => setTimeout(resolve, 170));
    await settle(24);
    host.querySelector<HTMLButtonElement>('#tab-base-to-icp')!.click();
    flushSync();
    await settle(24);

    const panel = host.querySelector<HTMLElement>('#panel-base-to-icp')!;
    expect(option('base-mainnet')!.hasAttribute('aria-disabled')).toBe(false);
    expect(panel.textContent).toContain('Releases to the Internet Computer are paused right now. A transfer sent now would wait until releases resume.');
    type('#base-amount', '2.5');
    await new Promise((resolve) => setTimeout(resolve, 320));
    await settle(24);
    const review = Array.from(panel.querySelectorAll('button')).find((b) => /^Review transfer$/.test(b.textContent ?? ''))!;
    expect(review.disabled).toBe(false);
    review.click();
    await settle();
    const send = Array.from(panel.querySelectorAll('button')).find((b) => /Burn on Base and bridge/.test(b.textContent ?? ''))!;
    expect(send.disabled).toBe(false);
    send.click();
    await settle(30);
    expect(sent).toHaveLength(1);
  });

  it('ICP to Base with deposits paused: the existing banner and blocked review button, nothing else', async () => {
    mocks.principal.set(TEST_OWNER);
    mocks.loadBridgeConfig.mockResolvedValue(baseConfig(makeBridgeInfo({ pauses: { mints: false, observer: false, releases: false, deposits: true } })));
    render();
    await settle();
    expect(option('base-mainnet')!.hasAttribute('aria-disabled')).toBe(false);
    expect(host.textContent).toContain('Deposits to Base are paused right now. Your icUSD has not moved.');
    type('#icp-amount', '25');
    type('#icp-recipient', '0x1111111111111111111111111111111111111111');
    expect(button(/^Review transfer$/)!.disabled).toBe(true);
    expectNothingSigned();
  });
});

// ---------------------------------------------------------------------------
// The forms themselves: a stale destination is re-read, and nothing is signed
// ---------------------------------------------------------------------------

describe('forms refuse a destination the newest read has turned off', () => {
  const ME = TEST_OWNER.toText();
  const noWallet: EvmWalletState = { status: 'unavailable', wallets: [], walletId: null, walletName: null, account: null, chainId: null, error: null };

  function viewFrom(info = makeBridgeInfo()): DestinationView {
    return buildDestinations(baseConfig(info), 'tfesu-vyaaa-aaaap-qrd7a-cai')[0];
  }
  const turnedOff = (): DestinationView =>
    baseDestination({ ok: false, reason: 'contract_not_bound', message: 'The Base contract is not bound yet.', chain: BASE_MAINNET }, 'tfesu-vyaaa-aaaap-qrd7a-cai');

  it('ICP to Base: Confirm asks the page to read again and never reaches the ledger or the bridge', async () => {
    mocks.principal.set(TEST_OWNER);
    const config = baseConfig();
    if (!config.ok) throw new Error('unreachable');
    const onRecheck = vi.fn();
    const store = new IntentStore(localStorage, ME);
    instance = mount(IcpToBaseForm, {
      target: host,
      props: {
        config: config.config,
        destination: viewFrom(),
        readFreshDestination: () => turnedOff(),
        onRecheck,
        wallet: noWallet,
        principal: TEST_OWNER,
        store,
      },
    });
    flushSync();
    await settle();
    type('#icp-amount', '25');
    type('#icp-recipient', '0x1111111111111111111111111111111111111111');
    click(/^Review transfer$/);
    await settle();
    expect(button(/Approve and bridge/)).toBeTruthy();

    click(/Approve and bridge/);
    await settle();
    expect(onRecheck).toHaveBeenCalled();
    expect(signers.ledger.icrc2_approve).not.toHaveBeenCalled();
    expect(signers.bridge.bridge_to_base).not.toHaveBeenCalled();
    expect(host.textContent).toContain('The Base contract is not bound yet.');
    expect(host.textContent).toContain(NOT_SIGNED_NOTE);
    expect(store.list()).toEqual([]);
  });

  it('Base to ICP: Send asks the page to read again and the wallet gets no further call', async () => {
    mocks.principal.set(TEST_OWNER);
    const config = baseConfig();
    if (!config.ok) throw new Error('unreachable');
    const onRecheck = vi.fn();
    const store = new IntentStore(localStorage, ME);
    const provider = createFakeProvider({
      chainId: 8453,
      handlers: {
        eth_call: (params) => {
          const data = (params as [{ data: string }])[0].data;
          if (data.startsWith(SELECTORS.balanceOf)) return `0x${encodeUint256(1_000_000_000n)}`;
          throw new Error('unexpected eth_call');
        },
        eth_getBalance: () => `0x${(10n ** 16n).toString(16)}`,
        eth_estimateGas: () => '0x13880',
        eth_gasPrice: () => '0xf4240',
      },
    });
    const connected: EvmWalletState = { ...noWallet, status: 'connected', account: ACCOUNT, chainId: 8453 };
    instance = mount(BaseToIcpForm, {
      target: host,
      props: {
        config: config.config,
        destination: viewFrom(),
        readFreshDestination: () => turnedOff(),
        onRecheck,
        wallet: connected,
        getProvider: () => provider,
        principal: TEST_OWNER,
        store,
        onConnect: () => {},
        onSwitch: () => {},
      },
    });
    flushSync();
    await settle();
    type('#base-amount', '2.5');
    await new Promise((resolve) => setTimeout(resolve, 320));
    await settle(24);
    click(/^Review transfer$/);
    await settle();
    const callsBefore = provider.calls.length;

    click(/Burn on Base and bridge/);
    await settle(24);
    expect(onRecheck).toHaveBeenCalled();
    expect(provider.calls.length).toBe(callsBefore);
    expect(provider.callsTo('eth_sendTransaction')).toHaveLength(0);
    expect(host.textContent).toContain(NOT_SIGNED_NOTE);
    expect(store.list()).toEqual([]);
  });
});
