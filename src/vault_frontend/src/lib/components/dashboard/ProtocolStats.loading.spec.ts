import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { mount, tick, unmount } from 'svelte';

const fx = vi.hoisted(() => ({
  collateralTotals: vi.fn(),
  redemptionClaim: vi.fn(),
  fetchSupportedCollateral: vi.fn(),
  tokenBalance: vi.fn(),
}));

vi.mock('$lib/services/protocol', () => ({ protocolService: { getProtocolStatus: vi.fn() } }));
vi.mock('$lib/services/protocol/apiClient', () => ({
  publicActor: {
    get_collateral_totals: fx.collateralTotals,
    get_nicp_redemption_claim: fx.redemptionClaim,
  },
}));
vi.mock('$lib/services/tokenService', () => ({ TokenService: { getTokenBalance: fx.tokenBalance } }));
vi.mock('$lib/config', () => ({ CANISTER_IDS: {
  PROTOCOL: 'tfesu-vyaaa-aaaap-qrd7a-cai',
  CKUSDT_LEDGER: 'cngnf-vqaaa-aaaar-qag4q-cai',
  CKUSDC_LEDGER: 'xevnm-gaaaa-aaaar-qafnq-cai',
} }));
vi.mock('$lib/stores/collateralStore', async () => {
  const { writable } = await import('svelte/store');
  const collaterals = [
    {
      principal: 'icp-ledger', symbol: 'ICP', decimals: 8, color: '#2DD4BF', price: 10,
      status: 'Active', minimumCr: 1.5, liquidationCr: 1.33,
    },
    {
      principal: 'buwm7-7yaaa-aaaar-qagva-cai', symbol: 'nICP', decimals: 8,
      color: '#a78bfa', price: 10, status: 'Active', lstHaircut: 0.25,
    },
  ];
  return {
    collateralStore: Object.assign(writable({ collaterals, loading: false, error: null, lastFetch: 0 }), {
      fetchSupportedCollateral: fx.fetchSupportedCollateral,
    }),
    COLLATERAL_DISPLAY_ORDER: ['ICP', 'nICP'],
  };
});

import ProtocolStats from './ProtocolStats.svelte';

let target: HTMLElement;
let component: ReturnType<typeof mount> | undefined;

const status = {
  mode: 'GeneralAvailability',
  totalIcpMargin: 2,
  totalIcusdBorrowed: 1,
  lastIcpRate: 10,
  lastIcpTimestamp: 0,
  totalCollateralRatio: 2,
  liquidationBonus: 1.15,
  recoveryTargetCr: 1.55,
  recoveryModeThreshold: 1.5,
  recoveryCrMultiplier: 1,
};

const collateralTotals = [{
  collateral_type: { toText: () => 'icp-ledger' },
  decimals: 8,
  total_collateral: 1_000_000_000n,
  price: 10,
}];

const claim = [{
  stage: { ClaimConfirmed: null },
  icp_due_e8s: 200_000_000n,
  received_icp_e8s: [],
  withdrawal_id: [1n],
}];

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => { resolve = res; });
  return { promise, resolve };
}

function statValue(label: string): HTMLElement {
  const row = [...target.querySelectorAll<HTMLElement>('.stat-row')]
    .find((element) => element.querySelector('.stat-label')?.textContent?.trim() === label);
  const value = row?.querySelector<HTMLElement>('.stat-value, .stat-value-stack');
  if (!value) throw new Error(`Missing stat row: ${label}`);
  return value;
}

beforeEach(() => {
  vi.clearAllMocks();
  fx.collateralTotals.mockResolvedValue(collateralTotals);
  fx.redemptionClaim.mockResolvedValue(claim);
  fx.fetchSupportedCollateral.mockResolvedValue([]);
  fx.tokenBalance.mockResolvedValue(0n);
  target = document.createElement('div');
  document.body.append(target);
});

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  target.remove();
});

it('waits for the claim before replacing the partial ICP subtotal with full collateral value', async () => {
  const claimRead = deferred<typeof claim>();
  const reserveRead = deferred<bigint>();
  fx.redemptionClaim.mockReturnValue(claimRead.promise);
  fx.tokenBalance.mockReturnValue(reserveRead.promise);

  component = mount(ProtocolStats, { target, props: { protocolStatus: status } });
  await vi.waitFor(() => expect(fx.redemptionClaim).toHaveBeenCalledOnce());
  await tick();

  expect(statValue('Collateral Value').textContent).toBe('…');
  expect(statValue('Collateral Value').textContent).not.toContain('$20');
  expect(statValue('Reserves').textContent).toContain('Loading…');

  claimRead.resolve(claim);
  await vi.waitFor(() => expect(statValue('Collateral Value').textContent).toContain('$115'));
  expect(statValue('Collateral Value').textContent).not.toContain('$120');

  reserveRead.resolve(0n);
});

it('shows collateral value as unavailable when the claim read fails', async () => {
  fx.redemptionClaim.mockRejectedValue(new Error('claim query unavailable'));

  component = mount(ProtocolStats, { target, props: { protocolStatus: status } });
  await vi.waitFor(() => expect(statValue('Collateral Value').textContent).toBe('Unavailable'));

  expect(statValue('Collateral (excl. sunset)').textContent).toContain('Unavailable');
  expect(statValue('Collateral Value').textContent).not.toContain('$20');
});
