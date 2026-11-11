import { Signer } from '@icp-sdk/signer';
import { SignerAgent } from '@icp-sdk/signer/agent';
import { PostMessageTransport } from '@icp-sdk/signer/web';
import { Principal as SdkPrincipal } from '@icp-sdk/core/principal';
import type { Principal } from '@dfinity/principal';

export class OisySwapOwnerMismatchError extends Error {
	constructor() {
		super(
			'OISY is using a different account from the connected Rumi account. Reconnect the same OISY account and refresh the quote.'
		);
		this.name = 'OisySwapOwnerMismatchError';
	}
}
export function isOisySenderMismatch(error: unknown): boolean {
	return (
		typeof error === 'object' &&
		error !== null &&
		'code' in error &&
		error.code === 502 &&
		'message' in error &&
		error.message === 'The sender must match the owner of the signer.'
	);
}
let active: object | null = null;
/** One click owns its channel; it never borrows a cached signer from another session. */
export function beginSwapOisyAction(owner: Principal, assertCurrent: () => void) {
	assertCurrent();
	if (owner.isAnonymous()) throw new Error('Connect an ICP wallet first.');
	if (active) throw new Error('Another swap wallet request is active. Wait for its result.');
	const ownerText = owner.toText(),
		context = {};
	const popupName = `rumi-swap-${crypto.randomUUID()}`;
	const actionWindow = {
		open: (url?: string | URL, _target?: string, features?: string) =>
			window.open(url, popupName, features),
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
	const agent = SignerAgent.createSync({ signer, account: SdkPrincipal.fromText(ownerText) });
	active = context;
	// Open synchronously before any fee lookup, queue or network await loses the click.
	const ready = signer.openChannel().then(() => {
		assertCurrent();
	});
	let closing: Promise<void> | null = null;
	const readOwnerTexts = async () => {
		assertCurrent();
		const accounts = await signer.getAccounts();
		assertCurrent();
		return accounts.map((account) => account.owner.toText());
	};
	return {
		agent,
		ready,
		readOwnerTexts,
		async verifyOwner() {
			if (!(await readOwnerTexts()).includes(ownerText)) throw new OisySwapOwnerMismatchError();
		},
		close() {
			if (!closing) {
				if (active === context) active = null;
				closing = signer.closeChannel();
			}
			return closing;
		}
	};
}

/** Explicit user-click inspection: accounts RPC only, no canister call or owner adoption. */
export async function inspectOisySwapAccount(owner: Principal, assertCurrent: () => void) {
	const action = beginSwapOisyAction(owner, assertCurrent);
	try {
		await action.ready;
		const signerOwners = await action.readOwnerTexts();
		assertCurrent();
		const connectedOwner = owner.toText();
		return { connectedOwner, signerOwners, matches: signerOwners.includes(connectedOwner) };
	} finally {
		await action.close().catch(() => {});
	}
}
