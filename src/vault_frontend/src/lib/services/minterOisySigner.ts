import { Signer } from '@icp-sdk/signer';
import { SignerAgent } from '@icp-sdk/signer/agent';
import { PostMessageTransport } from '@icp-sdk/signer/web';
import { Principal as SdkPrincipal } from '@icp-sdk/core/principal';
import type { Principal } from '@dfinity/principal';

export interface MinterOisyAction {
	ready: Promise<void>;
	close(): Promise<void>;
}
let active: { owner: string; agent: any; ready: boolean } | null = null;

export function beginMinterOisyAction(owner: Principal): MinterOisyAction {
	if (owner.isAnonymous()) throw new Error('Connect an ICP wallet first.');
	if (active) throw new Error('A minter wallet request is already active. Wait for its result.');
	// PNP's older signer uses the SDK's default window name and can close it
	// independently. Each minter action owns a separate popup and signer.
	const popupName = `rumi-minter-${crypto.randomUUID()}`;
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
	const agent = SignerAgent.createSync({ signer, account: SdkPrincipal.fromText(owner.toText()) });
	const context = { owner: owner.toText(), agent, ready: false };
	active = context;
	const ready = signer.openChannel().then(() => {
		context.ready = true;
	});
	let closing: Promise<void> | null = null;
	return {
		ready,
        close() {
            if (!closing) {
                if (active === context) active = null;
                closing = signer.closeChannel();
            }
            return closing;
        }
	};
}

export function getMinterOisySignerAgent(owner: Principal): any {
	if (!active || !active.ready)
		throw new Error('Open the minter wallet channel from its button click first.');
	if (active.owner !== owner.toText()) throw new Error('minter signer owner changed.');
	return active.agent;
}
