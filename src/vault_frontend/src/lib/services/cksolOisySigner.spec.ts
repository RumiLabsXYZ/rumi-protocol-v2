import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest';
import { Principal } from '@dfinity/principal';

const fx = vi.hoisted(() => ({ windows: new Map<string, any>(), inClick: false, failOpen: false }));
vi.mock('@icp-sdk/signer/web', () => ({
	PostMessageTransport: class {
		constructor(private options: any) {}
		establishChannel() {
			if (!fx.inClick)
				return Promise.reject(
					new Error('Signer window should not be opened outside of click handler')
				);
			if (fx.failOpen) return Promise.reject(new Error('Signer window could not be opened'));
			const popup = (this.options.window ?? window).open(
				this.options.url,
				'https://oisy.com-signer-window',
				''
			);
			const listeners = new Map<string, Set<(event?: any) => void>>();
			return Promise.resolve({
				get closed() {
					return popup.closed;
				},
				close() {
					popup.close();
					for (const f of listeners.get('close') ?? []) f();
					return Promise.resolve();
				},
				addEventListener(type: string, fn: (event?: any) => void) {
					if (!listeners.has(type)) listeners.set(type, new Set());
					listeners.get(type)!.add(fn);
					return () => listeners.get(type)?.delete(fn);
				},
				send(request: any) {
					queueMicrotask(() => {
						for (const f of listeners.get('response') ?? [])
							f({ jsonrpc: '2.0', id: request.id, result: { ok: true } });
					});
					return Promise.resolve();
				}
			});
		}
	}
}));
vi.mock('@icp-sdk/signer/agent', () => ({
	SignerAgent: {
		createSync: (options: any) => ({
			signer: options.signer,
			getPrincipal: async () => options.account
		})
	}
}));
import { beginCksolOisyAction, getCksolOisySignerAgent } from './cksolOisySigner';
const owner = Principal.fromText('rrkah-fqaaa-aaaaa-aaaaq-cai');
const other = Principal.fromText('ryjl3-tyaaa-aaaaa-aaaba-cai');
function click<T>(fn: () => T): T {
	fx.inClick = true;
	try {
		return fn();
	} finally {
		fx.inClick = false;
	}
}
beforeEach(() => {
	vi.useFakeTimers();
	fx.windows.clear();
	fx.failOpen = false;
	vi.spyOn(window, 'open').mockImplementation((_url: any, name: any) => {
		let popup = fx.windows.get(name);
		if (!popup || popup.closed) {
			popup = {
				closed: false,
				close() {
					this.closed = true;
				}
			};
			fx.windows.set(name, popup);
		}
		return popup;
	});
});
afterEach(() => {
	vi.useRealTimers();
	vi.restoreAllMocks();
});
describe('ckSOL OISY action channel', () => {
	it('keeps its own window through a background popup close and network preflight', async () => {
		const legacy = window.open('https://oisy.com/sign', 'https://oisy.com-signer-window')!;
		const action = click(() => beginCksolOisyAction(owner));
		try {
			await action.ready;
			const agent = getCksolOisySignerAgent(owner);
			await agent.signer.sendRequest({ jsonrpc: '2.0', id: 'first', method: 'test' });
			legacy.close();
			await vi.advanceTimersByTimeAsync(500);
			const response = await agent.signer.sendRequest({
				jsonrpc: '2.0',
				id: 'after-preflight',
				method: 'test'
			});
			expect(response.result).toEqual({ ok: true });
			expect([...fx.windows.values()].filter((p) => !p.closed)).toHaveLength(1);
		} finally {
			await action.close();
		}
		expect([...fx.windows.values()].every((p) => p.closed)).toBe(true);
		expect(() => getCksolOisySignerAgent(owner)).toThrow('click');
	});
	it('starts fresh after a failed channel opening and binds the action to its owner', async () => {
		fx.failOpen = true;
		const failed = click(() => beginCksolOisyAction(owner));
		await expect(failed.ready).rejects.toThrow('could not be opened');
		await failed.close();
		fx.failOpen = false;
		const next = click(() => beginCksolOisyAction(owner));
		try {
			await next.ready;
			expect(() => getCksolOisySignerAgent(other)).toThrow('owner');
			await expect(
				getCksolOisySignerAgent(owner).signer.sendRequest({
					jsonrpc: '2.0',
					id: 'retry',
					method: 'test'
				})
			).resolves.toHaveProperty('result');
		} finally {
			await next.close();
		}
	});
});
