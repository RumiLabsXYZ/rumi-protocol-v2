import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { Principal } from '@dfinity/principal';
import { Principal as SdkPrincipal } from '@icp-sdk/core/principal';
const fx = vi.hoisted(() => ({
	click: false,
	owner: '',
	failOpen: false,
	accountRows: null as null | { owner: string; subaccount?: string }[],
	requests: [] as any[],
	windows: new Map<string, any>()
}));
vi.mock('@icp-sdk/signer/web', () => ({
	PostMessageTransport: class {
		constructor(private options: any) {}
		establishChannel() {
			if (!fx.click || fx.failOpen) return Promise.reject(new Error('Synthetic popup unavailable'));
			const popup = this.options.window.open(
				this.options.url,
				'https://oisy.com-signer-window',
				''
			);
			const handlers = new Map<string, Set<(v?: any) => void>>();
			return Promise.resolve({
				get closed() {
					return popup.closed;
				},
				close() {
					popup.close();
					for (const fn of handlers.get('close') ?? []) fn();
					return Promise.resolve();
				},
				addEventListener(type: string, fn: any) {
					if (!handlers.has(type)) handlers.set(type, new Set());
					handlers.get(type)!.add(fn);
					return () => handlers.get(type)!.delete(fn);
				},
				send(request: any) {
					fx.requests.push(request);
					queueMicrotask(() => {
						const response =
							request.method === 'icrc27_accounts'
								? { result: { accounts: fx.accountRows ?? [{ owner: fx.owner }] } }
								: request.params?.sender !== fx.owner
									? {
											error: {
												code: 502,
												message: 'The sender must match the owner of the signer.'
											}
										}
									: { error: { code: 4000, message: 'Synthetic execution is disabled' } };
						for (const fn of handlers.get('response') ?? [])
							fn({ jsonrpc: '2.0', id: request.id, ...response });
					});
					return Promise.resolve();
				}
			});
		}
	}
}));
import {
	beginSwapOisyAction,
	OisySwapOwnerMismatchError,
	isOisySenderMismatch,
	inspectOisySwapAccount
} from './swapOisySigner';
const a = Principal.fromText('rrkah-fqaaa-aaaaa-aaaaq-cai'),
	b = Principal.fromText('ryjl3-tyaaa-aaaaa-aaaba-cai');
let current: ReturnType<typeof beginSwapOisyAction> | null = null;
function click<T>(fn: () => T) {
	fx.click = true;
	try {
		return fn();
	} finally {
		fx.click = false;
	}
}
beforeEach(() => {
	fx.owner = a.toText();
	fx.requests = [];
	fx.failOpen = false;
	fx.accountRows = null;
	fx.windows.clear();
	vi.spyOn(window, 'open').mockImplementation((_url, name) => {
		const key = String(name);
		if (!fx.windows.has(key))
			fx.windows.set(key, {
				closed: false,
				close() {
					this.closed = true;
				}
			});
		return fx.windows.get(key);
	});
});
afterEach(async () => {
	await current?.close().catch(() => {});
	current = null;
	vi.restoreAllMocks();
});
describe('action-owned OISY swap signer using installed SDK', () => {
	it('opens in the click, preserves the expected cross-library principal and verifies only read-only accounts', async () => {
		current = click(() => beginSwapOisyAction(a, () => {}));
		expect(window.open).toHaveBeenCalledOnce();
		await current.ready;
		expect((await current.agent.getPrincipal()).toText()).toBe(a.toText());
		await current.verifyOwner();
		expect(fx.requests.map((r) => r.method)).toEqual(['icrc27_accounts']);
	});
	it('rejects a different signer account instead of silently selecting it', async () => {
		fx.owner = b.toText();
		current = click(() => beginSwapOisyAction(a, () => {}));
		await current.ready;
		await expect(current.verifyOwner()).rejects.toBeInstanceOf(OisySwapOwnerMismatchError);
		expect(fx.requests.every((r) => r.method === 'icrc27_accounts')).toBe(true);
	});
	it('reproduces the exact RPC502 with the real SDK and fixed sender when the wallet owner differs', async () => {
		current = click(() => beginSwapOisyAction(a, () => {}));
		await current.ready;
		fx.owner = b.toText();
		await expect(
			current.agent.call(b.toText(), {
				methodName: 'synthetic_never_sent',
				arg: new Uint8Array(),
				effectiveCanisterId: SdkPrincipal.fromText(b.toText())
			})
		).rejects.toMatchObject({
			code: 502,
			message: 'The sender must match the owner of the signer.'
		});
		expect(fx.requests.find((r) => r.method === 'icrc49_call_canister').params.sender).toBe(
			a.toText()
		);
	});
	it('rejects session replacement while an account read is pending', async () => {
		let live = true;
		current = click(() =>
			beginSwapOisyAction(a, () => {
				if (!live) throw Error('session changed');
			})
		);
		await current.ready;
		const read = current.verifyOwner();
		live = false;
		await expect(read).rejects.toThrow('session changed');
	});
	it('uses a unique popup and releases it after open failure without reusing the old signer', async () => {
		fx.failOpen = true;
		current = click(() => beginSwapOisyAction(a, () => {}));
		await expect(current.ready).rejects.toThrow('popup unavailable');
		await current.close();
		fx.failOpen = false;
		current = click(() => beginSwapOisyAction(a, () => {}));
		await current.ready;
		expect(window.open).toHaveBeenCalledOnce();
		expect(String(vi.mocked(window.open).mock.calls[0][1])).toMatch(/^rumi-swap-/);
		expect(() => click(() => beginSwapOisyAction(a, () => {}))).toThrow('request is active');
	});
	it('does not share the default cached signer popup or let its closure end this action', async () => {
		const shared = window.open('https://oisy.com/sign', 'https://oisy.com-signer-window');
		current = click(() => beginSwapOisyAction(a, () => {}));
		await current.ready;
		shared!.close();
		await current.verifyOwner();
		const actionPopup = [...fx.windows.entries()].find(([key]) => key.startsWith('rumi-swap-'))![1];
		expect(actionPopup.closed).toBe(false);
	});
	it('classifies only the exact sender-not-allowed response as a pre-submission owner rejection', () => {
		expect(
			isOisySenderMismatch({ code: 502, message: 'The sender must match the owner of the signer.' })
		).toBe(true);
		expect(isOisySenderMismatch({ code: 4000, message: 'network timeout' })).toBe(false);
		expect(isOisySenderMismatch(new Error('The sender must match the owner of the signer.'))).toBe(
			false
		);
	});
});

describe('read-only OISY swap account inspection', () => {
	it('decodes the published account response shape using the real SDK without any canister call', async () => {
		fx.accountRows = [
			{ owner: a.toText(), subaccount: btoa(String.fromCharCode(...new Uint8Array(32))) }
		];
		const report = await click(() => inspectOisySwapAccount(a, () => {}));
		expect(report).toEqual({
			connectedOwner: a.toText(),
			signerOwners: [a.toText()],
			matches: true
		});
		expect(fx.requests.map((r) => r.method)).toEqual(['icrc27_accounts']);
	});
	it('reports a different owner without adopting it or asking for approval', async () => {
		fx.owner = b.toText();
		const report = await click(() => inspectOisySwapAccount(a, () => {}));
		expect(report).toEqual({
			connectedOwner: a.toText(),
			signerOwners: [b.toText()],
			matches: false
		});
		expect(fx.requests.map((r) => r.method)).toEqual(['icrc27_accounts']);
	});
	it('reports an empty account list without claiming a match', async () => {
		fx.accountRows = [];
		expect((await click(() => inspectOisySwapAccount(a, () => {}))).matches).toBe(false);
		expect(fx.requests.every((r) => r.method === 'icrc27_accounts')).toBe(true);
	});
	it('refuses a stale session during the account response and releases its channel', async () => {
		let live = true;
		const report = click(() =>
			inspectOisySwapAccount(a, () => {
				if (!live) throw Error('session changed');
			})
		);
		live = false;
		await expect(report).rejects.toThrow('session changed');
		current = click(() => beginSwapOisyAction(a, () => {}));
		await current.ready;
		expect(fx.requests.every((r) => r.method === 'icrc27_accounts')).toBe(true);
	});
});
