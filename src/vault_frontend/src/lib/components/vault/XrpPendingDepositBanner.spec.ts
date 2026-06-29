import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const componentPath = resolve(__dirname, 'XrpPendingDepositBanner.svelte');
const layoutPath = resolve(__dirname, '../../../routes/+layout.svelte');
const stripPath = resolve(__dirname, '../layout/PositionStrip.svelte');

describe('XrpPendingDepositBanner layout contract', () => {
  it('reserves fixed header space so the position strip cannot cover the recovery banner', () => {
    const banner = readFileSync(componentPath, 'utf8');
    const layout = readFileSync(layoutPath, 'utf8');
    const strip = readFileSync(stripPath, 'utf8');

    expect(banner).toContain("document.documentElement.style.setProperty('--rumi-xrp-recovery-height'");
    expect(banner).toContain('class="xrp-recovery-slot" bind:clientHeight={recoveryHeight}');
    expect(banner).toContain('top: 3.5rem');
    expect(strip).toContain('top: calc(3.5rem + var(--rumi-xrp-recovery-height, 0px))');
    expect(layout).toContain('var(--rumi-xrp-recovery-height, 0px) + var(--rumi-strip-height, 0px)');
  });
});
