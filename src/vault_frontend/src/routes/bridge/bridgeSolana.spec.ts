/**
 * The Solana branch of the /bridge page, through the real page and the real forms.
 *
 * Everything outside the page is injected: the destination read, the ICP signers and the anonymous reads, the Solana node
 * adapter (a fake chain), and the Wallet Standard registry (fake wallets). No network, no wallet and no key is ever touched.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
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
    principal: writable<any>(null),
    generation: writable(0),
    loadBridgeConfig: vi.fn(),
    getAnonymousBridge: vi.fn(),
    createSigners: vi.fn(),
    buildPrepareDeps: vi.fn(),
    loadSolanaDestination: vi.fn(),
    getAnonymousSolana: vi.fn(),
    createSolanaSigners: vi.fn(),
    buildSolanaPrepareDeps: vi.fn(),
    getSolanaRpc: vi.fn(),
    registry: { get: vi.fn(), on: vi.fn() },
    endpoints: [] as Array<{ networkId: string; genesisHash: string; url: string }>,
    realGetSolanaRpc: null as unknown as (network: unknown) => unknown,
  };
});

vi.mock('$lib/config', () => ({
  get BRIDGE_ENABLED() {
    return mocks.flags.enabled;
  },
  CANISTER_IDS: { ICUSD_BRIDGE: 'tfesu-vyaaa-aaaap-qrd7a-cai', ICUSD_SOL_BRIDGE: '' },
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
  getBridgeCanisterId: () => 'tfesu-vyaaa-aaaap-qrd7a-cai',
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
vi.mock('$lib/services/solanaBridge/solanaActor', async (importOriginal) => ({
  ...(await importOriginal<typeof import('$lib/services/solanaBridge/solanaActor')>()),
  loadSolanaDestination: mocks.loadSolanaDestination,
  getAnonymousSolanaBridge: mocks.getAnonymousSolana,
}));
vi.mock('$lib/services/solanaBridge/endpoints', () => ({
  get SOLANA_RPC_ENDPOINTS() {
    return mocks.endpoints;
  },
}));
vi.mock('$lib/services/solanaBridge/solanaDeps', async (importOriginal) => {
  const original = await importOriginal<typeof import('$lib/services/solanaBridge/solanaDeps')>();
  mocks.realGetSolanaRpc = original.getSolanaRpc as never;
  return {
    ...original,
    createSolanaSigners: mocks.createSolanaSigners,
    buildSolanaPrepareDeps: mocks.buildSolanaPrepareDeps,
    getSolanaRpc: mocks.getSolanaRpc,
  };
});

import Page from './+page.svelte';
import { makeBridgeInfo, TEST_OWNER } from '$lib/services/baseBridge/testSupport';
import { resolveBridgeConfig } from '$lib/services/baseBridge/chains';
import { SolanaIntentStore, type SolReturnIntent } from '$lib/services/solanaBridge/intents';
import { base58Encode, toHex } from '$lib/services/solanaBridge/bytes';
import { buildReturnMessage } from '$lib/services/solanaBridge/transaction';
import { SOLANA_SIGN_AND_SEND_TRANSACTION, SOLANA_SIGN_TRANSACTION } from '$lib/services/solanaBridge/walletStandard';
import { browserLocalStorage } from '$lib/services/baseBridge/intents';
import { KEYS, createFakeChain, landOn, makeAccount, makeRegistry, makeSolDeposit, makeSolRelease, makeSolReturn, makeSolanaConfig, makeSolanaDestination, makeStandardWallet, signWith, TEST_OWNER_TEXT, type FakeChain } from '$lib/services/solanaBridge/testSupport';

async function settle(rounds = 14) {
  for (let i = 0; i < rounds; i++) {
    await Promise.resolve();
    await tick();
    flushSync();
  }
}

let host: HTMLDivElement;
let instance: unknown;
let chain: FakeChain;

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
  get_return: vi.fn(),
  get_release: vi.fn(),
  list_deposits_for_owner: vi.fn(),
  list_releases_for_owner: vi.fn(),
  list_releases_for_solana_sender: vi.fn(),
};
const signers = {
  bridge: { bridge_to_solana: vi.fn(), request_deposit_refund: vi.fn(), notify_solana_return: vi.fn() },
  ledger: { icrc2_approve: vi.fn() },
};

const fill = (byte: number, n = 32) => new Uint8Array(n).fill(byte);
const hexOf = (bytes: ArrayLike<number>) => Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
let standard: ReturnType<typeof makeStandardWallet>;

function enableSolana() {
  mocks.loadSolanaDestination.mockResolvedValue(makeSolanaDestination());
}

beforeEach(() => {
  localStorage.clear();
  sessionStorage.clear();
  host = document.createElement('div');
  document.body.appendChild(host);
  chain = createFakeChain();
  standard = makeStandardWallet('Fake Wallet', [makeAccount(KEYS.holder)]);
  standard.signer = signWith(fill(0xe5, 64)) as never;
  mocks.flags.enabled = true;
  mocks.endpoints.length = 0;
  mocks.principal.set(TEST_OWNER);
  mocks.generation.set(0);
  mocks.loadBridgeConfig.mockReset().mockImplementation(async () => resolveBridgeConfig(makeBridgeInfo()));
  mocks.getAnonymousBridge.mockReset().mockResolvedValue({ get_deposit: vi.fn().mockResolvedValue([]), list_deposits_for_owner: vi.fn().mockResolvedValue([]), list_releases_for_owner: vi.fn().mockResolvedValue([]), list_releases_for_base_sender: vi.fn().mockResolvedValue([]), get_release: vi.fn().mockResolvedValue([]) });
  mocks.createSigners.mockReset().mockResolvedValue({ bridge: {}, ledger: {} });
  mocks.buildPrepareDeps.mockReset();
  mocks.loadSolanaDestination.mockReset().mockResolvedValue(null);
  mocks.getAnonymousSolana.mockReset().mockResolvedValue(anonymous);
  mocks.createSolanaSigners.mockReset().mockResolvedValue(signers);
  mocks.getSolanaRpc.mockReset().mockImplementation(() => chain);
  mocks.buildSolanaPrepareDeps.mockReset().mockImplementation(() => ({
    fetchConfig: async () => ({ depositFeeE8s: 10_000_000n, bridgeLedgerFeeE8s: null, limits: { minDepositE8s: 100_000_000n, maxDepositE8s: 1_000_000_000n, depositsPaused: false, halted: null } }),
    fetchLedgerFee: async () => 10_000n,
    readAllowance: async () => ({ allowance: 0n, expiresAtNs: null }),
    readBalance: async () => 5_000_000_000n,
    now: () => Date.now(),
  }));
  mocks.registry.get.mockReset().mockImplementation(() => [standard]);
  mocks.registry.on.mockReset().mockImplementation(() => () => {});
  anonymous.get_deposit.mockReset().mockResolvedValue([makeSolDeposit({ status: { Escrowed: null } })]);
  anonymous.get_deposit_by_request.mockReset().mockResolvedValue([]);
  anonymous.get_return.mockReset().mockResolvedValue([]);
  anonymous.get_release.mockReset().mockResolvedValue([]);
  anonymous.list_deposits_for_owner.mockReset().mockResolvedValue([]);
  anonymous.list_releases_for_owner.mockReset().mockResolvedValue([]);
  anonymous.list_releases_for_solana_sender.mockReset().mockResolvedValue([]);
  signers.bridge.bridge_to_solana.mockReset().mockResolvedValue({ Ok: makeSolDeposit({ status: { Escrowed: null } }) });
  signers.bridge.request_deposit_refund.mockReset();
  signers.bridge.notify_solana_return.mockReset().mockResolvedValue({ Ok: null });
  signers.ledger.icrc2_approve.mockReset().mockResolvedValue({ Ok: 5n });
});

afterEach(() => {
  vi.unstubAllGlobals();
  if (instance) unmount(instance as any);
  instance = undefined;
  host.remove();
  vi.restoreAllMocks();
});

/** Not one Solana wallet call of any kind: nothing discovered, connected, signed or broadcast. */
function expectNoSolanaWalletActivity() {
  expect(mocks.registry.get).not.toHaveBeenCalled();
  expect(mocks.registry.on).not.toHaveBeenCalled();
  expect(standard.connects).toBe(0);
  expect(standard.signs).toHaveLength(0);
  expect(chain.sent).toHaveLength(0);
}

async function selectSolana() {
  const entry = option('solana-mainnet')!;
  expect(entry.tagName).toBe('BUTTON');
  entry.click();
  flushSync();
  await settle();
}

async function connectWallet() {
  const before = standard.connects;
  click(/^Connect Solana wallet$/);
  await settle();
  expect(standard.connects).toBe(before + 1);
  expect(host.textContent).toContain(standard.name);
}

describe('Solana while it is disabled (every build today)', () => {
  it('shows a disabled row, mounts nothing and asks no wallet, whatever the user does', async () => {
    render();
    await settle();
    const entry = option('solana-mainnet')!;
    expect(entry.getAttribute('aria-disabled')).toBe('true');
    expect(entry.tagName).toBe('DIV');
    expect(entry.textContent).toContain('Unavailable');
    expect(entry.textContent).toContain('Not available yet: there is no icUSD bridge to Solana at the moment.');
    entry.click();
    entry.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true }));
    flushSync();
    await settle();
    // The page stays on Base; no Solana panel, tab, wallet bar or form exists.
    expect(option('base-mainnet')!.getAttribute('aria-checked')).toBe('true');
    expect(host.querySelector('#tab-icp-to-solana')).toBeNull();
    expect(host.querySelector('[aria-label="Solana wallet"]')).toBeNull();
    expect(host.querySelector('#sol-icp-amount')).toBeNull();
    expect(host.textContent).toContain('INTERNET COMPUTER ↔ BASE');
    expectNoSolanaWalletActivity();
    expect(mocks.createSolanaSigners).not.toHaveBeenCalled();
    expect(signers.ledger.icrc2_approve).not.toHaveBeenCalled();
    expect(signers.bridge.bridge_to_solana).not.toHaveBeenCalled();
  });

  it('keeps a Solana instance that the canister has not set up disabled, with its reason', async () => {
    const refused = makeSolanaDestination();
    mocks.loadSolanaDestination.mockResolvedValue({ ...refused, status: 'disabled', reason: 'The Solana bridge is not set up yet.', native: null });
    render();
    await settle();
    expect(option('solana-mainnet')!.getAttribute('aria-disabled')).toBe('true');
    expect(option('solana-mainnet')!.textContent).toContain('The Solana bridge is not set up yet.');
    expectNoSolanaWalletActivity();
  });

  it('turns an enabled Solana entry off again at the 60 second refresh, without a wallet call', async () => {
    enableSolana();
    let tick60: (() => Promise<void>) | null = null;
    const real = globalThis.setInterval.bind(globalThis);
    vi.spyOn(globalThis, 'setInterval').mockImplementation(((handler: TimerHandler, delay?: number, ...rest: unknown[]) => {
      if (delay === 60_000) {
        tick60 = handler as () => Promise<void>;
        return 0 as never;
      }
      return real(handler, delay, ...rest);
    }) as never);
    render();
    await settle();
    await selectSolana();
    expect(host.querySelector('#tab-icp-to-solana')).toBeTruthy();
    mocks.loadSolanaDestination.mockResolvedValue({ ...makeSolanaDestination(), status: 'disabled', reason: 'The Solana bridge is not set up yet.', native: null });
    await tick60!();
    await settle();
    expect(host.querySelector('#tab-icp-to-solana')).toBeNull();
    expect(host.textContent).toContain('The Solana bridge is not set up yet.');
    expect(standard.signs).toHaveLength(0);
  });
});

describe('an enabled Solana instance (injected fixture)', () => {
  it('selects Solana, shows the Solana panel and discovers wallets without connecting any', async () => {
    enableSolana();
    render();
    await settle();
    expect(option('base-mainnet')!.getAttribute('aria-checked')).toBe('true');
    expect(option('solana-mainnet')!.hasAttribute('aria-disabled')).toBe(false);
    await selectSolana();
    expect(host.textContent).toContain('INTERNET COMPUTER ↔ SOLANA');
    // The subtitle follows the selected destination.
    expect(host.querySelector('.subtitle')!.textContent).toContain('between the Internet Computer and Solana.');
    expect(host.querySelector('.subtitle')!.textContent).toContain('Every icUSD on Solana is backed one to one');
    expect(host.querySelector('.subtitle')!.textContent).not.toContain('Base');
    expect(host.querySelector('#tab-icp-to-solana')!.getAttribute('aria-selected')).toBe('true');
    expect(host.querySelector('#tab-solana-to-icp')).toBeTruthy();
    expect(host.textContent).toContain('ICP to Solana');
    // Wallets were looked up; none was asked to connect, and the Base form is gone.
    expect(mocks.registry.get).toHaveBeenCalled();
    expect(standard.connects).toBe(0);
    expect(host.querySelector('#icp-amount')).toBeNull();
    expect(host.textContent).toContain('Token-2022');
    expect(host.textContent).toContain(KEYS.mint);
  });

  it('connects through the Wallet Standard only when the user clicks Connect', async () => {
    enableSolana();
    render();
    await settle();
    await selectSolana();
    expect(host.textContent).toContain('Solana wallet not connected');
    await connectWallet();
    expect(host.querySelector('[aria-label="Solana wallet"]')!.textContent).toContain(KEYS.holder.slice(0, 6));
    expect(standard.signs).toHaveLength(0);
  });

  it('bridges ICP to Solana through the ICP signer, to the connected wallet, and follows the deposit to delivery', async () => {
    enableSolana();
    render();
    await settle();
    await selectSolana();
    await connectWallet();
    // The recipient defaults to the connected account and the token account is shown before any approval.
    expect(host.querySelector<HTMLInputElement>('#sol-icp-recipient')!.value).toBe(KEYS.holder);
    expect(host.querySelector('[data-testid="sol-token-account"]')!.textContent).toContain('Token-2022');
    type('#sol-icp-amount', '2');
    click(/^Review transfer$/);
    await settle();
    expect(host.textContent).toContain('You receive on Solana');
    expect(host.textContent).toContain('1.9 icUSD');
    expect(signers.ledger.icrc2_approve).not.toHaveBeenCalled();
    click(/^Approve and bridge$/);
    await settle();
    expect(signers.ledger.icrc2_approve).toHaveBeenCalledTimes(1);
    expect(signers.bridge.bridge_to_solana).toHaveBeenCalledTimes(1);
    const args = signers.bridge.bridge_to_solana.mock.calls[0][0];
    expect(args).toMatchObject({ amount_e8s: 200_000_000n, solana_recipient: KEYS.holder, expected_deposit_fee_e8s: 10_000_000n });
    // The Solana wallet signed nothing in this direction.
    expect(standard.signs).toHaveLength(0);
    expect(host.querySelector('[aria-label="Deposit status"]')!.textContent).toContain('In escrow');
    const intents = new SolanaIntentStore(browserLocalStorage(), TEST_OWNER_TEXT);
    expect(intents.list()).toHaveLength(1);
    expect(intents.list()[0]).toMatchObject({ kind: 'icp_to_solana', status: 'submitted', recipient: KEYS.holder, destination: 'solana-mainnet' });
  });

  it('follows an ICP to Solana deposit from escrow to delivered, by the bridge records only', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    try {
      enableSolana();
      render();
      await settle();
      await selectSolana();
      await connectWallet();
      type('#sol-icp-amount', '2');
      click(/^Review transfer$/);
      await settle();
      click(/^Approve and bridge$/);
      await settle();
      const status = () => host.querySelector('[aria-label="Deposit status"]')!.textContent ?? '';
      expect(status()).toContain('In escrow');
      anonymous.get_deposit.mockResolvedValue([makeSolDeposit({ status: { Delivered: null }, delivered: [{ signature: 'DeliveredSig11111111111', slot: 9n }] })]);
      await vi.advanceTimersByTimeAsync(60_000);
      await settle();
      expect(status()).toContain('Delivered');
      expect(standard.signs).toHaveLength(0);
    } finally {
      vi.useRealTimers();
    }
  });

  it('refuses to review a recipient that is a program or one of the bridge keys', async () => {
    enableSolana();
    render();
    await settle();
    await selectSolana();
    type('#sol-icp-recipient', makeSolanaConfig().mint);
    await settle();
    expect(host.textContent).toContain('program or one of the bridge');
    expect((button(/^Review transfer$/) as HTMLButtonElement).disabled).toBe(true);
    expect(signers.ledger.icrc2_approve).not.toHaveBeenCalled();
  });

  async function openReturnTab() {
    enableSolana();
    render();
    await settle();
    await selectSolana();
    await connectWallet();
    click(/^Solana to ICP$/);
    await settle();
  }

  it('returns icUSD from Solana: shows the exact destination before the wallet, then signs ONE transaction and broadcasts exactly those bytes', async () => {
    await openReturnTab();
    expect(host.querySelector<HTMLSelectElement>('#sol-ret-source')!.value).toBe(KEYS.holderSource);
    expect(host.querySelector<HTMLInputElement>('#sol-ret-principal')!.value).toBe(TEST_OWNER_TEXT);
    type('#sol-ret-amount', '3');
    click(/^Review transfer$/);
    await settle();
    const cfg = makeSolanaConfig();
    expect(host.querySelector('[data-testid="return-account"]')!.textContent).toContain(cfg.returnAccount);
    expect(host.querySelector('[data-testid="return-destination"]')!.textContent).toContain(TEST_OWNER_TEXT);
    const memo = host.querySelector('[data-testid="return-memo"]')!.textContent!;
    expect(memo).toMatch(new RegExp(`^rumi1\\|r\\|${TEST_OWNER_TEXT}\\|-\\|\\d+$`));
    expect(host.textContent).toContain('Token-2022');
    // Reviewing asked the wallet for nothing.
    expect(standard.signs).toHaveLength(0);

    click(/^Sign in your Solana wallet$/);
    await settle();
    expect(standard.signs).toHaveLength(1);
    const asked = standard.signs[0] as unknown as { transaction: Uint8Array; chain: string };
    expect(asked.chain).toBe('solana:mainnet');
    const reference = buildReturnMessage({
      tokenProgram: 'token2022',
      mint: fill(0x44),
      returnOwner: fill(0x33),
      roleKeys: [fill(0x11), fill(0x22)],
      holder: fill(0xa0),
      sourceTokenAccount: fill(0xa1),
      amount: 300_000_000n,
      memo,
      recentBlockhash: fill(0x77),
    });
    expect(toHex(asked.transaction.slice(65))).toBe(toHex(reference.message));
    expect(chain.sent).toHaveLength(1);
    expect(host.querySelector('[data-testid="tracking"]')!.textContent).toContain('Not found yet');

    // What was broadcast is the signed wire, and what was saved first is its message and derived signature.
    const wire = chain.sent[0];
    const saved = new SolanaIntentStore(browserLocalStorage(), TEST_OWNER_TEXT).list()[0] as SolReturnIntent;
    expect(saved).toMatchObject({ status: 'sent', signature: base58Encode(wire.slice(1, 65)), memo, messageHex: toHex(reference.message) });
    expect(saved.nonce).toMatch(/^\d+$/);
    expect(button(/^Tell the bridge now$/)).toBeTruthy();
  });

  it('shows a decline in the wallet as a decline: nothing saved, nothing sent', async () => {
    await openReturnTab();
    (standard.features as Record<string, { signTransaction: () => Promise<never> }>)[SOLANA_SIGN_TRANSACTION].signTransaction = async () => {
      throw Object.assign(new Error('User rejected the request.'), { code: 4001 });
    };
    type('#sol-ret-amount', '3');
    click(/^Review transfer$/);
    await settle();
    click(/^Sign in your Solana wallet$/);
    await settle();
    expect(host.textContent).toContain('You declined the transfer in your wallet. Nothing was signed or sent.');
    expect(chain.sent).toHaveLength(0);
    expect(new SolanaIntentStore(browserLocalStorage(), TEST_OWNER_TEXT).list()).toHaveLength(0);
  });

  it('holds an ambiguous sign-and-send attempt on screen and on disk, and a reload shows it without signing anything', async () => {
    standard = makeStandardWallet('Send Only', [makeAccount(KEYS.holder)], {
      [SOLANA_SIGN_TRANSACTION]: undefined,
      [SOLANA_SIGN_AND_SEND_TRANSACTION]: {
        version: '1.0.0',
        supportedTransactionVersions: ['legacy'],
        signAndSendTransaction: async () => {
          throw new Error('RPC timeout while confirming');
        },
      },
    });
    await openReturnTab();
    type('#sol-ret-amount', '3');
    click(/^Review transfer$/);
    await settle();
    click(/^Sign in your Solana wallet$/);
    await settle();
    const held = host.querySelector('[data-testid="held"]')!;
    expect(held.textContent).toContain('This attempt has no recorded outcome. It is held.');
    expect(held.textContent).toContain('Nothing was rebuilt or signed again');
    expect(chain.sent).toHaveLength(0);
    const stored = new SolanaIntentStore(browserLocalStorage(), TEST_OWNER_TEXT).list()[0] as SolReturnIntent;
    expect(stored).toMatchObject({ status: 'held', signMode: 'sign_and_send' });
    expect(chain.callsTo('getLatestBlockhash')).toHaveLength(1);

    // Reload: a fresh page over the same storage shows the held attempt, and nothing is signed or sent.
    unmount(instance as any);
    host.innerHTML = '';
    standard.signs.length = 0;
    const blockhashReads = chain.callsTo('getLatestBlockhash').length;
    render();
    await settle();
    await selectSolana();
    await connectWallet();
    click(/^Solana to ICP$/);
    await settle();
    expect(host.querySelector('[data-testid="held"]')).toBeTruthy();
    expect(standard.signs).toHaveLength(0);
    expect(chain.sent).toHaveLength(0);
    expect(chain.callsTo('getLatestBlockhash')).toHaveLength(blockhashReads);
    // The only way out is evidence: nothing landed and the blockhash can still land, so it cannot be discarded.
    click(/^Check the evidence$/);
    await settle();
    expect(host.querySelector('[data-testid="evidence"]')!.textContent).toContain('Not found yet');
    expect(button(/^Discard this attempt$/)).toBeUndefined();
    chain.height = 1_001n;
    click(/^Check the evidence$/);
    await settle();
    // Absence plus expiry is NOT proof that it never executed: the attempt stays held, with no way to forget it.
    expect(host.querySelector('[data-testid="evidence"]')!.textContent).toContain('Not found, outcome unknown');
    expect(host.querySelector('[data-testid="evidence"]')!.textContent).not.toMatch(/never landed|No icUSD moved/);
    expect(button(/discard/i)).toBeUndefined();
    const kept = new SolanaIntentStore(browserLocalStorage(), TEST_OWNER_TEXT).list();
    expect(kept).toHaveLength(1);
    expect(kept[0]).toMatchObject({ id: stored.id, status: 'held', messageHex: stored.messageHex });
    expect(standard.signs).toHaveLength(0);
    expect(chain.sent).toHaveLength(0);
  });

  /** A wallet that signs and sends, then fails ambiguously, and the transaction the wallet in fact broadcast. */
  async function heldSendOnlyAttempt(): Promise<{ stored: SolReturnIntent; wire: Uint8Array }> {
    standard = makeStandardWallet('Send Only', [makeAccount(KEYS.holder)], {
      [SOLANA_SIGN_TRANSACTION]: undefined,
      [SOLANA_SIGN_AND_SEND_TRANSACTION]: {
        version: '1.0.0',
        supportedTransactionVersions: ['legacy'],
        signAndSendTransaction: async () => {
          throw new Error('RPC timeout while confirming');
        },
      },
    });
    await openReturnTab();
    type('#sol-ret-amount', '3');
    click(/^Review transfer$/);
    await settle();
    click(/^Sign in your Solana wallet$/);
    await settle();
    const stored = new SolanaIntentStore(browserLocalStorage(), TEST_OWNER_TEXT).list()[0] as SolReturnIntent;
    const message = Uint8Array.from(stored.messageHex.match(/../g)!.map((h) => parseInt(h, 16)));
    return { stored, wire: Uint8Array.from([1, ...fill(0xe5, 64), ...message]) };
  }

  it('does not let a held attempt go while Solana\'s failure of it is not final yet', async () => {
    const { stored, wire } = await heldSendOnlyAttempt();
    // The wallet in fact broadcast it, and its execution failed, but only at "confirmed" so far.
    landOn(chain, wire, 'confirmed', true);
    click(/^Check the evidence$/);
    await settle();
    expect(host.querySelector('[data-testid="tracking"]')!.textContent).toContain('Failed, but not final yet');
    expect(button(/discard/i)).toBeUndefined();
    const kept = new SolanaIntentStore(browserLocalStorage(), TEST_OWNER_TEXT).list();
    expect(kept).toHaveLength(1);
    expect(kept[0]).toMatchObject({ id: stored.id, messageHex: stored.messageHex });
    expect(standard.signs).toHaveLength(0);
  });

  it('lets a held attempt go once Solana FINALIZES its failure', async () => {
    const { wire } = await heldSendOnlyAttempt();
    landOn(chain, wire, 'finalized', true);
    click(/^Check the evidence$/);
    await settle();
    expect(host.querySelector('[data-testid="evidence"]')!.textContent).toContain('The transaction failed on Solana');
    click(/^Discard this attempt$/);
    await settle();
    expect(new SolanaIntentStore(browserLocalStorage(), TEST_OWNER_TEXT).list()).toHaveLength(0);
    expect(standard.signs).toHaveLength(0);
  });

  /** A broadcast return that an earlier visit left on disk, and the signed bytes Solana finalized. */
  function seedSentReturn(): { id: string; wire: Uint8Array } {
    const config = makeSolanaConfig();
    const store = new SolanaIntentStore(browserLocalStorage(), TEST_OWNER_TEXT);
    const memo = `rumi1|r|${TEST_OWNER_TEXT}|-|42`;
    const reference = buildReturnMessage({ tokenProgram: 'token2022', mint: fill(0x44), returnOwner: fill(0x33), roleKeys: [fill(0x11), fill(0x22)], holder: fill(0xa0), sourceTokenAccount: fill(0xa1), amount: 300_000_000n, memo, recentBlockhash: fill(0x77) });
    const wire = Uint8Array.from([1, ...fill(0xe5, 64), ...reference.message]);
    const id = '77'.repeat(16);
    store.put({ v: 1, kind: 'solana_to_icp', id, destination: 'solana-mainnet', genesisHash: config.network.genesisHash, instance: config.instanceCanisterId, mint: config.mint, tokenProgram: 'token2022', returnAccount: config.returnAccount, wallet: KEYS.holder, sourceAccount: KEYS.holderSource, icpPrincipal: TEST_OWNER_TEXT, icpSubaccount: null, amountE8s: 300_000_000n, nonce: '42', memo, blockhash: base58Encode(fill(0x77)), lastValidBlockHeight: '1000', messageHex: toHex(reference.message), signMode: 'sign', signedHex: toHex(wire), signature: base58Encode(fill(0xe5, 64)), createdAt: 5, status: 'sent' });
    landOn(chain, wire);
    return { id, wire };
  }

  it('follows a broadcast return after a reload, without a wallet, and shows the burn the bridge reports', async () => {
    seedSentReturn();
    enableSolana();
    anonymous.get_return.mockResolvedValue([makeSolReturn({ status: { Burning: { burn_op_id: 1n } } })]);
    render();
    await settle();
    await selectSolana();
    click(/^Solana to ICP$/);
    await settle();
    const tracking = host.querySelector('[data-testid="tracking"]')!;
    expect(tracking).toBeTruthy();
    expect(tracking.textContent).toContain('Burning your returned icUSD');
    expect(standard.signs).toHaveLength(0);
    expect(chain.sent).toHaveLength(0);
    expect(standard.connects).toBe(0);
  });

  it('walks a return from finalized on Solana through burn and release pending to Released, settled only by the release record', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    try {
      const { id } = seedSentReturn();
      enableSolana();
      render();
      await settle();
      await selectSolana();
      click(/^Solana to ICP$/);
      await settle();
      const label = () => host.querySelector('[data-testid="tracking"]')!.textContent ?? '';
      const step = async () => {
        await vi.advanceTimersByTimeAsync(60_000);
        await settle();
      };
      // 1. The bridge has not seen it; Solana has finalized it. A wait, never a settlement.
      expect(label()).toContain('Finalized on Solana');
      // A fresh store reads what the page wrote to storage (each instance caches its own copy).
      const saved = () => new SolanaIntentStore(browserLocalStorage(), TEST_OWNER_TEXT).get(id);
      expect(saved()).toMatchObject({ status: 'sent' });
      // 2. The bridge credits and burns.
      anonymous.get_return.mockResolvedValue([makeSolReturn({ status: { Burning: { burn_op_id: 1n } } })]);
      await step();
      expect(label()).toContain('Burning your returned icUSD');
      // 3. The burn is final; the release is pending on the ICP ledger.
      anonymous.get_return.mockResolvedValue([makeSolReturn({ status: { Burned: { burn_op_id: 1n } } })]);
      anonymous.get_release.mockResolvedValue([makeSolRelease({ status: { Pending: null } })]);
      await step();
      expect(label()).toContain('Release pending');
      expect(saved()).toMatchObject({ status: 'sent' });
      // 4. Only the release record being Released settles it.
      anonymous.get_release.mockResolvedValue([makeSolRelease({ status: { Released: null }, release_block_index: [77n], amount_to_receive_e8s: [299_980_000n] })]);
      await step();
      expect(label()).toContain('Released (ledger block 77)');
      expect(label()).toContain('2.9998 icUSD');
      expect(saved()).toMatchObject({ status: 'done' });
      expect(standard.signs).toHaveLength(0);
      expect(chain.sent).toHaveLength(0);
    } finally {
      vi.useRealTimers();
    }
  });

  it('blocks a transfer on the wrong Solana network, and when the wallet account changed after the review', async () => {
    standard = makeStandardWallet('Devnet Only', [makeAccount(KEYS.holder, ['solana:devnet'])]);
    enableSolana();
    render();
    await settle();
    await selectSolana();
    await connectWallet();
    expect(host.textContent).toContain('This wallet account is not on Solana.');
    click(/^Solana to ICP$/);
    await settle();
    type('#sol-ret-amount', '3');
    expect((button(/^Review transfer$/) as HTMLButtonElement).disabled).toBe(false);
    click(/^Review transfer$/);
    await settle();
    expect(host.textContent).toContain('Switch your wallet to Solana.');
    expect(standard.signs).toHaveLength(0);
    expect(chain.sent).toHaveLength(0);
  });

  it('never offers to forget a bridge request that may have been submitted: the original is retried under its own id, with no second request', async () => {
    enableSolana();
    render();
    await settle();
    await selectSolana();
    await connectWallet();
    // The bridge call is lost (the ingress is still pending), and the query by request id finds nothing yet.
    signers.bridge.bridge_to_solana.mockRejectedValueOnce(new Error('ingress timed out'));
    type('#sol-icp-amount', '2');
    click(/^Review transfer$/);
    await settle();
    click(/^Approve and bridge$/);
    await settle();
    expect(host.textContent).toContain('not confirmed yet');
    expect(button(/discard/i)).toBeUndefined();
    const journal = () => new SolanaIntentStore(browserLocalStorage(), TEST_OWNER_TEXT).list();
    expect(journal()).toHaveLength(1);
    const originalId = journal()[0].id;
    // The old update executes late. The only way forward is the same request id, which returns the existing deposit.
    signers.bridge.bridge_to_solana.mockImplementationOnce(async (args: { client_request_id: Uint8Array }) => ({ Ok: makeSolDeposit({ client_request_id: args.client_request_id, status: { Escrowed: null } }) }));
    click(/^Retry with the same request$/);
    await settle();
    const ids = signers.bridge.bridge_to_solana.mock.calls.map((call) => hexOf(call[0].client_request_id));
    expect(ids).toEqual([originalId, originalId]);
    expect(signers.ledger.icrc2_approve).toHaveBeenCalledTimes(1);
    expect(journal()).toHaveLength(1);
    expect(journal()[0]).toMatchObject({ id: originalId, status: 'submitted' });
  });

  it('after a reload, an approved request whose bridge call may have started offers only the same-id continuation and blocks a fresh start', async () => {
    enableSolana();
    render();
    await settle();
    await selectSolana();
    await connectWallet();
    signers.bridge.bridge_to_solana.mockRejectedValueOnce(new Error('ingress timed out'));
    type('#sol-icp-amount', '2');
    click(/^Review transfer$/);
    await settle();
    click(/^Approve and bridge$/);
    await settle();
    const journal = () => new SolanaIntentStore(browserLocalStorage(), TEST_OWNER_TEXT).list();
    expect(journal()).toHaveLength(1);
    expect(journal()[0].status).toBe('approved');
    const originalId = journal()[0].id;

    // Reload. The earlier request is shown and the query by request id still finds nothing.
    unmount(instance as any);
    host.innerHTML = '';
    render();
    await settle();
    await selectSolana();
    await connectWallet();
    // A fresh start with a new request id is refused: no second approval, no second bridge request.
    type('#sol-icp-amount', '3');
    click(/^Review transfer$/);
    await settle();
    click(/^Approve and bridge$/);
    await settle();
    expect(signers.ledger.icrc2_approve).toHaveBeenCalledTimes(1);
    expect(signers.bridge.bridge_to_solana).toHaveBeenCalledTimes(1);
    expect(journal()).toHaveLength(1);
    // The earlier request resumes: no discard, only the same-id continuation.
    click(/^Check status$/);
    await settle();
    expect(host.textContent).toContain('Approval confirmed');
    expect(button(/discard/i)).toBeUndefined();
    signers.bridge.bridge_to_solana.mockImplementationOnce(async (args: { client_request_id: Uint8Array }) => ({ Ok: makeSolDeposit({ client_request_id: args.client_request_id, status: { Escrowed: null } }) }));
    click(/^Bridge to Solana$/);
    await settle();
    const ids = signers.bridge.bridge_to_solana.mock.calls.map((call) => hexOf(call[0].client_request_id));
    expect(ids).toEqual([originalId, originalId]);
    expect(signers.ledger.icrc2_approve).toHaveBeenCalledTimes(1);
    expect(journal()).toHaveLength(1);
    expect(journal()[0]).toMatchObject({ id: originalId, status: 'submitted' });
  });

  /** A fake Solana node behind the global fetch: records every request so a spec can see which URL was used. */
  function stubNode() {
    const log: Array<{ url: string; method: string }> = [];
    const stub = vi.fn(async (url: string, init: { body: string }) => {
      const body = JSON.parse(init.body);
      log.push({ url, method: body.method });
      const result =
        body.method === 'getTokenAccountsByOwner'
          ? { value: [{ pubkey: KEYS.holderSource, account: { owner: makeSolanaConfig().tokenProgramId, data: { parsed: { info: { tokenAmount: { amount: '5000000000' } } } } } }] }
          : body.method === 'getLatestBlockhash'
            ? { value: { blockhash: base58Encode(fill(0x77)), lastValidBlockHeight: 1000 } }
            : 900;
      return { ok: true, status: 200, json: async () => ({ jsonrpc: '2.0', id: body.id, result }) };
    });
    vi.stubGlobal('fetch', stub);
    return { log, stub };
  }

  it('with an endpoint configured for this network and genesis, the REAL adapter serves the return flow', async () => {
    mocks.getSolanaRpc.mockImplementation((network: unknown) => mocks.realGetSolanaRpc(network));
    const cfg = makeSolanaConfig();
    mocks.endpoints.push({ networkId: cfg.network.networkId, genesisHash: cfg.network.genesisHash, url: 'https://rpc.test.invalid/solana' });
    const { log } = stubNode();
    await openReturnTab();
    expect(log.map((entry) => entry.method)).toContain('getTokenAccountsByOwner');
    expect(new Set(log.map((entry) => entry.url))).toEqual(new Set(['https://rpc.test.invalid/solana']));
    expect(host.querySelector<HTMLSelectElement>('#sol-ret-source')!.value).toBe(KEYS.holderSource);
    type('#sol-ret-amount', '3');
    click(/^Review transfer$/);
    await settle();
    expect(host.querySelector('[data-testid="return-account"]')!.textContent).toContain(cfg.returnAccount);
    expect(log.map((entry) => entry.method)).toContain('getLatestBlockhash');
    // The adapter is keyless: reviewing asked the wallet for nothing.
    expect(standard.signs).toHaveLength(0);
    expect(log.map((entry) => entry.method)).not.toContain('sendTransaction');
  });

  it('with no endpoint configured, a return cannot be prepared and no node is ever contacted', async () => {
    mocks.getSolanaRpc.mockImplementation((network: unknown) => mocks.realGetSolanaRpc(network));
    const { stub } = stubNode();
    await openReturnTab();
    expect(stub).not.toHaveBeenCalled();
    expect(host.textContent).toContain('no Solana node configured');
    expect((button(/^Review transfer$/) as HTMLButtonElement).disabled).toBe(true);
    expect(host.querySelector<HTMLSelectElement>('#sol-ret-source')!.value).toBe('');
    expect(standard.signs).toHaveLength(0);
  });

  it('ignores an endpoint written for another genesis hash', async () => {
    mocks.getSolanaRpc.mockImplementation((network: unknown) => mocks.realGetSolanaRpc(network));
    const cfg = makeSolanaConfig();
    mocks.endpoints.push({ networkId: cfg.network.networkId, genesisHash: '4uhcVJyU9pJkvQyS88uRDiswHXSCkY3zQawwpjk2NsNY', url: 'https://rpc.test.invalid/other' });
    const { stub } = stubNode();
    await openReturnTab();
    expect(stub).not.toHaveBeenCalled();
    expect((button(/^Review transfer$/) as HTMLButtonElement).disabled).toBe(true);
  });
});

describe('Base is unchanged by the Solana branch', () => {
  it('still defaults to Base and never touches Solana while Base is the one in use', async () => {
    enableSolana();
    render();
    await settle();
    expect(option('base-mainnet')!.getAttribute('aria-checked')).toBe('true');
    expect(host.querySelector('#icp-amount')).toBeTruthy();
    expect(host.querySelector('#tab-icp-to-base')).toBeTruthy();
    expect(host.textContent).toContain('INTERNET COMPUTER ↔ BASE');
    expect(host.querySelector('.subtitle')!.textContent).toContain('between the Internet Computer and Base. Every icUSD on Base is backed one to one');
    expectNoSolanaWalletActivity();
    expect(mocks.createSolanaSigners).not.toHaveBeenCalled();
  });
});
