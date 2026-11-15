import { mount, unmount, tick } from 'svelte';
import type { Writable } from 'svelte/store';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { Principal } from '@dfinity/principal';
const fx = vi.hoisted(() => ({ inspect: vi.fn() }));
vi.mock('../../services/swapOisySigner', () => ({ inspectOisySwapAccount: fx.inspect }));
vi.mock('../../services/auth', async () => {
	const { writable } = await import('svelte/store');
	return { currentWalletType: writable('oisy'), walletSessionGeneration: writable(0) };
});
vi.mock('../../stores/wallet', async () => {
	const { writable } = await import('svelte/store');
	return { walletStore: writable({ isConnected: true, loading: false, principal: null }) };
});
import Check from './OisyAccountCheck.svelte';
import { currentWalletType, walletSessionGeneration } from '../../services/auth';
import { walletStore } from '../../stores/wallet';
const wallet = walletStore as unknown as Writable<any>;
const owner = Principal.fromText('rrkah-fqaaa-aaaaa-aaaaq-cai'),
	other = Principal.fromText('ryjl3-tyaaa-aaaaa-aaaba-cai');
let target: HTMLElement, component: ReturnType<typeof mount> | null;
beforeEach(() => {
	fx.inspect
		.mockReset()
		.mockResolvedValue({
			connectedOwner: owner.toText(),
			signerOwners: [owner.toText()],
			matches: true
		});
	currentWalletType.set('oisy');
	walletSessionGeneration.set(0);
	wallet.set({ isConnected: true, loading: false, principal: owner });
	target = document.createElement('div');
	document.body.append(target);
	component = null;
});
afterEach(async () => {
	if (component) await unmount(component);
	target.remove();
});
async function show() {
	component = mount(Check, { target });
	await tick();
}
describe('OISY read-only identity inspection', () => {
	it('does not open a wallet automatically and an explicit click requests only inspection', async () => {
		await show();
		expect(fx.inspect).not.toHaveBeenCalled();
		const button = target.querySelector('button')!;
		expect(button.textContent).toContain('read only');
		button.click();
		await vi.waitFor(() => expect(fx.inspect).toHaveBeenCalledOnce());
		await tick();
		expect(fx.inspect.mock.calls[0][0].toText()).toBe(owner.toText());
		expect(target.textContent).toContain('matches this Rumi connection');
		expect(target.textContent).toContain('does not verify earlier approvals or swaps');
	});
	it('shows a mismatched owner without changing the connected account', async () => {
		fx.inspect.mockResolvedValue({
			connectedOwner: owner.toText(),
			signerOwners: [other.toText()],
			matches: false
		});
		await show();
		target.querySelector('button')!.click();
		await vi.waitFor(() => expect(target.textContent).toContain('does not match'));
		expect(target.textContent).toContain(owner.toText());
		expect(target.textContent).toContain(other.toText());
		let actual: any;
		const off = wallet.subscribe((v) => (actual = v));
		off();
		expect(actual.principal.toText()).toBe(owner.toText());
	});
	it('prevents duplicate clicks while a read is pending', async () => {
		let finish!: (v: any) => void;
		fx.inspect.mockReturnValue(new Promise((r) => (finish = r)));
		await show();
		const b = target.querySelector('button')!;
		b.click();
		await tick();
		expect(b.disabled).toBe(true);
		b.click();
		expect(fx.inspect).toHaveBeenCalledOnce();
		finish({ connectedOwner: owner.toText(), signerOwners: [], matches: false });
		await vi.waitFor(() => expect(target.textContent).toContain('None returned'));
	});
	it('discards a response after the same-owner session changes', async () => {
		let finish!: (v: any) => void;
		fx.inspect.mockReturnValue(new Promise((r) => (finish = r)));
		await show();
		target.querySelector('button')!.click();
		walletSessionGeneration.set(1);
		await tick();
		finish({ connectedOwner: owner.toText(), signerOwners: [other.toText()], matches: false });
		await tick();
		await tick();
		expect(target.querySelector('[role="status"]')).toBeNull();
		expect(target.querySelector('[role="alert"]')).toBeNull();
	});
	it('hides old results when the provider changes', async () => {
		await show();
		target.querySelector('button')!.click();
		await vi.waitFor(() => expect(target.querySelector('[role="status"]')).not.toBeNull());
		currentWalletType.set('plug');
		await tick();
		expect(target.querySelector('section')).toBeNull();
		expect(fx.inspect).toHaveBeenCalledOnce();
	});
	it('reports an unavailable read without inventing an account mismatch', async () => {
		fx.inspect.mockRejectedValue(new Error('Account permission declined'));
		await show();
		target.querySelector('button')!.click();
		await vi.waitFor(() => expect(target.textContent).toContain('Account permission declined'));
		expect(target.querySelector('[role="status"]')).toBeNull();
	});
});
