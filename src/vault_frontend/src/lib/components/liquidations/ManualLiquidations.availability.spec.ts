import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { mount, tick, unmount } from 'svelte';

vi.hoisted(() => { Object.defineProperty(window, 'matchMedia', { configurable: true, value: () => ({ matches: false, addEventListener() {}, removeEventListener() {} }) }); });
const fx = vi.hoisted(() => ({ queue: vi.fn(), all: vi.fn(), status: vi.fn(), liquidate: vi.fn(), partial: vi.fn(), stable: vi.fn(), approve: vi.fn(), allowance: vi.fn() }));
vi.mock('$lib/services/protocol', () => ({ protocolService: {
  getLiquidatableVaults: fx.queue, getAllVaults: fx.all,
  getProtocolStatus: vi.fn().mockResolvedValue({ lastIcpRate: 2 }),
  liquidateVault: fx.liquidate, partialLiquidateVault: fx.partial, partialLiquidateVaultWithStable: fx.stable,
} }));
vi.mock('$lib/services/protocol/apiClient', () => ({ ApiClient: { getPublicData: fx.status } }));
vi.mock('$lib/services/protocol/walletOperations', () => ({ walletOperations: { checkIcusdAllowance: fx.allowance, approveIcusdTransfer: fx.approve }, isOisyWallet: () => true }));
vi.mock('$lib/stores/wallet', async () => {
  const { writable } = await import('svelte/store');
  return { walletStore: Object.assign(writable({ isConnected: true, principal: { toText: () => 'owner' }, tokenBalances: { ICUSD: { formatted: '1000' } } }), { refreshBalance: vi.fn().mockResolvedValue(undefined) }) };
});
vi.mock('$lib/stores/collateralStore', () => ({ collateralStore: {
  fetchSupportedCollateral: vi.fn().mockResolvedValue([]), getCollateralInfo: () => ({ decimals: 8, price: 2, symbol: 'ICP' }), getCollateralColor: () => '#a78bfa',
} }));
vi.mock('$lib/protocol', () => ({ getLiquidationCR: () => 1.33, getMinimumCR: () => 1.5 }));
vi.mock('$lib/services/xrpVaultService', () => ({ XrpVaultService: {} }));
import Page from './ManualLiquidations.svelte';
let target: HTMLElement;
let component: ReturnType<typeof mount>;
const vault = { vault_id: 202, owner: 'owner', borrowed_icusd_amount: 10_000_000_000, collateral_amount: 5_000_000_000, icp_margin_amount: 5_000_000_000, collateral_type: 'ryjl3-tyaaa-aaaaa-aaaba-cai', accrued_interest: 0 };
const liquidate = () => target.querySelector('.btn-liquidate') as HTMLButtonElement;
const refresh = () => (target.querySelector('.summary-refresh') as HTMLButtonElement).click();
async function ready() { await vi.waitFor(() => expect(target.querySelector('.liq-card')).toBeTruthy()); await tick(); }
async function amount() { const input=target.querySelector('.liq-input') as HTMLInputElement; input.value='5';input.dispatchEvent(new Event('input',{bubbles:true}));await tick(); }
beforeEach(() => {
  vi.clearAllMocks();fx.all.mockResolvedValue([]);fx.queue.mockResolvedValue([vault]);fx.status.mockResolvedValue([]);fx.partial.mockResolvedValue({success:false,error:'Vault #202 is locked — bot liquidation in progress'});
  target=document.createElement('div');document.body.append(target);
});
afterEach(async () => { if(component) await unmount(component);target.remove(); });

it('shows a bot-reserved row without offering a manual approval or liquidation', async () => {
  fx.status.mockResolvedValue(BigUint64Array.from([202n]));
  component=mount(Page,{target});await ready();await amount();
  expect(target.textContent).toContain('Reserved for bot liquidation');
  expect(liquidate().disabled).toBe(true);
  expect((target.querySelector('.liq-input') as HTMLInputElement).disabled).toBe(true);
  expect(fx.status).toHaveBeenCalledWith('get_bot_claim_vault_ids');
  expect(fx.approve).not.toHaveBeenCalled();expect(fx.partial).not.toHaveBeenCalled();
});
it('fails closed when public claim availability cannot be read', async () => {
  fx.status.mockRejectedValue(new Error('offline'));
  component=mount(Page,{target});await ready();await amount();
  expect(target.textContent).toContain('Could not verify liquidation availability');
  expect(liquidate().disabled).toBe(true);expect(fx.approve).not.toHaveBeenCalled();expect(fx.partial).not.toHaveBeenCalled();
});
it('keeps a fresh unlocked row usable and disables it after a later backend lock rejection', async () => {
  component=mount(Page,{target});await ready();await amount();
  expect(liquidate().disabled).toBe(false);liquidate().click();
  await vi.waitFor(()=>expect(fx.partial).toHaveBeenCalledOnce());
  await vi.waitFor(()=>expect(target.textContent).toContain('Reserved for bot liquidation'));
  expect(liquidate().disabled).toBe(true);
  liquidate().click();expect(fx.partial).toHaveBeenCalledOnce();
  refresh();await vi.waitFor(()=>expect(fx.status).toHaveBeenCalledTimes(2));
  await vi.waitFor(()=>expect(liquidate().disabled).toBe(false));
});
it('keeps manual actions disabled while refreshing availability', async () => {
  component=mount(Page,{target});await ready();await amount();
  let resolve!: (ids: bigint[]) => void;
  fx.status.mockImplementationOnce(()=>new Promise<bigint[]>(done=>{resolve=done;}));
  refresh();await tick();expect(liquidate().disabled).toBe(true);
  expect(target.textContent).toContain('Checking liquidation availability');
  resolve([]);await vi.waitFor(()=>expect(liquidate().disabled).toBe(false));
});
it('does not let an older in-flight read clear a newer backend lock rejection', async () => {
  let rejectAction!: (result: unknown) => void;
  fx.partial.mockImplementationOnce(()=>new Promise(done=>{rejectAction=done;}));
  component=mount(Page,{target});await ready();await amount();liquidate().click();
  await vi.waitFor(()=>expect(fx.partial).toHaveBeenCalledOnce());
  let resolveStatus!: (ids: bigint[]) => void;
  fx.status.mockImplementationOnce(()=>new Promise<bigint[]>(done=>{resolveStatus=done;}));
  refresh();await vi.waitFor(()=>expect(fx.status).toHaveBeenCalledTimes(2));
  expect(liquidate().textContent).toContain('Liquidating…');
  rejectAction({success:false,error:'Vault #202 is locked — bot liquidation in progress'});
  await vi.waitFor(()=>expect(target.textContent).toContain('Reserved for bot liquidation'));
  resolveStatus([]);await tick();await tick();
  expect(liquidate().disabled).toBe(true);expect(target.textContent).toContain('Reserved for bot liquidation');
});

it('shows a redemption claim as suspended rather than zero nICP and a liquidation gauge',async()=>{
 fx.all.mockResolvedValue([{...vault,vault_id:51,collateral_amount:0,icp_margin_amount:0,nicp_redemption_claim:[{stage:{ClaimConfirmed:null},icp_due_e8s:190_000_000_000n,received_icp_e8s:[],withdrawal_id:[5n]}]}]);
 component=mount(Page,{target});
 await vi.waitFor(()=>expect(target.querySelector('.ov-card')?.textContent).toContain('ICP redemption claim'));
 const card=target.querySelector('.ov-card')!;
 expect(card.textContent).toContain('1,900 ICP');
 expect(card.textContent).toContain('Liquidation suspended');
 expect(card.querySelector('.ov-gauge-track')).toBeNull();
 expect(card.textContent).not.toContain('0.0%');
 expect(fx.approve).not.toHaveBeenCalled();expect(fx.partial).not.toHaveBeenCalled();
});
