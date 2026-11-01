import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import XrpDepositTagNotice from './XrpDepositTagNotice.svelte';

const root = resolve(__dirname, '../..');
let host: HTMLDivElement;
let instance: ReturnType<typeof mount> | undefined;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  if (instance) unmount(instance);
  instance = undefined;
  host.remove();
});

describe('XRP incoming deposit tag guidance', () => {
  it('mounts clear instructions for unique Rumi vault addresses', () => {
    instance = mount(XrpDepositTagNotice, { target: host });
    flushSync();

    expect(host.querySelector('[role="note"]')?.textContent).toContain('No destination tag or memo is needed.');
    expect(host.textContent).toContain('Rumi gives each vault its own XRP deposit address.');
    expect(host.textContent).toContain('choose “No tag” or “No memo.”');
    expect(host.textContent).toContain('ask its support');
    expect(host.textContent).not.toMatch(/use (a )?(random|fake|zero|0) tag/i);
  });

  it('keeps the same guidance beside every incoming address surface and preserves the outgoing tag input', () => {
    const modal = readFileSync(resolve(root, 'components/borrow/XrpBorrowModal.svelte'), 'utf8');
    const panel = readFileSync(resolve(root, 'components/vault/XrpVaultPanel.svelte'), 'utf8');
    const banner = readFileSync(resolve(root, 'components/vault/XrpPendingDepositBanner.svelte'), 'utf8');
    const service = readFileSync(resolve(root, 'services/xrpVaultService.ts'), 'utf8');

    expect(modal).toContain('<XrpDepositTagNotice />');
    expect(panel.match(/<XrpDepositTagNotice \/>/g)).toHaveLength(2);
    expect(banner).toContain('<XrpDepositTagNotice />');
    expect(panel).toContain('placeholder="Recipient destination tag (optional)"');
    expect(panel).toContain('inputmode="numeric"');
    expect(service).toContain('actor.settle_xrp_claim_with_tag(claimIdBig, destination, destinationTag)');
  });
});
