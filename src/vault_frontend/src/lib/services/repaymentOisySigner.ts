import { Signer } from '@icp-sdk/signer';
import { SignerAgent } from '@icp-sdk/signer/agent';
import { PostMessageTransport } from '@icp-sdk/signer/web';
import { Principal as SdkPrincipal } from '@icp-sdk/core/principal';
import type { Principal } from '@dfinity/principal';

export interface RepaymentOisyAction {
	ready: Promise<void>;
	close(): Promise<void>;
}
let active: { owner: string; agent: any; ready: boolean; assertCurrent: () => void } | null = null;

/** One repayment owns its popup and agent through approval and repayment. */
export function beginRepaymentOisyAction(
	owner: Principal,
	assertCurrent: () => void
): RepaymentOisyAction {
	assertCurrent();
	if (owner.isAnonymous()) throw new Error('Connect the vault owner wallet first.');
	if (active) throw new Error('Another repayment wallet request is active. Wait for its result.');
	// PNP's legacy signer can close its default window independently. Never
	// share that named window or its idle timer with this financial action.
	const popupName = `rumi-repayment-${crypto.randomUUID()}`;
	let canceled = false;
	let popup: Window | null = null;
	const actionWindow = {
		open: (url?: string | URL, _target?: string, features?: string) => {
			if (canceled) throw new Error('Repayment wallet action was closed.');
			popup = window.open(url, popupName, features);
			return popup;
		},
		addEventListener: window.addEventListener.bind(window),
		removeEventListener: window.removeEventListener.bind(window),
		focus: window.focus.bind(window)
	} as Window;
	const signer = new Signer({
		autoCloseTransportChannel: false,
		transport: new PostMessageTransport({
			window: actionWindow,
			url: 'https://oisy.com/sign',
			windowOpenerFeatures: 'toolbar=0,location=0,menubar=0,width=525,height=705'
		})
	});
	const agent = SignerAgent.createSync({ signer, account: SdkPrincipal.fromText(owner.toText()) });
	const context = { owner: owner.toText(), agent, ready: false, assertCurrent };
	active = context;
	// Invoke synchronously from the click, before any actor or preflight await.
	const ready = signer.openChannel().then(() => {
		if (canceled) throw new Error('Repayment wallet action was closed.');
		assertCurrent();
		context.ready = true;
	});
	let closing: Promise<void> | null = null;
	return {
		ready,
		close() {
			if (!closing) {
				canceled = true;
				context.ready = false;
				if (active === context) active = null;
				// SDK closeChannel only sees an established channel. Close the
				// owned window now, then also clean up a late handshake channel.
				popup?.close();
				closing = Promise.allSettled([ready, signer.closeChannel()]).then(() =>
					signer.closeChannel()
				);
			}
			return closing;
		}
	};
}

export function getRepaymentOisySignerAgent(owner: Principal): any {
	if (!active || !active.ready)
		throw new Error('Open the repayment wallet channel from its button click first.');
	if (active.owner !== owner.toText()) throw new Error('Repayment signer owner changed.');
	active.assertCurrent();
	return active.agent;
}
