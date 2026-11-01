import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import DestinationSelector from './DestinationSelector.svelte';
import { unavailableEvmDestinations, type DestinationView } from '$lib/services/baseBridge/destinations';
import { makeDestination } from '$lib/services/baseBridge/testSupport';

let host: HTMLDivElement;
let instance: ReturnType<typeof mount> | undefined;
let onSelect: ReturnType<typeof vi.fn>;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  onSelect = vi.fn();
});

afterEach(() => {
  if (instance) unmount(instance);
  instance = undefined;
  host.remove();
});

/** Base first, then the five static disabled entries. */
function oneEnabled(): DestinationView[] {
  return [makeDestination(), ...unavailableEvmDestinations()];
}

/** A future state with a second live instance: a disabled entry sits between the two enabled ones. */
function twoEnabled(): DestinationView[] {
  const [ethereum, arbitrum, ...rest] = unavailableEvmDestinations();
  return [makeDestination(), ethereum, { ...arbitrum, status: 'enabled', reason: null }, ...rest];
}

function render(destinations: DestinationView[], selectedId: string | null = 'base-mainnet') {
  instance = mount(DestinationSelector, { target: host, props: { destinations, selectedId, onSelect } });
  flushSync();
}

const option = (id: string) => host.querySelector<HTMLElement>(`#destination-${id}`)!;
const key = (name: string) => {
  host.querySelector('[role="radiogroup"]')!.dispatchEvent(new KeyboardEvent('keydown', { key: name, bubbles: true, cancelable: true }));
  flushSync();
};

describe('DestinationSelector', () => {
  it('lists every destination as a radio, with the selected one checked', () => {
    render(oneEnabled());
    const radios = Array.from(host.querySelectorAll('[role="radio"]'));
    expect(radios).toHaveLength(6);
    expect(host.querySelector('[role="radiogroup"]')!.getAttribute('aria-label')).toBe('Destination network');
    expect(option('base-mainnet').getAttribute('aria-checked')).toBe('true');
    for (const id of ['ethereum-mainnet', 'arbitrum-mainnet', 'robinhood-mainnet', 'hyperevm-mainnet', 'monad-mainnet']) {
      expect(option(id).getAttribute('aria-checked'), id).toBe('false');
    }
    expect(option('base-mainnet').textContent).toContain('Mainnet');
    expect(option('base-mainnet').textContent).toContain('Chain 8453');
  });

  it('marks a disabled entry with its reason, aria-disabled, and no way to focus or activate it', () => {
    render(oneEnabled());
    for (const id of ['ethereum-mainnet', 'arbitrum-mainnet', 'robinhood-mainnet', 'hyperevm-mainnet', 'monad-mainnet']) {
      const entry = option(id);
      expect(entry.getAttribute('aria-disabled'), id).toBe('true');
      expect(entry.tagName).toBe('DIV'); // not a button
      expect(entry.hasAttribute('tabindex')).toBe(false);
      expect(entry.textContent).toContain('Unavailable');
      expect(entry.textContent).toContain('Not available yet: there is no icUSD bridge to');
    }
    expect(option('base-mainnet').hasAttribute('aria-disabled')).toBe(false);
    expect(option('base-mainnet').tagName).toBe('BUTTON');
  });

  it('has exactly one tab stop: the selected enabled entry', () => {
    render(oneEnabled());
    const stops = Array.from(host.querySelectorAll('[role="radio"]')).filter((entry) => (entry as HTMLElement).tabIndex === 0);
    expect(stops.map((entry) => entry.id)).toEqual(['destination-base-mainnet']);
  });

  it('does nothing when a disabled entry is clicked, pressed or focused', () => {
    render(oneEnabled());
    const entry = option('ethereum-mainnet');
    entry.click();
    entry.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    entry.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true }));
    entry.dispatchEvent(new KeyboardEvent('keydown', { key: ' ', bubbles: true, cancelable: true }));
    flushSync();
    expect(onSelect).not.toHaveBeenCalled();
    expect(option('base-mainnet').getAttribute('aria-checked')).toBe('true');
    expect(entry.getAttribute('aria-checked')).toBe('false');

    entry.focus();
    expect(document.activeElement).not.toBe(entry);
  });

  it('selects an enabled entry on click', () => {
    render(twoEnabled());
    option('arbitrum-mainnet').click();
    expect(onSelect).toHaveBeenCalledTimes(1);
    expect(onSelect).toHaveBeenCalledWith('arbitrum-mainnet');
  });

  it('moves between enabled entries with the arrow keys, skipping disabled ones, and focuses the one it selects', () => {
    render(twoEnabled(), 'base-mainnet');
    key('ArrowRight');
    expect(onSelect).toHaveBeenLastCalledWith('arbitrum-mainnet'); // skipped Ethereum
    expect(document.activeElement).toBe(option('arbitrum-mainnet'));
    key('ArrowLeft');
    expect(onSelect).toHaveBeenLastCalledWith('arbitrum-mainnet'); // from Base (selectedId is a prop) back to the last enabled
    key('End');
    expect(onSelect).toHaveBeenLastCalledWith('arbitrum-mainnet');
    key('Home');
    expect(onSelect).toHaveBeenLastCalledWith('base-mainnet');
    expect(document.activeElement).toBe(option('base-mainnet'));
  });

  it('never selects or focuses a disabled entry from the keyboard, however many keys are pressed', () => {
    render(twoEnabled(), 'base-mainnet');
    const disabled = new Set(['ethereum-mainnet', 'robinhood-mainnet', 'hyperevm-mainnet', 'monad-mainnet']);
    for (const name of ['ArrowRight', 'ArrowDown', 'ArrowLeft', 'ArrowUp', 'Home', 'End', 'ArrowRight', 'ArrowRight', 'ArrowLeft']) {
      key(name);
      for (const call of onSelect.mock.calls) expect(disabled.has(call[0] as string), `${name} selected ${call[0]}`).toBe(false);
      const active = document.activeElement as HTMLElement | null;
      expect(active && disabled.has(active.id.replace('destination-', '')), `${name} focused ${active?.id}`).toBeFalsy();
    }
  });

  it('with one enabled entry the arrows stay on it', () => {
    render(oneEnabled());
    key('ArrowRight');
    key('ArrowLeft');
    expect(onSelect.mock.calls.map((call) => call[0])).toEqual(['base-mainnet', 'base-mainnet']);
    expect(document.activeElement).toBe(option('base-mainnet'));
  });

  it('ignores other keys', () => {
    render(oneEnabled());
    key('a');
    key('Tab');
    expect(onSelect).not.toHaveBeenCalled();
  });

  it('moves the tab stop to the first enabled entry when the selected one has been turned off', () => {
    const entries = oneEnabled();
    entries[0] = makeDestination({ status: 'disabled', reason: 'The Base contract is not bound yet.' });
    entries[2] = { ...entries[2], status: 'enabled', reason: null };
    render(entries, 'base-mainnet');
    expect(option('base-mainnet').getAttribute('aria-checked')).toBe('true');
    expect(option('base-mainnet').getAttribute('aria-disabled')).toBe('true');
    expect(option('base-mainnet').textContent).toContain('The Base contract is not bound yet.');
    const stops = Array.from(host.querySelectorAll('[role="radio"]')).filter((entry) => (entry as HTMLElement).tabIndex === 0);
    expect(stops.map((entry) => entry.id)).toEqual(['destination-arbitrum-mainnet']);
  });

  it('shows a testnet loudly and an unknown chain without a network badge', () => {
    render([makeDestination({ name: 'Base Sepolia', chainId: '84532', network: 'testnet' }), makeDestination({ id: 'x', name: 'Base', chainId: '', status: 'disabled', reason: 'r' })]);
    expect(option('base-mainnet').textContent).toContain('Testnet');
    expect(option('base-mainnet').querySelector('.net.testnet')).toBeTruthy();
    expect(option('x').textContent).not.toMatch(/Mainnet|Testnet|Chain/);
  });
});
