/**
 * The Sui branch of the /bridge page, through the real page and the real forms.
 *
 * Everything outside the page is injected: the destination reads, the ICP signers and the anonymous reads, the Sui node
 * adapter (a fake node, or the REAL SDK adapter over a stubbed fetch), and the Wallet Standard registry (fake wallets that
 * build and sign like a real one with a test-only key). No network, no wallet and no real key is ever touched.
 */
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, tick, unmount } from 'svelte';

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
    flags: { enabled: true },
    baseId: { value: 'tfesu-vyaaa-aaaap-qrd7a-cai' },
    principal: writable<any>(null),
    generation: writable(0),
    loadBridgeConfig: vi.fn(),
    getAnonymousBridge: vi.fn(),
    createSigners: vi.fn(),
    buildPrepareDeps: vi.fn(),
    loadSuiDestination: vi.fn(),
    getAnonymousSui: vi.fn(),
    createSuiSigners: vi.fn(),
    buildSuiPrepareDeps: vi.fn(),
    getSuiRpc: vi.fn(),
    registry: { get: vi.fn(), on: vi.fn() },
    endpoints: [] as Array<{ networkId: string; chainIdentifier: string; url: string }>,
    realGetSuiRpc: null as unknown as (network: unknown) => unknown,
  };
});

vi.mock('$lib/config', () => ({
  get BRIDGE_ENABLED() {
    return mocks.flags.enabled;
  },
  CANISTER_IDS: { ICUSD_BRIDGE: 'tfesu-vyaaa-aaaap-qrd7a-cai', ICUSD_SOL_BRIDGE: '', ICUSD_SUI_BRIDGE: '' },
  CONFIG: { isLocal: false, host: 'https://icp0.io', currentIcusdLedgerId: 't6bor-paaaa-aaaap-qrd5q-cai' },
}));
vi.mock('$lib/services/baseBridge/keccak', () => ({ keccak256: null }));
vi.mock('$lib/stores/wallet', () => ({
  principal: mocks.principal,
  isConnected: { subscribe: (fn: (v: boolean) => void) => (fn(true), () => {}) },
  walletStore: { getActor: vi.fn() },
}));
vi.mock('$lib/services/auth', () => ({ walletSessionGeneration: mocks.generation }));
vi.mock('$lib/services/ledgerFeeService', () => ({ fetchLedgerFeeStrict: vi.fn() }));
vi.mock('@wallet-standard/app', () => ({ getWallets: () => mocks.registry }));
vi.mock('$lib/services/baseBridge/bridgeActor', () => ({
  getBridgeCanisterId: () => mocks.baseId.value,
  needsSeparateApprovalClick: () => false,
  loadBridgeConfig: mocks.loadBridgeConfig,
  getAnonymousBridge: mocks.getAnonymousBridge,
  createSigners: mocks.createSigners,
  buildPrepareDeps: mocks.buildPrepareDeps,
  captureSessionGuard: () => {
    const generation = mocks.generation.value;
    const text = mocks.principal.value ? mocks.principal.value.toText() : null;
    return () => text !== null && mocks.generation.value === generation && (mocks.principal.value ? mocks.principal.value.toText() : null) === text;
  },
  buildIcpToBaseDeps: () => ({}),
  buildBaseToIcpDeps: () => ({}),
}));
vi.mock('$lib/services/suiBridge/suiActor', async (importOriginal) => ({
  ...(await importOriginal<typeof import('$lib/services/suiBridge/suiActor')>()),
  loadSuiDestination: mocks.loadSuiDestination,
  getAnonymousSuiBridge: mocks.getAnonymousSui,
}));
vi.mock('$lib/services/suiBridge/endpoints', () => ({
  get SUI_NODE_ENDPOINTS() {
    return mocks.endpoints;
  },
}));
vi.mock('$lib/services/suiBridge/suiDeps', async (importOriginal) => {
  const original = await importOriginal<typeof import('$lib/services/suiBridge/suiDeps')>();
  mocks.realGetSuiRpc = original.getSuiRpc as never;
  return {
    ...original,
    createSuiSigners: mocks.createSuiSigners,
    buildSuiPrepareDeps: mocks.buildSuiPrepareDeps,
    getSuiRpc: mocks.getSuiRpc,
  };
});

import Page from './+page.svelte';
import { makeBridgeInfo, TEST_OWNER, createFakeProvider } from '$lib/services/baseBridge/testSupport';
import { resolveBridgeConfig } from '$lib/services/baseBridge/chains';
import { browserLocalStorage } from '$lib/services/baseBridge/intents';
import { SuiIntentStore, type SuiReturnIntent } from '$lib/services/suiBridge/intents';
import { SUI_SIGN_AND_EXECUTE_TRANSACTION, SUI_SIGN_TRANSACTION } from '$lib/services/suiBridge/walletStandard';
import { SUI_MAINNET } from '$lib/services/suiBridge/networks';
import { buildReturnTransaction, digestOf } from '$lib/services/suiBridge/transaction';
import { Principal } from '@dfinity/principal';
import { fromBase64 } from '@mysten/sui/utils';
import { base58Encode } from '$lib/services/solanaBridge/bytes';
import {
  COIN_TYPE,
  KEYS,
  TEST_OWNER_TEXT,
  createFakeSui,
  landOn,
  makeAccount,
  makeCandidate,
  makeObserved,
  makeRegistry,
  makeStandardWallet,
  makeSuiConfig,
  makeSuiDeposit,
  makeSuiDestination,
  makeSuiRelease,
  releaseFor,
  signLikeAWallet,
  type FakeSui,
} from '$lib/services/suiBridge/testSupport';

async function settle(rounds = 14) {
  for (let i = 0; i < rounds; i++) {
    await Promise.resolve();
    await tick();
    flushSync();
    // The Sui panel is imported lazily (so the Sui SDK is never loaded on the disabled path); give that import a turn.
    await new Promise((resolve) => (globalThis.setImmediate ?? setTimeout)(resolve as never));
  }
}

let host: HTMLDivElement;
let instance: unknown;
let node: FakeSui;

// The page imports the Sui panel (and with it the Sui SDK) lazily. Load it once up front so each test's dynamic import is a cache hit.
beforeAll(async () => {
  await import('$lib/components/bridge/SuiBridgePanel.svelte');
}, 120_000);

function render() {
  instance = mount(Page, { target: host });
  flushSync();
}

// Both direction panels stay mounted (so an in-flight transfer survives a tab switch); only the visible one is operable.
const button = (text: RegExp) => Array.from(host.querySelectorAll('button')).find((c) => !c.closest('[hidden]') && text.test(c.textContent ?? ''));
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

const anonymous = {
  get_deposit: vi.fn(),
  get_deposit_by_request: vi.fn(),
  get_release: vi.fn(),
  get_observer: vi.fn(),
  list_deposits_for_owner: vi.fn(),
  list_releases_for_owner: vi.fn(),
  list_releases_for_sui_sender: vi.fn(),
};
const signers = {
  bridge: { bridge_to_sui: vi.fn(), request_deposit_refund: vi.fn(), notify_sui_return: vi.fn() },
  ledger: { icrc2_approve: vi.fn() },
};

const hexOf = (bytes: ArrayLike<number>) => Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
let standard: ReturnType<typeof makeStandardWallet>;

function enableSui(overrides: Parameters<typeof makeSuiConfig>[0] = {}) {
  mocks.loadSuiDestination.mockResolvedValue(makeSuiDestination(makeSuiConfig(overrides)));
}

const journalKey = `rumi:sui-bridge:v1:${TEST_OWNER_TEXT}`;
const journal = () => new SuiIntentStore(browserLocalStorage(), TEST_OWNER_TEXT).list();

beforeEach(() => {
  localStorage.clear();
  sessionStorage.clear();
  host = document.createElement('div');
  document.body.appendChild(host);
  node = createFakeSui();
  standard = makeStandardWallet('Fake Wallet', [makeAccount(KEYS.holder)]);
  mocks.flags.enabled = true;
  mocks.baseId.value = 'tfesu-vyaaa-aaaap-qrd7a-cai';
  mocks.endpoints.length = 0;
  mocks.principal.set(TEST_OWNER);
  mocks.generation.set(0);
  mocks.loadBridgeConfig.mockReset().mockImplementation(async () => resolveBridgeConfig(makeBridgeInfo()));
  mocks.getAnonymousBridge.mockReset().mockResolvedValue({ get_deposit: vi.fn().mockResolvedValue([]), list_deposits_for_owner: vi.fn().mockResolvedValue([]), list_releases_for_owner: vi.fn().mockResolvedValue([]), list_releases_for_base_sender: vi.fn().mockResolvedValue([]), get_release: vi.fn().mockResolvedValue([]) });
  mocks.createSigners.mockReset().mockResolvedValue({ bridge: {}, ledger: {} });
  mocks.buildPrepareDeps.mockReset();
  mocks.loadSuiDestination.mockReset().mockResolvedValue(null);
  mocks.getAnonymousSui.mockReset().mockResolvedValue(anonymous);
  mocks.createSuiSigners.mockReset().mockResolvedValue(signers);
  mocks.getSuiRpc.mockReset().mockImplementation(() => node);
  mocks.buildSuiPrepareDeps.mockReset().mockImplementation(() => ({
    fetchConfig: async () => ({ depositFeeE8s: 10_000_000n, bridgeLedgerFeeE8s: null, limits: { minDepositE8s: 100_000_000n, maxDepositE8s: 1_000_000_000n, depositsPaused: false, halted: null } }),
    fetchLedgerFee: async () => 10_000n,
    readAllowance: async () => ({ allowance: 0n, expiresAtNs: null }),
    readBalance: async () => 5_000_000_000n,
    now: () => Date.now(),
  }));
  mocks.registry.get.mockReset().mockImplementation(() => [standard]);
  mocks.registry.on.mockReset().mockImplementation(() => () => {});
  anonymous.get_deposit.mockReset().mockResolvedValue([makeSuiDeposit({ status: { Escrowed: null } })]);
  anonymous.get_deposit_by_request.mockReset().mockResolvedValue([]);
  anonymous.get_release.mockReset().mockResolvedValue([]);
  anonymous.get_observer.mockReset().mockResolvedValue({ candidates: [] });
  anonymous.list_deposits_for_owner.mockReset().mockResolvedValue([]);
  anonymous.list_releases_for_owner.mockReset().mockResolvedValue([]);
  anonymous.list_releases_for_sui_sender.mockReset().mockResolvedValue([]);
  signers.bridge.bridge_to_sui.mockReset().mockResolvedValue({ Ok: makeSuiDeposit({ status: { Escrowed: null } }) });
  signers.bridge.request_deposit_refund.mockReset();
  signers.bridge.notify_sui_return.mockReset().mockResolvedValue({ Ok: null });
  signers.ledger.icrc2_approve.mockReset().mockResolvedValue({ Ok: 5n });
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  if (instance) unmount(instance as any);
  instance = undefined;
  host.remove();
  delete (window as Window & { ethereum?: unknown }).ethereum;
  vi.restoreAllMocks();
});

/** Not one Sui wallet call of any kind: nothing discovered, connected, signed or executed, and no node or actor made. */
function expectNoSuiActivity() {
  expect(mocks.registry.get).not.toHaveBeenCalled();
  expect(mocks.registry.on).not.toHaveBeenCalled();
  expect(standard.connects).toBe(0);
  expect(standard.signs).toHaveLength(0);
  expect(node.executed).toHaveLength(0);
  expect(mocks.getSuiRpc).not.toHaveBeenCalled();
  expect(mocks.createSuiSigners).not.toHaveBeenCalled();
}

async function selectSui() {
  const entry = option('sui-mainnet')!;
  expect(entry.tagName).toBe('BUTTON');
  entry.click();
  flushSync();
  await settle();
}

async function connectWallet() {
  const before = standard.connects;
  click(/^Connect Sui wallet$/);
  await settle();
  expect(standard.connects).toBe(before + 1);
  expect(host.textContent).toContain(standard.name);
}

async function openReturnTab() {
  enableSui();
  render();
  await settle();
  await selectSui();
  await connectWallet();
  click(/^Sui to ICP$/);
  await settle();
}

describe('Sui while it is disabled (every build today)', () => {
  it('shows a disabled row, mounts nothing and asks no wallet, whatever the user does', async () => {
    render();
    await settle();
    const entry = option('sui-mainnet')!;
    expect(entry.getAttribute('aria-disabled')).toBe('true');
    expect(entry.tagName).toBe('DIV');
    expect(entry.textContent).toContain('Unavailable');
    expect(entry.textContent).toContain('Not available yet: there is no icUSD bridge to Sui at the moment.');
    entry.click();
    entry.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true }));
    flushSync();
    await settle();
    expect(option('base-mainnet')!.getAttribute('aria-checked')).toBe('true');
    expect(host.querySelector('#tab-icp-to-sui')).toBeNull();
    expect(host.querySelector('[aria-label="Sui wallet"]')).toBeNull();
    expect(host.textContent).toContain('INTERNET COMPUTER ↔ BASE');
    expectNoSuiActivity();
    expect(signers.ledger.icrc2_approve).not.toHaveBeenCalled();
    expect(signers.bridge.bridge_to_sui).not.toHaveBeenCalled();
  });

  it('keeps a Sui instance that the canister has not set up disabled, with its reason', async () => {
    mocks.loadSuiDestination.mockResolvedValue({ ...makeSuiDestination(), status: 'disabled', reason: 'The Sui bridge is not set up yet.', native: null });
    render();
    await settle();
    expect(option('sui-mainnet')!.getAttribute('aria-disabled')).toBe('true');
    expect(option('sui-mainnet')!.textContent).toContain('The Sui bridge is not set up yet.');
    expectNoSuiActivity();
  });

  it('makes no request at all when the page is off', async () => {
    mocks.flags.enabled = false;
    render();
    await settle();
    expect(host.textContent).toContain('Bridge is not available yet');
    expect(mocks.loadBridgeConfig).not.toHaveBeenCalled();
    expect(mocks.loadSuiDestination).not.toHaveBeenCalled();
    expectNoSuiActivity();
  });
});

describe('a native-only deployment (Sui configured, Base absent or unreachable)', () => {
  it('works with NO Base instance: Sui is usable, Base is a disabled row, and no Base request or EVM wallet call is made', async () => {
    mocks.baseId.value = '';
    const provider = createFakeProvider();
    Object.defineProperty(window, 'ethereum', { configurable: true, value: provider });
    enableSui();
    render();
    await settle();
    expect(mocks.loadBridgeConfig).not.toHaveBeenCalled();
    expect(mocks.getAnonymousBridge).not.toHaveBeenCalled();
    expect(provider.calls).toHaveLength(0);
    expect(option('base-mainnet') ?? option('base-unknown')).toBeTruthy();
    const baseRow = host.querySelector<HTMLElement>('#destination-base-unknown');
    expect(baseRow?.getAttribute('aria-disabled')).toBe('true');
    expect(baseRow?.textContent).toContain('Not available yet: there is no icUSD bridge to Base at the moment.');
    // Sui is the selected destination and its panel is up.
    expect(option('sui-mainnet')!.getAttribute('aria-checked')).toBe('true');
    expect(host.querySelector('#tab-icp-to-sui')).toBeTruthy();
    expect(host.textContent).toContain('INTERNET COMPUTER ↔ SUI');
    expect(host.textContent).not.toContain('Bridge is not available yet');
    // Still no Sui wallet is touched until the user clicks Connect.
    expect(standard.connects).toBe(0);
  });

  it('works when Base is configured but UNREACHABLE: Sui stays usable and the Base row says why', async () => {
    mocks.loadBridgeConfig.mockRejectedValue(new Error('replica offline'));
    enableSui();
    render();
    await settle();
    expect(option('sui-mainnet')!.getAttribute('aria-checked')).toBe('true');
    expect(host.querySelector('#tab-icp-to-sui')).toBeTruthy();
    const baseRow = host.querySelector<HTMLElement>('#destination-base-unknown');
    expect(baseRow?.getAttribute('aria-disabled')).toBe('true');
    expect(baseRow?.textContent).toContain('The bridge could not be reached: replica offline');
  });

  it('shows a healthy Sui entry within a bounded time when the configured Base read NEVER settles, and the Base row says why', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    mocks.loadBridgeConfig.mockImplementation(() => new Promise(() => {}));
    enableSui();
    render();
    await settle();
    // Still waiting for the unfinished read: nothing is shown yet, and nothing is asked of any wallet.
    expect(host.querySelector('#tab-icp-to-sui')).toBeNull();
    expect(standard.connects).toBe(0);
    await vi.advanceTimersByTimeAsync(10_000);
    await settle();
    expect(option('sui-mainnet')!.getAttribute('aria-checked')).toBe('true');
    expect(host.querySelector('#tab-icp-to-sui')).toBeTruthy();
    const baseRow = host.querySelector<HTMLElement>('#destination-base-unknown');
    expect(baseRow?.getAttribute('aria-disabled')).toBe('true');
    expect(baseRow?.textContent).toContain('did not answer');
    expect(standard.connects).toBe(0);
  });

  it('keeps Base\'s own behavior when Base is unreachable and nothing else is configured', async () => {
    mocks.loadBridgeConfig.mockRejectedValue(new Error('replica offline'));
    render();
    await settle();
    expect(host.textContent).toContain('Bridge is not available yet');
    expect(host.textContent).toContain('replica offline');
    expect(host.querySelector('[role="tablist"]')).toBeNull();
  });

  it('keeps a failing Sui read local: Base stays usable and the Sui row says it could not be reached', async () => {
    mocks.loadSuiDestination.mockRejectedValue(new Error('sui canister down'));
    render();
    await settle();
    expect(option('base-mainnet')!.getAttribute('aria-checked')).toBe('true');
    expect(host.querySelector('#tab-icp-to-base')).toBeTruthy();
    expect(option('sui-mainnet')!.getAttribute('aria-disabled')).toBe('true');
    expectNoSuiActivity();
  });

  it('shows a configured but unreadable Sui instance as unavailable with its reason, asking no wallet', async () => {
    mocks.baseId.value = '';
    mocks.loadSuiDestination.mockResolvedValue({ ...makeSuiDestination(), status: 'disabled', reason: 'The Sui bridge could not be reached: replica offline', native: null });
    render();
    await settle();
    expect(host.textContent).toContain('The Sui bridge could not be reached: replica offline');
    expect(host.querySelector('#tab-icp-to-sui')).toBeNull();
    expectNoSuiActivity();
  });
});

describe('an enabled Sui instance (injected fixture)', () => {
  it('selects Sui, shows the Sui panel and discovers wallets without connecting any', async () => {
    enableSui();
    render();
    await settle();
    expect(option('base-mainnet')!.getAttribute('aria-checked')).toBe('true');
    await selectSui();
    expect(host.textContent).toContain('INTERNET COMPUTER ↔ SUI');
    expect(host.querySelector('.subtitle')!.textContent).toContain('between the Internet Computer and Sui.');
    expect(host.querySelector('.subtitle')!.textContent).not.toContain('Base');
    expect(host.querySelector('#tab-icp-to-sui')!.getAttribute('aria-selected')).toBe('true');
    expect(mocks.registry.get).toHaveBeenCalled();
    expect(standard.connects).toBe(0);
    expect(host.querySelector('#icp-amount')).toBeNull();
    expect(host.textContent).toContain(KEYS.packageId);
    expect(host.textContent).toContain(KEYS.gateId);
  });

  it('offers BOTH directions from the real accepted canister response, where module_digest_observed is false, and says the module bytes are not matched', async () => {
    // The accepted backend always stores false (MC-DEF-13). The page must work with exactly that response.
    expect(makeObserved().module_digest_observed).toBe(false);
    enableSui();
    render();
    await settle();
    expect(option('sui-mainnet')!.hasAttribute('aria-disabled')).toBe(false);
    await selectSui();
    expect(host.querySelector('[data-testid="module-disclosure"]')!.textContent).toContain('does not yet compare the published Gate module');
    await connectWallet();
    // ICP to Sui reviews.
    type('#sui-icp-amount', '2');
    click(/^Review transfer$/);
    await settle();
    expect(host.textContent).toContain('You receive on Sui');
    // Sui to ICP reviews.
    click(/^Sui to ICP$/);
    await settle();
    type('#sui-ret-amount', '3');
    click(/^Review transfer$/);
    await settle();
    expect(host.querySelector('#panel-sui-to-icp [aria-label="Transfer summary"]')!.textContent).toContain('Transfer id the bridge will use');
    expect(standard.signs).toHaveLength(0);
    expect(signers.ledger.icrc2_approve).not.toHaveBeenCalled();
  });

  it('shows a direction pause apart from identity, with the mint lane\'s own reasons', async () => {
    enableSui({ config: { pauses: { mints: false, observer: false, releases: true, deposits: true } } });
    render();
    await settle();
    await selectSui();
    expect(host.querySelector('[data-testid="deposits-paused"]')!.textContent).toContain('Deposits to Sui are paused');
    click(/^Sui to ICP$/);
    await settle();
    expect(host.querySelector('[data-testid="releases-paused"]')!.textContent).toContain('Releases to the Internet Computer are paused');
    // The destination itself stays available.
    expect(option('sui-mainnet')!.hasAttribute('aria-disabled')).toBe(false);
  });

  it('shows why the mint lane is not ready instead of hiding the destination', async () => {
    enableSui({ lane: { ready: false, blocked_by: ['no gas coin is configured', 'mints are paused'] } });
    render();
    await settle();
    await selectSui();
    expect(host.querySelector('[data-testid="mint-notice"]')!.textContent).toContain('no gas coin is configured; mints are paused');
    expect(option('sui-mainnet')!.hasAttribute('aria-disabled')).toBe(false);
  });

  it('connects through the Wallet Standard only when the user clicks Connect', async () => {
    enableSui();
    render();
    await settle();
    await selectSui();
    expect(host.textContent).toContain('Sui wallet not connected');
    await connectWallet();
    expect(host.querySelector('[aria-label="Sui wallet"]')!.textContent).toContain(KEYS.holder.slice(0, 6));
    expect(standard.signs).toHaveLength(0);
  });

  it('bridges ICP to Sui through the ICP signer, to the connected address, and follows the deposit to delivery from the canister\'s record', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    enableSui();
    render();
    await settle();
    await selectSui();
    await connectWallet();
    expect(host.querySelector<HTMLInputElement>('#sui-icp-recipient')!.value).toBe(KEYS.holder);
    type('#sui-icp-amount', '2');
    click(/^Review transfer$/);
    await settle();
    expect(host.textContent).toContain('You receive on Sui');
    expect(host.textContent).toContain('1.9 icUSD');
    expect(signers.ledger.icrc2_approve).not.toHaveBeenCalled();
    click(/^Approve and bridge$/);
    await settle();
    expect(signers.ledger.icrc2_approve).toHaveBeenCalledTimes(1);
    expect(signers.bridge.bridge_to_sui).toHaveBeenCalledTimes(1);
    expect(signers.bridge.bridge_to_sui.mock.calls[0][0]).toMatchObject({ amount_e8s: 200_000_000n, sui_recipient: KEYS.holder, expected_deposit_fee_e8s: 10_000_000n });
    expect(standard.signs).toHaveLength(0);
    const status = () => host.querySelector('[aria-label="Deposit status"]')!.textContent ?? '';
    expect(status()).toContain('In escrow');
    expect(journal()[0]).toMatchObject({ kind: 'icp_to_sui', status: 'submitted', recipient: KEYS.holder, destination: 'sui-mainnet', coinType: COIN_TYPE });
    anonymous.get_deposit.mockResolvedValue([makeSuiDeposit({ status: { Delivered: null }, delivered: [{ tx_digest: ['DeliveredDigest11111111111'], checkpoint: 9n, sequence: 1n }] })]);
    await vi.advanceTimersByTimeAsync(60_000);
    await settle();
    expect(status()).toContain('Delivered on Sui');
    expect(standard.signs).toHaveLength(0);
  });

  it('refuses to review a recipient that belongs to the bridge, in short form, or is the zero address', async () => {
    enableSui();
    render();
    await settle();
    await selectSui();
    for (const bad of [KEYS.packageId, KEYS.treasury, '0x2', `0x${'0'.repeat(64)}`]) {
      type('#sui-icp-recipient', bad);
      await settle();
      expect((button(/^Review transfer$/) as HTMLButtonElement).disabled, bad).toBe(true);
    }
    expect(signers.ledger.icrc2_approve).not.toHaveBeenCalled();
  });

  it('never offers to forget a bridge request that may have been submitted: the original is retried under its own id, with no second request', async () => {
    enableSui();
    render();
    await settle();
    await selectSui();
    await connectWallet();
    signers.bridge.bridge_to_sui.mockRejectedValueOnce(new Error('ingress timed out'));
    type('#sui-icp-amount', '2');
    click(/^Review transfer$/);
    await settle();
    click(/^Approve and bridge$/);
    await settle();
    expect(host.textContent).toContain('not confirmed yet');
    expect(button(/discard/i)).toBeUndefined();
    expect(journal()).toHaveLength(1);
    const originalId = journal()[0].id;
    signers.bridge.bridge_to_sui.mockImplementationOnce(async (args: { client_request_id: Uint8Array }) => ({ Ok: makeSuiDeposit({ client_request_id: args.client_request_id, status: { Escrowed: null } }) }));
    click(/^Retry with the same request$/);
    await settle();
    const ids = signers.bridge.bridge_to_sui.mock.calls.map((call) => hexOf(call[0].client_request_id));
    expect(ids).toEqual([originalId, originalId]);
    expect(signers.ledger.icrc2_approve).toHaveBeenCalledTimes(1);
    expect(journal()).toHaveLength(1);
    expect(journal()[0]).toMatchObject({ id: originalId, status: 'submitted' });
  });

  it('after a reload, an approved request whose bridge call may have started offers only the same-id continuation and blocks a fresh start', async () => {
    enableSui();
    render();
    await settle();
    await selectSui();
    await connectWallet();
    signers.bridge.bridge_to_sui.mockRejectedValueOnce(new Error('ingress timed out'));
    type('#sui-icp-amount', '2');
    click(/^Review transfer$/);
    await settle();
    click(/^Approve and bridge$/);
    await settle();
    expect(journal()[0].status).toBe('approved');
    const originalId = journal()[0].id;
    unmount(instance as any);
    host.innerHTML = '';
    render();
    await settle();
    await selectSui();
    await connectWallet();
    type('#sui-icp-amount', '3');
    click(/^Review transfer$/);
    await settle();
    click(/^Approve and bridge$/);
    await settle();
    expect(signers.ledger.icrc2_approve).toHaveBeenCalledTimes(1);
    expect(signers.bridge.bridge_to_sui).toHaveBeenCalledTimes(1);
    expect(journal()).toHaveLength(1);
    click(/^Check status$/);
    await settle();
    expect(host.textContent).toContain('Approval confirmed');
    expect(button(/discard/i)).toBeUndefined();
    signers.bridge.bridge_to_sui.mockImplementationOnce(async (args: { client_request_id: Uint8Array }) => ({ Ok: makeSuiDeposit({ client_request_id: args.client_request_id, status: { Escrowed: null } }) }));
    click(/^Bridge to Sui$/);
    await settle();
    const ids = signers.bridge.bridge_to_sui.mock.calls.map((call) => hexOf(call[0].client_request_id));
    expect(ids).toEqual([originalId, originalId]);
    expect(signers.ledger.icrc2_approve).toHaveBeenCalledTimes(1);
    expect(journal()[0]).toMatchObject({ id: originalId, status: 'submitted' });
  });
});

describe('returning from Sui (injected fixture)', () => {
  it('shows every binding line before the wallet is asked, then signs ONE transaction and broadcasts exactly those bytes', async () => {
    await openReturnTab();
    expect(host.querySelector('[data-testid="holdings"]')!.textContent).toContain('50 icUSD in 1 coin object');
    type('#sui-ret-amount', '3');
    expect(host.querySelector<HTMLInputElement>('#sui-ret-principal')!.value).toBe(TEST_OWNER_TEXT);
    click(/^Review transfer$/);
    await settle();
    const review = host.querySelector('[aria-label="Transfer summary"]')!.textContent ?? '';
    for (const line of ['Amount burned', '3 icUSD', KEYS.holder, KEYS.packageId, KEYS.gateId, COIN_TYPE, TEST_OWNER_TEXT, 'default subaccount', 'Client nonce', 'Transfer id the bridge will use', 'Gas (paid in SUI', 'you hold 2 SUI']) {
      expect(review, line).toContain(line);
    }
    const reviewedNonce = host.querySelector('[data-testid="return-nonce"]')!.textContent!;
    const reviewedId = host.querySelector('[data-testid="return-transfer-id"]')!.textContent!;
    expect(reviewedId).toMatch(/^[0-9a-f]{64}$/);
    expect(standard.signs).toHaveLength(0);
    click(/^Sign in your Sui wallet$/);
    await settle();
    expect(standard.signs).toHaveLength(1);
    expect(standard.signs[0].chain).toBe('sui:mainnet');
    expect(node.executed).toHaveLength(1);
    const saved = journal()[0] as SuiReturnIntent;
    expect(saved).toMatchObject({ status: 'sent', nonce: reviewedNonce, transferId: reviewedId, sender: KEYS.holder, signMode: 'sign' });
    expect(Array.from(node.executed[0].bytes)).toEqual(Array.from(fromBase64(saved.signedBytes!)));
    expect(saved.digest).toBe(digestOf(node.executed[0].bytes));
    expect(host.querySelector('[data-testid="tracking"]')!.textContent).toContain('Not found by this check');
    // The ICP signer was not asked to notify the bridge: that is an explicit click.
    expect(signers.bridge.notify_sui_return).not.toHaveBeenCalled();
    expect(button(/^Tell the bridge now$/)).toBeTruthy();
  });

  it('tells the bridge only on an explicit click', async () => {
    await openReturnTab();
    type('#sui-ret-amount', '3');
    click(/^Review transfer$/);
    await settle();
    click(/^Sign in your Sui wallet$/);
    await settle();
    expect(signers.bridge.notify_sui_return).not.toHaveBeenCalled();
    click(/^Tell the bridge now$/);
    await settle();
    expect(signers.bridge.notify_sui_return).toHaveBeenCalledTimes(1);
    expect(signers.bridge.notify_sui_return).toHaveBeenCalledWith((journal()[0] as SuiReturnIntent).digest);
  });

  it('keeps the ICP signer call INSIDE the "Tell the bridge now" click: no await stands between the click and notify_sui_return (popup-signer timing)', async () => {
    await openReturnTab();
    type('#sui-ret-amount', '3');
    click(/^Review transfer$/);
    await settle();
    click(/^Sign in your Sui wallet$/);
    await settle();
    const tell = button(/^Tell the bridge now$/)!;
    expect(tell).toBeTruthy();
    expect(signers.bridge.notify_sui_return).not.toHaveBeenCalled();
    // A popup signer keeps the user gesture only while the call is made synchronously from the click.
    tell.click();
    expect(signers.bridge.notify_sui_return, 'notify_sui_return must be called synchronously inside the click').toHaveBeenCalledTimes(1);
    expect(signers.bridge.notify_sui_return).toHaveBeenCalledWith((journal()[0] as SuiReturnIntent).digest);
    await settle();
    expect(host.textContent).toContain('The bridge was told.');
    expect(signers.bridge.notify_sui_return).toHaveBeenCalledTimes(1);
  });

  it('prepares the ICP signer BEFORE offering the click: a cold actor gives a readiness state, never a queued or automatic notify, even after a reload', async () => {
    await openReturnTab();
    type('#sui-ret-amount', '3');
    click(/^Review transfer$/);
    await settle();
    click(/^Sign in your Sui wallet$/);
    await settle();
    const sent = journal()[0] as SuiReturnIntent;
    // Reload with a COLD ICP actor: creating the signers does not finish until the test releases it.
    unmount(instance as any);
    host.innerHTML = '';
    let release!: () => void;
    mocks.createSuiSigners.mockImplementation(() => new Promise((resolve) => (release = () => resolve(signers))));
    render();
    await settle();
    await selectSui();
    click(/^Sui to ICP$/);
    await settle();
    expect(host.querySelector('[data-testid="tracking"]')).toBeTruthy();
    // Not ready: there is no enabled "Tell the bridge now" to click, and a click on whatever is offered signs nothing.
    expect(button(/^Tell the bridge now$/)).toBeUndefined();
    const waiting = host.querySelector<HTMLButtonElement>('[data-testid="tracking"] .actions button')!;
    expect(waiting.textContent).toContain('Preparing your ICP wallet');
    expect(waiting.disabled).toBe(true);
    waiting.click();
    flushSync();
    // Becoming ready queues nothing and notifies nobody.
    release();
    await settle();
    expect(signers.bridge.notify_sui_return).not.toHaveBeenCalled();
    // Only now is the explicit click offered, and it calls the signer synchronously.
    const tell = button(/^Tell the bridge now$/)!;
    expect(tell).toBeTruthy();
    tell.click();
    expect(signers.bridge.notify_sui_return).toHaveBeenCalledTimes(1);
    expect(signers.bridge.notify_sui_return).toHaveBeenCalledWith(sent.digest);
  });

  it('says so, and offers a distinct prepare click, when the ICP actor could not be made', async () => {
    await openReturnTab();
    type('#sui-ret-amount', '3');
    click(/^Review transfer$/);
    await settle();
    click(/^Sign in your Sui wallet$/);
    await settle();
    unmount(instance as any);
    host.innerHTML = '';
    mocks.createSuiSigners.mockRejectedValue(new Error('signer popup blocked'));
    render();
    await settle();
    await selectSui();
    click(/^Sui to ICP$/);
    await settle();
    expect(button(/^Tell the bridge now$/)).toBeUndefined();
    // The prepare click reports the problem; it signs and notifies nothing.
    click(/^Prepare ICP wallet to tell the bridge$/);
    await settle();
    expect(host.textContent).toContain('Your ICP wallet is not ready: signer popup blocked');
    expect(signers.bridge.notify_sui_return).not.toHaveBeenCalled();
    expect(button(/^Tell the bridge now$/)).toBeUndefined();
    // Once the actor can be made, the next prepare click readies it, and still nothing is queued or sent.
    mocks.createSuiSigners.mockResolvedValue(signers);
    click(/^Prepare ICP wallet to tell the bridge$/);
    await settle();
    expect(signers.bridge.notify_sui_return).not.toHaveBeenCalled();
    expect(button(/^Tell the bridge now$/)).toBeTruthy();
  });

  it('labels the return tracking card for Sui', async () => {
    await openReturnTab();
    type('#sui-ret-amount', '3');
    click(/^Review transfer$/);
    await settle();
    click(/^Sign in your Sui wallet$/);
    await settle();
    expect(host.querySelector('[data-testid="tracking"] .kicker')!.textContent).toBe('SUI RETURN');
  });

  it('follows a return from burn observed to Released from the canister records only, across a reload, with no wallet and no notify', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    await openReturnTab();
    type('#sui-ret-amount', '3');
    click(/^Review transfer$/);
    await settle();
    click(/^Sign in your Sui wallet$/);
    await settle();
    const sent = journal()[0] as SuiReturnIntent;
    landOn(node, fromBase64(sent.signedBytes!));
    // Reload: a fresh page over the same storage tracks the attempt without any wallet call.
    unmount(instance as any);
    host.innerHTML = '';
    standard.signs.length = 0;
    const connectsBefore = standard.connects;
    render();
    await settle();
    await selectSui();
    click(/^Sui to ICP$/);
    await settle();
    const label = () => host.querySelector('[data-testid="tracking"]')!.textContent ?? '';
    const step = async () => {
      await vi.advanceTimersByTimeAsync(60_000);
      await settle();
    };
    expect(label()).toContain('Final on Sui');
    anonymous.get_observer.mockResolvedValue({ candidates: [makeCandidate({ client_nonce: BigInt(sent.nonce) })] });
    await step();
    expect(label()).toContain('Seen by the bridge, verifying');
    anonymous.get_release.mockResolvedValue([releaseFor(sent, { status: { Pending: null } })]);
    await step();
    expect(label()).toContain('Burn observed, release pending');
    expect(new SuiIntentStore(browserLocalStorage(), TEST_OWNER_TEXT).get(sent.id)).toMatchObject({ status: 'sent' });
    anonymous.get_release.mockResolvedValue([releaseFor(sent, { status: { Released: null }, release_block_index: [77n], amount_to_receive_e8s: [299_980_000n] })]);
    await step();
    expect(label()).toContain('Released (ledger block 77)');
    expect(label()).toContain('2.9998 icUSD');
    expect(new SuiIntentStore(browserLocalStorage(), TEST_OWNER_TEXT).get(sent.id)).toMatchObject({ status: 'done' });
    expect(standard.signs).toHaveLength(0);
    expect(standard.connects).toBe(connectsBefore);
    expect(node.executed).toHaveLength(1);
    expect(signers.bridge.notify_sui_return).not.toHaveBeenCalled();
    expect(anonymous.get_release).toHaveBeenCalledWith(sent.transferId);
  });

  it('shows a decline in the wallet as a decline: nothing saved, nothing sent', async () => {
    await openReturnTab();
    (standard.features as Record<string, { signTransaction: () => Promise<never> }>)[SUI_SIGN_TRANSACTION].signTransaction = async () => {
      throw Object.assign(new Error('User rejected the request.'), { code: 4001 });
    };
    type('#sui-ret-amount', '3');
    click(/^Review transfer$/);
    await settle();
    click(/^Sign in your Sui wallet$/);
    await settle();
    expect(host.textContent).toContain('You declined the transfer in your wallet. Nothing was signed or sent.');
    expect(node.executed).toHaveLength(0);
    expect(journal()).toHaveLength(0);
  });

  it('refuses a transaction the wallet changed and sends nothing', async () => {
    await openReturnTab();
    standard.respond = async (transaction) => {
      const signed = await signLikeAWallet(transaction, { mutate: (t) => void t.transferObjects([t.gas], KEYS.otherHolder) });
      return { bytes: signed.bytes, signature: signed.signature };
    };
    type('#sui-ret-amount', '3');
    click(/^Review transfer$/);
    await settle();
    click(/^Sign in your Sui wallet$/);
    await settle();
    expect(host.textContent).toContain('The wallet changed the steps of the transaction.');
    expect(node.executed).toHaveLength(0);
    expect(journal()).toHaveLength(0);
  });

  async function execOnlyAttempt(error: Error) {
    standard = makeStandardWallet('Exec Only', [makeAccount(KEYS.holder)], {
      [SUI_SIGN_TRANSACTION]: undefined,
      [SUI_SIGN_AND_EXECUTE_TRANSACTION]: {
        version: '2.0.0',
        signAndExecuteTransaction: async (input: { transaction: { toJSON(): Promise<string> } }) => {
          await signLikeAWallet(input.transaction); // the wallet really executed it
          throw error;
        },
      },
    });
    await openReturnTab();
    type('#sui-ret-amount', '3');
    click(/^Review transfer$/);
    await settle();
    click(/^Sign in your Sui wallet$/);
    await settle();
  }

  it('holds an ambiguous sign-and-execute attempt (a lost connection) on screen and on disk, and a reload shows it without signing anything', async () => {
    await execOnlyAttempt(new Error('connection closed'));
    const held = host.querySelector('[data-testid="held"]')!;
    expect(held.textContent).toContain('This attempt has no recorded outcome. It is held.');
    expect(held.textContent).toContain('not rebuilt, signed again or given a new nonce');
    expect(held.textContent).toContain('cannot be told apart from one that another page is still waiting on');
    const stored = journal()[0] as SuiReturnIntent;
    expect(stored).toMatchObject({ status: 'held', signMode: 'sign_and_execute' });
    expect(stored.digest).toBeUndefined();
    expect(node.executed).toHaveLength(0);

    // Reload: the same attempt, held, with no wallet call. Evidence can never forget it.
    unmount(instance as any);
    host.innerHTML = '';
    render();
    await settle();
    await selectSui();
    await connectWallet();
    click(/^Sui to ICP$/);
    await settle();
    const signsBefore = standard.signs.length;
    expect(host.querySelector('[data-testid="held"]')).toBeTruthy();
    click(/^Check the evidence$/);
    await settle();
    expect(host.querySelector('[data-testid="evidence"]')!.textContent).toContain('Waiting for the bridge');
    expect(button(/discard/i)).toBeUndefined();
    expect(journal()).toHaveLength(1);
    expect(journal()[0]).toMatchObject({ id: stored.id, nonce: stored.nonce, transferId: stored.transferId });
    expect(standard.signs.length).toBe(signsBefore);
    // The bridge's own record settles it.
    anonymous.get_release.mockResolvedValue([releaseFor(stored, { status: { Pending: null } })]);
    click(/^Check the evidence$/);
    await settle();
    expect(host.querySelector('[data-testid="tracking"]')!.textContent).toContain('Burn observed, release pending');
    expect(anonymous.get_release).toHaveBeenLastCalledWith(stored.transferId);
  });

  it('shows a record that is NOT the reviewed attempt as a deviation with its actual facts, never as credited or done, and keeps the held original', async () => {
    const OTHER = 'rrkah-fqaaa-aaaaa-aaaaq-cai';
    standard = makeStandardWallet('Exec Only', [makeAccount(KEYS.holder)], {
      [SUI_SIGN_TRANSACTION]: undefined,
      [SUI_SIGN_AND_EXECUTE_TRANSACTION]: {
        version: '2.0.0',
        // A wallet that changes the ICP destination but keeps the sender and the nonce.
        signAndExecuteTransaction: async () => {
          const saved = journal()[0] as SuiReturnIntent;
          return signLikeAWallet(
            buildReturnTransaction({
              packageId: saved.packageId,
              gateId: saved.gateId,
              gateInitialSharedVersion: '42',
              sender: saved.sender,
              coins: saved.coins,
              amountE8s: saved.amountE8s,
              principalBytes: Principal.fromText(OTHER).toUint8Array(),
              subaccount: new Uint8Array(32),
              nonce: BigInt(saved.nonce),
            }),
          );
        },
      },
    });
    await openReturnTab();
    type('#sui-ret-amount', '3');
    click(/^Review transfer$/);
    await settle();
    click(/^Sign in your Sui wallet$/);
    await settle();
    expect(host.querySelector('[data-testid="held"]')).toBeTruthy();
    const held = journal()[0] as SuiReturnIntent;
    expect(held.status).toBe('held');
    // The canister recorded the changed operation under the same transfer id.
    anonymous.get_release.mockResolvedValue([releaseFor(held, { destination: { owner: Principal.fromText(OTHER), subaccount: [] }, status: { Released: null } })]);
    click(/^Check the evidence$/);
    await settle();
    const evidenceText = host.querySelector('[data-testid="evidence"]')!.textContent!;
    expect(evidenceText).toContain('The bridge recorded a different transfer');
    expect(evidenceText).toContain(OTHER);
    expect(host.querySelector('[data-testid="tracking"]')).toBeNull();
    expect(host.textContent).not.toContain('Released (ledger block');
    expect(button(/discard/i)).toBeUndefined();
    expect(journal()).toHaveLength(1);
    expect(journal()[0]).toMatchObject({ id: held.id, status: 'held', icpPrincipal: TEST_OWNER_TEXT, nonce: held.nonce });
    expect(standard.signs).toHaveLength(0);
  });

  it('clears an execute-only attempt only on the explicit rejection code', async () => {
    standard = makeStandardWallet('Exec Only', [makeAccount(KEYS.holder)], {
      [SUI_SIGN_TRANSACTION]: undefined,
      [SUI_SIGN_AND_EXECUTE_TRANSACTION]: {
        version: '2.0.0',
        signAndExecuteTransaction: async () => {
          throw Object.assign(new Error('closed'), { code: 4001 });
        },
      },
    });
    await openReturnTab();
    type('#sui-ret-amount', '3');
    click(/^Review transfer$/);
    await settle();
    click(/^Sign in your Sui wallet$/);
    await settle();
    expect(host.textContent).toContain('You declined the transfer in your wallet.');
    expect(journal()).toHaveLength(0);
  });

  it('blocks a transfer on the wrong Sui network, and when the wallet account changed after the review', async () => {
    standard = makeStandardWallet('Testnet Only', [makeAccount(KEYS.holder, ['sui:testnet'])], {}, ['sui:testnet']);
    enableSui();
    render();
    await settle();
    await selectSui();
    await connectWallet();
    expect(host.textContent).toContain('This wallet account is not on Sui.');
    click(/^Sui to ICP$/);
    await settle();
    type('#sui-ret-amount', '3');
    click(/^Review transfer$/);
    await settle();
    expect(host.textContent).toContain('Switch your wallet to Sui.');
    expect(standard.signs).toHaveLength(0);
    expect(node.executed).toHaveLength(0);
  });
});

describe('a saved attempt of an EARLIER Sui bridge (redeployed on the same network)', () => {
  const OTHER_INSTANCE = 'ryjl3-tyaaa-aaaaa-aaaba-cai';

  it('is never looked up, continued or settled on the bridge now in use: an old ICP request is named, not retried', async () => {
    enableSui();
    new SuiIntentStore(browserLocalStorage(), TEST_OWNER_TEXT).put({
      v: 1, kind: 'icp_to_sui', id: '66'.repeat(16), destination: 'sui-mainnet', chainIdentifier: SUI_MAINNET.chainIdentifier, instance: OTHER_INSTANCE,
      packageId: KEYS.packageId, gateId: KEYS.gateId, coinType: COIN_TYPE, recipient: KEYS.holder, amountE8s: 200_000_000n, expectedDepositFee: 10_000_000n,
      createdAt: 5, status: 'approved',
    });
    render();
    await settle();
    await selectSui();
    expect(host.textContent).toContain('Earlier request without a recorded outcome');
    click(/^Check status$/);
    await settle();
    expect(host.textContent).toContain('belongs to an earlier Sui bridge');
    expect(host.textContent).toContain(OTHER_INSTANCE);
    // The replacement canister is not asked, and nothing is approved, pulled or submitted.
    expect(anonymous.get_deposit_by_request).not.toHaveBeenCalled();
    expect(signers.bridge.bridge_to_sui).not.toHaveBeenCalled();
    expect(signers.ledger.icrc2_approve).not.toHaveBeenCalled();
    expect(button(/discard/i)).toBeUndefined();
    expect(journal()).toHaveLength(1);
    expect(journal()[0]).toMatchObject({ id: '66'.repeat(16), instance: OTHER_INSTANCE, status: 'approved' });
  });

  it('shows an old broadcast return as belonging to the earlier bridge, reads nothing from the new canister and signs nothing', async () => {
    await openReturnTab();
    type('#sui-ret-amount', '3');
    click(/^Review transfer$/);
    await settle();
    click(/^Sign in your Sui wallet$/);
    await settle();
    const sent = journal()[0] as SuiReturnIntent;
    new SuiIntentStore(browserLocalStorage(), TEST_OWNER_TEXT).patch<SuiReturnIntent>(sent.id, { instance: OTHER_INSTANCE, gateId: KEYS.mintCap });
    unmount(instance as any);
    host.innerHTML = '';
    standard.signs.length = 0;
    anonymous.get_release.mockClear();
    anonymous.get_release.mockResolvedValue([releaseFor(sent, { status: { Released: null } })]);
    render();
    await settle();
    await selectSui();
    click(/^Sui to ICP$/);
    await settle();
    const card = host.querySelector('[data-testid="tracking"]')!.textContent!;
    expect(card).toContain('Belongs to an earlier bridge');
    expect(card).toContain(OTHER_INSTANCE);
    expect(card).not.toContain('Released');
    expect(anonymous.get_release).not.toHaveBeenCalled();
    expect(button(/^Tell the bridge now$/)).toBeUndefined();
    expect(standard.signs).toHaveLength(0);
    expect(node.executed).toHaveLength(1);
    expect(journal()[0]).toMatchObject({ id: sent.id, status: 'sent', instance: OTHER_INSTANCE, nonce: sent.nonce });
  });
});

describe('the Sui node adapter on the real route', () => {
  function stubNodeFetch() {
    const log: Array<{ url: string; operation: string }> = [];
    const stub = vi.fn(async (url: string, init: { body: string }) => {
      const body = JSON.parse(init.body);
      const operation = /(?:query|mutation)\s+(\w+)/.exec(body.query)?.[1] ?? 'unknown';
      log.push({ url, operation });
      const data =
        operation === 'getChainIdentifier'
          ? { checkpoint: { digest: SUI_MAINNET.chainIdentifierBase58 } }
          : operation === 'getCoins'
            ? { address: { objects: { pageInfo: { hasNextPage: false, endCursor: null }, nodes: [{ address: KEYS.coinA, version: 7, digest: base58Encode(new Uint8Array(32).fill(0x57)), owner: { __typename: 'AddressOwner', address: { address: KEYS.holder } }, contents: { type: { repr: COIN_TYPE }, json: { balance: '5000000000' } } }] } } }
            : operation === 'getBalance'
              ? { address: { balance: { coinType: { repr: '0x2::sui::SUI' }, totalBalance: '2000000000', addressBalance: '0' } } }
              : {};
      return { ok: true, status: 200, statusText: 'OK', json: async () => ({ data }) };
    });
    vi.stubGlobal('fetch', stub);
    return { log, stub };
  }

  it('with an endpoint configured for this network and chain identifier, the REAL SDK adapter serves the return flow', async () => {
    mocks.getSuiRpc.mockImplementation((network: unknown) => mocks.realGetSuiRpc(network));
    mocks.endpoints.push({ networkId: SUI_MAINNET.networkId, chainIdentifier: SUI_MAINNET.chainIdentifier, url: 'https://sui.test.invalid/graphql' });
    const { log } = stubNodeFetch();
    await openReturnTab();
    await settle(30);
    expect(log.map((entry) => entry.operation)).toContain('getCoins');
    expect(new Set(log.map((entry) => entry.url))).toEqual(new Set(['https://sui.test.invalid/graphql']));
    expect(host.querySelector('[data-testid="holdings"]')!.textContent).toContain('50 icUSD in 1 coin object');
    type('#sui-ret-amount', '3');
    click(/^Review transfer$/);
    await settle(30);
    expect(host.querySelector('[aria-label="Transfer summary"]')).toBeTruthy();
    expect(log.map((entry) => entry.operation)).toContain('getBalance');
    expect(log.map((entry) => entry.operation)).not.toContain('executeTransaction');
    expect(standard.signs).toHaveLength(0);
  });

  it('with no endpoint configured, a return cannot be prepared and no node is ever contacted', async () => {
    mocks.getSuiRpc.mockImplementation((network: unknown) => mocks.realGetSuiRpc(network));
    const { stub } = stubNodeFetch();
    await openReturnTab();
    expect(stub).not.toHaveBeenCalled();
    expect(host.textContent).toContain('no Sui node configured');
    expect((button(/^Review transfer$/) as HTMLButtonElement).disabled).toBe(true);
    expect(standard.signs).toHaveLength(0);
  });

  it('ignores an endpoint written for another chain identifier', async () => {
    mocks.getSuiRpc.mockImplementation((network: unknown) => mocks.realGetSuiRpc(network));
    mocks.endpoints.push({ networkId: SUI_MAINNET.networkId, chainIdentifier: '4c78adacf2a2f5ad80f27ed7d54aa69d3a78f1ca67fdef9ecf5754f5b8bb77b0', url: 'https://sui.test.invalid/other' });
    const { stub } = stubNodeFetch();
    await openReturnTab();
    expect(stub).not.toHaveBeenCalled();
    expect((button(/^Review transfer$/) as HTMLButtonElement).disabled).toBe(true);
  });
});

describe('Base is unchanged by the Sui branch', () => {
  it('still defaults to Base and never touches Sui while Base is the one in use', async () => {
    enableSui();
    render();
    await settle();
    expect(option('base-mainnet')!.getAttribute('aria-checked')).toBe('true');
    expect(host.querySelector('#icp-amount')).toBeTruthy();
    expect(host.querySelector('#tab-icp-to-base')).toBeTruthy();
    expect(host.querySelector('.subtitle')!.textContent).toContain('between the Internet Computer and Base. Every icUSD on Base is backed one to one');
    expectNoSuiActivity();
    expect(localStorage.getItem(journalKey)).toBeNull();
    void landOn;
  });
});
