import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, tick, unmount } from 'svelte';
import { Principal } from '@dfinity/principal';

const BRIDGE_ID = 'tfesu-vyaaa-aaaap-qrd7a-cai';
const ACCOUNT = '0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa';

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

vi.mock('$lib/stores/wallet', () => ({
  principal: mocks.principal,
  isConnected: { subscribe: (fn: (v: boolean) => void) => (fn(true), () => {}) },
  walletStore: { getActor: vi.fn() },
}));

vi.mock('$lib/services/auth', () => ({ walletSessionGeneration: mocks.generation }));

vi.mock('$lib/services/baseBridge/bridgeActor', () => ({
  getBridgeCanisterId: () => 'tfesu-vyaaa-aaaap-qrd7a-cai',
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
import { resolveBridgeConfig } from '$lib/services/baseBridge/chains';
import { SELECTORS, encodeUint256 } from '$lib/services/baseBridge/abi';
import { createFakeProvider, makeBridgeInfo, makeDeposit, makeRelease, walletError, TEST_OWNER } from '$lib/services/baseBridge/testSupport';
import { IntentStore } from '$lib/services/baseBridge/intents';

async function settle(rounds = 14) {
  for (let i = 0; i < rounds; i++) {
    await Promise.resolve();
    await tick();
    flushSync();
  }
}

/** EIP-6963 discovery waits ~120ms for wallets to announce themselves. */
async function waitForWalletDiscovery() {
  await new Promise((resolve) => setTimeout(resolve, 170));
  await settle(24);
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

const anonymous = {
  get_deposit: vi.fn(),
  get_deposit_by_request: vi.fn(),
  get_release: vi.fn(),
  list_deposits_for_owner: vi.fn(),
  list_releases_for_owner: vi.fn(),
  list_releases_for_base_sender: vi.fn(),
};
const signers = {
  bridge: {
    bridge_to_base: vi.fn(),
    list_my_deposits: vi.fn(),
    request_deposit_refund: vi.fn(),
    notify_base_bridge_out: vi.fn(),
  },
  ledger: { icrc2_approve: vi.fn() },
};

beforeEach(() => {
  localStorage.clear();
  sessionStorage.clear();
  host = document.createElement('div');
  document.body.appendChild(host);
  mocks.flags.enabled = false;
  mocks.principal.set(null);
  mocks.generation.set(0);
  mocks.loadBridgeConfig.mockReset().mockImplementation(async () => resolveBridgeConfig(makeBridgeInfo()));
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
  anonymous.get_deposit.mockReset().mockResolvedValue([makeDeposit({ status: { Escrowed: null }, client_request_id: new Uint8Array(16).fill(0xcd) })]);
  anonymous.get_release.mockReset().mockResolvedValue([]);
  anonymous.get_deposit_by_request.mockReset().mockResolvedValue([]);
  anonymous.list_deposits_for_owner.mockReset().mockResolvedValue([]);
  anonymous.list_releases_for_owner.mockReset().mockResolvedValue([]);
  anonymous.list_releases_for_base_sender.mockReset().mockResolvedValue([]);
  signers.bridge.bridge_to_base.mockReset().mockResolvedValue({ Ok: makeDeposit({ status: { Escrowed: null }, client_request_id: new Uint8Array(16).fill(0xcd) }) });
  signers.bridge.list_my_deposits.mockReset().mockResolvedValue([]);
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

describe('/bridge when the bridge canister id is not configured', () => {
  it('says the bridge is not available and makes no calls at all', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    const provider = createFakeProvider();
    Object.defineProperty(window, 'ethereum', { configurable: true, value: provider });
    mocks.principal.set(TEST_OWNER);
    render();
    await settle();

    expect(host.textContent).toContain('Bridge is not available yet');
    expect(host.querySelector('[role="tablist"]')).toBeNull();
    expect(mocks.loadBridgeConfig).not.toHaveBeenCalled();
    expect(mocks.getAnonymousBridge).not.toHaveBeenCalled();
    expect(mocks.createSigners).not.toHaveBeenCalled();
    expect(anonymous.list_deposits_for_owner).not.toHaveBeenCalled();
    expect(provider.calls).toHaveLength(0);
    expect(fetchSpy).not.toHaveBeenCalled();
  });
});

describe('/bridge when enabled', () => {
  beforeEach(() => {
    mocks.flags.enabled = true;
  });

  it('loads the live configuration and shows the chain and contract from get_bridge_info', async () => {
    render();
    await settle();
    expect(mocks.loadBridgeConfig).toHaveBeenCalledTimes(1);
    expect(host.textContent).toContain('Bridge icUSD');
    expect(host.querySelector('[role="tablist"]')).toBeTruthy();
    expect(host.textContent).toContain('Base');
    expect(host.textContent).toContain('0x2222222222222222222222222222222222222222');
    expect(host.textContent).toContain('chain 8453');
    expect(host.textContent).toContain('Connect your ICP wallet');
  });

  it('shows an unavailable state when the contract is not bound yet', async () => {
    mocks.loadBridgeConfig.mockResolvedValue(resolveBridgeConfig(makeBridgeInfo({ contract: [] })));
    render();
    await settle();
    expect(host.textContent).toContain('Bridge is not available yet');
    expect(host.textContent).toContain('not bound');
    expect(host.querySelector('[role="tablist"]')).toBeNull();
  });

  it('shows an unavailable state when the canister cannot be reached', async () => {
    mocks.loadBridgeConfig.mockRejectedValue(new Error('replica offline'));
    render();
    await settle();
    expect(host.textContent).toContain('Bridge is not available yet');
    expect(host.textContent).toContain('replica offline');
  });

  it('switches between the two directions with accessible tabs', async () => {
    render();
    await settle();
    const icpTab = host.querySelector<HTMLButtonElement>('#tab-icp-to-base')!;
    const baseTab = host.querySelector<HTMLButtonElement>('#tab-base-to-icp')!;
    expect(icpTab.getAttribute('aria-selected')).toBe('true');
    expect(host.querySelector<HTMLElement>('#panel-base-to-icp')!.hidden).toBe(true);
    baseTab.click();
    flushSync();
    expect(baseTab.getAttribute('aria-selected')).toBe('true');
    expect(host.querySelector<HTMLElement>('#panel-icp-to-base')!.hidden).toBe(true);
    expect(host.querySelector<HTMLElement>('#panel-base-to-icp')!.hidden).toBe(false);
  });
});

describe('/bridge history and keyboard support', () => {
  beforeEach(() => {
    mocks.flags.enabled = true;
  });

  it('moves between the direction tabs with the arrow keys', async () => {
    render();
    await settle();
    const list = host.querySelector<HTMLElement>('[role="tablist"]')!;
    list.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }));
    flushSync();
    expect(host.querySelector('#tab-base-to-icp')!.getAttribute('aria-selected')).toBe('true');
    list.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }));
    flushSync();
    expect(host.querySelector('#tab-icp-to-base')!.getAttribute('aria-selected')).toBe('true');
  });

  it('lists deposits and releases from public queries with their canister statuses', async () => {
    mocks.principal.set(TEST_OWNER);
    anonymous.list_deposits_for_owner.mockResolvedValue([
      makeDeposit({ id: 3n, status: { Delivered: null }, delivered: [{ block_number: 9n, block_hash: '0x1', tx_hash: `0x${'34'.repeat(32)}` }] }),
      makeDeposit({ id: 2n, status: { MintHeld: { reason: 'paused' } } }),
      makeDeposit({ id: 1n, status: { PullFailed: { reason: 'allowance' } } }),
    ]);
    anonymous.list_releases_for_owner.mockResolvedValue([
      makeRelease({ status: { Released: null }, release_block_index: [12n], transfer_id: `0x${'aa'.repeat(32)}` }),
      makeRelease({ status: { TransferUnknown: null }, transfer_id: `0x${'bb'.repeat(32)}` }),
    ]);
    render();
    await settle(24);
    const history = host.querySelector<HTMLElement>('[aria-label="Bridge history"]')!;
    expect(anonymous.list_deposits_for_owner).toHaveBeenCalledWith({ owner: TEST_OWNER, start_after: [], limit: 20 });
    expect(anonymous.list_releases_for_owner).toHaveBeenCalledWith({ owner: TEST_OWNER, start_after_sequence: [], limit: 20 });
    expect(history.textContent).toContain('Delivered (finalized)');
    expect(history.textContent).toContain('Held');
    expect(history.textContent).toContain('Escrow failed (no funds moved)');
    expect(history.textContent).toContain('Released (ledger block 12)');
    expect(history.textContent).toContain('Confirming');
  });

  it('gives every wallet type the full history without any signed or caller-bound call (Oisy included)', async () => {
    mocks.principal.set(TEST_OWNER);
    anonymous.list_deposits_for_owner.mockResolvedValue([makeDeposit({ id: 5n, status: { Delivered: null } })]);
    render();
    await settle(24);
    expect(host.textContent).toContain('#5');
    expect(host.textContent).not.toContain('only deposits started on this device');
    expect(mocks.createSigners).not.toHaveBeenCalled();
    expect(signers.bridge.list_my_deposits).not.toHaveBeenCalled();
  });

  it('shows held claims for the connected Base address, deduplicated with the owner listing', async () => {
    mocks.principal.set(TEST_OWNER);
    const provider = createFakeProvider();
    Object.defineProperty(window, 'ethereum', { configurable: true, value: provider });
    const owned = makeRelease({ transfer_id: `0x${'aa'.repeat(32)}`, status: { Released: null }, release_block_index: [12n], created_at_ns: 10n });
    anonymous.list_releases_for_owner.mockResolvedValue([owned]);
    anonymous.list_releases_for_base_sender.mockResolvedValue([
      owned,
      makeRelease({ transfer_id: `0x${'bb'.repeat(32)}`, status: { Reminting: { deposit_id: 4n } }, created_at_ns: 20n, destination: [] }),
    ]);
    render();
    await waitForWalletDiscovery();
    await settle(24);
    expect(anonymous.list_releases_for_base_sender).toHaveBeenCalledWith({ base_sender: ACCOUNT, start_after_sequence: [], limit: 20 });
    const history = host.querySelector<HTMLElement>('[aria-label="Bridge history"]')!;
    expect(history.textContent).toContain('Re-minting to your Base address');
    expect(history.textContent).toContain('Released (ledger block 12)');
    expect(history.querySelectorAll('li')).toHaveLength(2);
  });

  it('does not list by Base sender when no Base wallet is connected', async () => {
    mocks.principal.set(TEST_OWNER);
    render();
    await waitForWalletDiscovery();
    expect(anonymous.list_releases_for_base_sender).not.toHaveBeenCalled();
  });

  it('keeps the owner history when the Base sender listing fails, and says so', async () => {
    mocks.principal.set(TEST_OWNER);
    Object.defineProperty(window, 'ethereum', { configurable: true, value: createFakeProvider() });
    anonymous.list_releases_for_owner.mockResolvedValue([makeRelease({ transfer_id: `0x${'aa'.repeat(32)}`, status: { Released: null } })]);
    anonymous.list_releases_for_base_sender.mockRejectedValue(new Error('offline'));
    render();
    await waitForWalletDiscovery();
    await settle(24);
    expect(host.textContent).toContain('could not be listed right now');
    expect(host.textContent).toContain('Released');
  });

  it('asks for a sign-in instead of loading history when no ICP wallet is connected', async () => {
    render();
    await settle(24);
    expect(host.textContent).toContain('Connect your ICP wallet to see your bridge history');
    expect(anonymous.list_deposits_for_owner).not.toHaveBeenCalled();
  });
});

describe('ICP to Base through the page', () => {
  beforeEach(() => {
    mocks.flags.enabled = true;
    mocks.principal.set(TEST_OWNER);
  });

  async function reviewTransfer(amount = '25') {
    render();
    await settle();
    type('#icp-amount', amount);
    type('#icp-recipient', '0x1111111111111111111111111111111111111111');
    click(/^Review transfer$/);
    await settle();
  }

  it('shows every fee, the delivery time and that it does not borrow before anything is signed', async () => {
    await reviewTransfer();
    const text = host.textContent ?? '';
    expect(text).toContain('Pulled from your icUSD account');
    expect(text).toContain('25 icUSD');
    expect(text).toContain('icUSD ledger fee, charged on top');
    expect(text).toContain('0.001 icUSD');
    expect(text).toContain('Approval fee');
    expect(text).toContain('Bridge deposit fee');
    expect(text).toContain('0.0001 icUSD');
    expect(text).toContain('24.9999 icUSD');
    expect(text).toContain('about 20 minutes after the deposit is escrowed');
    expect(text).toContain('does not borrow');
    expect(text).toContain('approve exactly 25.001 icUSD');
    expect(signers.ledger.icrc2_approve).not.toHaveBeenCalled();
    expect(signers.bridge.bridge_to_base).not.toHaveBeenCalled();
  });

  it('issues the approval inside the click itself, then the bridge call, and tracks the deposit', async () => {
    await reviewTransfer();
    const confirm = button(/Approve and bridge/)!;
    confirm.click();
    // No await between the click and this assertion: the first signer call happened synchronously.
    expect(signers.ledger.icrc2_approve).toHaveBeenCalledTimes(1);
    expect(signers.bridge.bridge_to_base).not.toHaveBeenCalled();
    const stored = JSON.parse(localStorage.getItem(`rumi:base-bridge:v1:${TEST_OWNER.toText()}`)!);
    expect(stored.intents[0]).toMatchObject({ kind: 'icp_to_base', status: 'prepared', amountE8s: '2500000000' });
    flushSync();
    await settle();
    expect(signers.bridge.bridge_to_base).toHaveBeenCalledTimes(1);
    expect(host.textContent).toContain('Escrowed');
    expect(host.textContent).toContain('DEPOSIT #1');
    const after = JSON.parse(localStorage.getItem(`rumi:base-bridge:v1:${TEST_OWNER.toText()}`)!);
    expect(after.intents[0]).toMatchObject({ status: 'submitted', depositId: '1' });
  });

  it('skips the approval when the allowance already covers the transfer', async () => {
    mocks.buildPrepareDeps.mockImplementation(() => ({
      fetchConfig: async () => ({
        depositFeeE8s: 10_000n,
        bridgeLedgerFeeE8s: null,
        limits: { minDepositE8s: 100_000_000n, maxDepositE8s: 50_000_000_000n, depositsPaused: false, halted: null },
      }),
      fetchLedgerFee: async () => 100_000n,
      readAllowance: async () => ({ allowance: 2_500_100_000n, expiresAtNs: null }),
      readBalance: async () => 10_000_000_000n,
      now: () => Date.now(),
    }));
    await reviewTransfer();
    expect(host.textContent).not.toContain('Approval fee');
    click(/^Bridge to Base$/);
    expect(signers.ledger.icrc2_approve).not.toHaveBeenCalled();
    expect(signers.bridge.bridge_to_base).toHaveBeenCalledTimes(1);
  });

  it('does not sign anything when the approval is declined', async () => {
    signers.ledger.icrc2_approve.mockRejectedValue(new Error('User rejected the request'));
    await reviewTransfer();
    click(/Approve and bridge/);
    await settle();
    expect(signers.bridge.bridge_to_base).not.toHaveBeenCalled();
    expect(host.textContent).toContain('The approval was declined in your wallet');
    expect(localStorage.getItem(`rumi:base-bridge:v1:${TEST_OWNER.toText()}`)).not.toContain('"clientRequestId"');
  });

  it('asks for a new confirmation when the bridge reports a different fee', async () => {
    signers.bridge.bridge_to_base.mockResolvedValue({ Err: { FeeChanged: { current_fee_e8s: 20_000n } } });
    await reviewTransfer();
    click(/Approve and bridge/);
    await settle();
    expect(host.textContent).toContain('The bridge deposit fee changed to 0.0002 icUSD');
    expect(host.textContent).toContain('No icUSD moved');
    expect(button(/Review new fee/)).toBeTruthy();
    expect(button(/Approve and bridge/)).toBeUndefined();
  });

  it('after a lost reply, finds the deposit with get_deposit_by_request and tracks it without any retry', async () => {
    const landedFor = (requestId: Uint8Array) => makeDeposit({ id: 12n, status: { Escrowed: null }, client_request_id: requestId });
    signers.bridge.bridge_to_base.mockRejectedValue(new Error("Cannot read properties of undefined (reading '_arr')"));
    // The page generates a random request id; the bridge answers for whichever id it is asked about.
    anonymous.get_deposit_by_request.mockImplementation(async (_owner: unknown, requestId: Uint8Array) => [landedFor(requestId)]);
    anonymous.get_deposit.mockResolvedValue([landedFor(new Uint8Array(16))]);
    await reviewTransfer();
    click(/Approve and bridge/);
    await settle(24);
    expect(signers.bridge.bridge_to_base).toHaveBeenCalledTimes(1);
    expect(anonymous.get_deposit_by_request).toHaveBeenCalledTimes(1);
    expect(signers.bridge.list_my_deposits).not.toHaveBeenCalled();
    expect(host.textContent).toContain('DEPOSIT #12');
    expect(host.textContent).not.toContain('not confirmed yet');
  });

  it('after a lost reply, offers a retry that reuses the same request id', async () => {
    signers.bridge.bridge_to_base.mockRejectedValueOnce(new Error('network error'));
    await reviewTransfer();
    click(/Approve and bridge/);
    await settle();
    expect(host.textContent).toContain('The result of this transfer is not confirmed yet');
    expect(signers.bridge.bridge_to_base).toHaveBeenCalledTimes(1);
    click(/Retry with the same request/);
    await settle();
    expect(signers.bridge.bridge_to_base).toHaveBeenCalledTimes(2);
    const ids = signers.bridge.bridge_to_base.mock.calls.map((call) => Array.from(call[0].client_request_id as Uint8Array).join(','));
    expect(new Set(ids).size).toBe(1);
    expect(host.textContent).toContain('DEPOSIT #1');
  });

  it('refuses to review an invalid amount or recipient without calling anything', async () => {
    render();
    await settle();
    type('#icp-amount', '1.123456789');
    type('#icp-recipient', '0x1111111111111111111111111111111111111111');
    click(/^Review transfer$/);
    await settle();
    expect(host.textContent).toContain('at most 8 decimal places');
    type('#icp-amount', '5');
    type('#icp-recipient', '0xnotanaddress');
    click(/^Review transfer$/);
    await settle();
    expect(host.textContent).toContain('0x followed by 40 hexadecimal characters');
    expect(mocks.buildPrepareDeps).not.toHaveBeenCalled();
  });

  it('defaults the recipient to the connected Base account', async () => {
    const provider = createFakeProvider();
    Object.defineProperty(window, 'ethereum', { configurable: true, value: provider });
    render();
    await waitForWalletDiscovery();
    expect(host.querySelector<HTMLInputElement>('#icp-recipient')!.value).toBe(ACCOUNT);
  });

  it('offers a refund to the owner of a held deposit and signs inside the click', async () => {
    const held = makeDeposit({ status: { MintHeld: { reason: 'supply cap reached' } }, client_request_id: new Uint8Array(16).fill(0xcd) });
    signers.bridge.bridge_to_base.mockResolvedValue({ Ok: held });
    anonymous.get_deposit.mockResolvedValue([held]);
    signers.bridge.request_deposit_refund.mockResolvedValue({ Ok: makeDeposit({ status: { Refunding: null } }) });
    await reviewTransfer();
    click(/Approve and bridge/);
    await settle(24);
    expect(host.textContent).toContain('supply cap reached');
    const refund = button(/Request refund/);
    expect(refund, 'refund button for the owner of a held deposit').toBeTruthy();
    refund!.click();
    expect(signers.bridge.request_deposit_refund).toHaveBeenCalledWith(1n); // first signer call, no await before it
    await settle(24);
    expect(host.textContent).toContain('Refund requested');
  });

  it('shows no refund button to someone who does not own the held deposit', async () => {
    const stranger = Principal.fromText('aaaaa-aa');
    const held = makeDeposit({ status: { MintHeld: { reason: 'supply cap reached' } }, owner: stranger, client_request_id: new Uint8Array(16).fill(0xcd) });
    signers.bridge.bridge_to_base.mockResolvedValue({ Ok: held });
    anonymous.get_deposit.mockResolvedValue([held]);
    await reviewTransfer();
    click(/Approve and bridge/);
    await settle(24);
    expect(host.textContent).toContain('supply cap reached');
    expect(button(/Request refund/)).toBeUndefined();
  });

  it('never shows a refund button for any other deposit status', async () => {
    const done = makeDeposit({ status: { Delivered: null }, client_request_id: new Uint8Array(16).fill(0xcd) });
    signers.bridge.bridge_to_base.mockResolvedValue({ Ok: done });
    anonymous.get_deposit.mockResolvedValue([done]);
    await reviewTransfer();
    click(/Approve and bridge/);
    await settle(24);
    expect(host.textContent).toContain('Delivered (finalized)');
    expect(button(/Request refund/)).toBeUndefined();
  });

  it('resolves an earlier request after a reload by its saved request id, without sending anything', async () => {
    const store = new IntentStore(localStorage, TEST_OWNER.toText());
    store.put({
      v: 1,
      kind: 'icp_to_base',
      clientRequestId: `0x${'cd'.repeat(16)}`,
      amountE8s: 2_500_000_000n,
      recipient: '0x1111111111111111111111111111111111111111',
      expectedDepositFee: 10_000n,
      createdAt: 1,
      status: 'prepared',
    });
    const landed = makeDeposit({ id: 7n, status: { Escrowed: null }, client_request_id: new Uint8Array(16).fill(0xcd) });
    anonymous.get_deposit_by_request.mockResolvedValue([landed]);
    anonymous.get_deposit.mockResolvedValue([landed]);
    render();
    await settle(24);
    expect(host.textContent).toContain('Earlier request without a recorded outcome');
    // A new transfer is blocked until this one is resolved.
    click(/Check status/);
    await settle(24);
    const [owner, requestId] = anonymous.get_deposit_by_request.mock.calls[0];
    expect(owner.toText()).toBe(TEST_OWNER.toText());
    expect(Array.from(requestId as Uint8Array)).toEqual(Array.from({ length: 16 }, () => 0xcd));
    expect(signers.bridge.list_my_deposits).not.toHaveBeenCalled();
    expect(signers.bridge.bridge_to_base).not.toHaveBeenCalled();
    expect(signers.ledger.icrc2_approve).not.toHaveBeenCalled();
    expect(host.textContent).toContain('DEPOSIT #7');
    expect(host.textContent).not.toContain('Earlier request without a recorded outcome');
  });

  it('abandons an in-flight transfer when the ICP wallet session changes', async () => {
    let finishApproval!: (value: unknown) => void;
    signers.ledger.icrc2_approve.mockImplementation(() => new Promise((resolve) => (finishApproval = resolve)));
    await reviewTransfer();
    click(/Approve and bridge/);
    await settle();
    mocks.generation.set(1);
    await settle();
    expect(host.textContent).toContain('wallet session changed');
    finishApproval({ Ok: 3n });
    await settle();
    expect(signers.bridge.bridge_to_base).not.toHaveBeenCalled();
  });
});

describe('Base to ICP through the page', () => {
  const NONCE_TRANSFER = `0x${'ee'.repeat(32)}`;
  let used: boolean;
  let sent: Array<{ from: string; to: string; data: string }>;

  function wallet(chainId = 8453) {
    used = false;
    sent = [];
    const provider = createFakeProvider({
      chainId,
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
          sent.push((params as [(typeof sent)[number]])[0]);
          used = true;
          return `0x${'12'.repeat(32)}`;
        },
        eth_getTransactionReceipt: () => ({ transactionHash: `0x${'12'.repeat(32)}`, blockNumber: '0x64', status: '0x1' }),
      },
    });
    Object.defineProperty(window, 'ethereum', { configurable: true, value: provider });
    return provider;
  }

  beforeEach(() => {
    mocks.flags.enabled = true;
    mocks.principal.set(TEST_OWNER);
  });

  async function openBaseTab() {
    render();
    await waitForWalletDiscovery();
    host.querySelector<HTMLButtonElement>('#tab-base-to-icp')!.click();
    flushSync();
    await settle(24);
  }

  it('blocks sending on the wrong network and offers to switch', async () => {
    const provider = wallet(1);
    await openBaseTab();
    const panel = host.querySelector<HTMLElement>('#panel-base-to-icp')!;
    expect(panel.textContent).toContain('Switch to Base');
    expect(Array.from(panel.querySelectorAll('button')).some((b) => /^Review transfer$/.test(b.textContent ?? ''))).toBe(false);
    const switchButton = Array.from(panel.querySelectorAll('button')).find((b) => /Switch to Base/.test(b.textContent ?? ''))!;
    switchButton.click();
    await settle();
    expect(provider.callsTo('wallet_switchEthereumChain')[0].params).toEqual([{ chainId: '0x2105' }]);
    expect(provider.callsTo('eth_sendTransaction')).toHaveLength(0);
  });

  it('shows the ICP destination explicitly, sends the exact calldata and tracks through the canister', async () => {
    wallet();
    await openBaseTab();
    type('#base-amount', '2.5');
    await new Promise((resolve) => setTimeout(resolve, 320));
    await settle(24);
    const panel = host.querySelector<HTMLElement>('#panel-base-to-icp')!;
    expect(panel.textContent).toContain('Balance on Base: 10 icUSD');
    const review = Array.from(panel.querySelectorAll('button')).find((b) => /^Review transfer$/.test(b.textContent ?? ''))!;
    expect(review.disabled).toBe(false);
    review.click();
    await settle();
    expect(panel.textContent).toContain(`icUSD will be released to`);
    expect(panel.textContent).toContain(TEST_OWNER.toText());
    expect(panel.textContent).toContain('about 20 minutes');

    const sendButton = Array.from(panel.querySelectorAll('button')).find((b) => /Burn on Base and bridge/.test(b.textContent ?? ''))!;
    sendButton.click();
    await settle(30);
    expect(sent).toHaveLength(1);
    expect(sent[0]).toMatchObject({ from: ACCOUNT, to: '0x2222222222222222222222222222222222222222' });
    expect(sent[0].data.startsWith(SELECTORS.bridgeOut)).toBe(true);
    // Wallet receipt alone: awaiting finality, never released.
    const statusLabel = () => panel.querySelector('[aria-label="Transfer status"] .label')?.textContent;
    expect(statusLabel()).toBe('Awaiting finality');
    expect(panel.textContent).not.toMatch(/Released \(ledger block/);

    anonymous.get_release.mockResolvedValue([makeRelease({ status: { Pending: null } })]);
    await new Promise((resolve) => setTimeout(resolve, 2100));
    await settle(24);
    expect(statusLabel()).toBe('Release pending');
  });

  it('returns to the form without a trace when the user declines in the wallet', async () => {
    const provider = wallet();
    provider.handlers.eth_sendTransaction = () => {
      throw walletError(4001, 'User rejected');
    };
    await openBaseTab();
    type('#base-amount', '2.5');
    await new Promise((resolve) => setTimeout(resolve, 320));
    await settle(24);
    const panel = host.querySelector<HTMLElement>('#panel-base-to-icp')!;
    Array.from(panel.querySelectorAll('button')).find((b) => /^Review transfer$/.test(b.textContent ?? ''))!.click();
    await settle();
    Array.from(panel.querySelectorAll('button')).find((b) => /Burn on Base and bridge/.test(b.textContent ?? ''))!.click();
    await settle(30);
    expect(panel.textContent).toContain('You declined the request in your wallet');
    const record = JSON.parse(localStorage.getItem(`rumi:base-bridge:v1:${TEST_OWNER.toText()}`) ?? '{"intents":[]}');
    expect(record.intents).toEqual([]);
  });

  function saveUnsentTransfer(from = ACCOUNT) {
    new IntentStore(localStorage, TEST_OWNER.toText()).put({
      v: 1,
      kind: 'base_to_icp',
      clientNonce: `0x${'7a'.repeat(32)}`,
      transferId: NONCE_TRANSFER,
      from,
      amountE8s: 250_000_000n,
      destinationPrincipal: TEST_OWNER.toText(),
      chainId: 8453,
      contract: '0x2222222222222222222222222222222222222222',
      createdAt: 1,
      status: 'prepared',
    });
  }

  it('after a reload, resumes an unsent transfer with the SAME nonce', async () => {
    wallet();
    saveUnsentTransfer();
    const provider = (window as unknown as { ethereum: ReturnType<typeof createFakeProvider> }).ethereum;
    await openBaseTab();
    const panel = host.querySelector<HTMLElement>('#panel-base-to-icp')!;
    expect(panel.textContent).toContain('Saved transfers on this device');
    expect(panel.textContent).toContain('never sent');
    Array.from(panel.querySelectorAll('button')).find((b) => /Resume \(same request\)/.test(b.textContent ?? ''))!.click();
    await settle(30);
    expect(sent).toHaveLength(1);
    expect(sent[0].data).toContain('7a'.repeat(32));
    const idRequests = provider
      .callsTo('eth_call')
      .filter((call) => (call.params as [{ data: string }])[0].data.startsWith(SELECTORS.computeTransferId));
    expect(idRequests).toHaveLength(0);
    const record = JSON.parse(localStorage.getItem(`rumi:base-bridge:v1:${TEST_OWNER.toText()}`)!);
    expect(record.intents).toHaveLength(1);
    expect(record.intents[0]).toMatchObject({ status: 'sent', clientNonce: `0x${'7a'.repeat(32)}` });
  });

  it('does not offer to send an unsent transfer from a different Base account, but allows discarding it', async () => {
    wallet();
    saveUnsentTransfer('0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb');
    await openBaseTab();
    const panel = host.querySelector<HTMLElement>('#panel-base-to-icp')!;
    expect(panel.textContent).toContain('cannot be sent from the connected account');
    expect(Array.from(panel.querySelectorAll('button')).some((b) => /Resume/.test(b.textContent ?? ''))).toBe(false);
    Array.from(panel.querySelectorAll('button')).find((b) => /Discard unsent transfer/.test(b.textContent ?? ''))!.click();
    await settle(30);
    expect(panel.textContent).toContain('removed from this device');
    const record = JSON.parse(localStorage.getItem(`rumi:base-bridge:v1:${TEST_OWNER.toText()}`)!);
    expect(record.intents).toEqual([]);
    expect(sent).toHaveLength(0);
  });

  it('keeps a transfer that already exists on Base when asked to discard it', async () => {
    wallet();
    saveUnsentTransfer('0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb');
    await openBaseTab();
    used = true;
    const panel = host.querySelector<HTMLElement>('#panel-base-to-icp')!;
    Array.from(panel.querySelectorAll('button')).find((b) => /Discard unsent transfer/.test(b.textContent ?? ''))!.click();
    await settle(30);
    expect(panel.textContent).toContain('exists on Base, so it was kept');
    const record = JSON.parse(localStorage.getItem(`rumi:base-bridge:v1:${TEST_OWNER.toText()}`)!);
    expect(record.intents).toHaveLength(1);
  });

  it('requires an ICP sign-in before anything can be sent', async () => {
    mocks.principal.set(null);
    wallet();
    await openBaseTab();
    type('#base-amount', '2.5');
    await settle(12);
    const panel = host.querySelector<HTMLElement>('#panel-base-to-icp')!;
    expect(panel.textContent).toContain('Sign in with your ICP wallet');
    expect(Array.from(panel.querySelectorAll('button')).some((b) => /Burn on Base/.test(b.textContent ?? ''))).toBe(false);
  });
});
