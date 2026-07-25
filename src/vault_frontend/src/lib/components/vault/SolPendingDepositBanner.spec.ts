import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const componentPath = resolve(__dirname, 'SolPendingDepositBanner.svelte');
const xrpComponentPath = resolve(__dirname, 'XrpPendingDepositBanner.svelte');
const layoutPath = resolve(__dirname, '../../../routes/+layout.svelte');
const stripPath = resolve(__dirname, '../layout/PositionStrip.svelte');

describe('SolPendingDepositBanner layout contract', () => {
  it('owns its own CSS var, distinct from the XRP recovery-height var', () => {
    const banner = readFileSync(componentPath, 'utf8');

    expect(banner).toContain("document.documentElement.style.setProperty('--rumi-sol-recovery-height'");
    expect(banner).not.toContain("setProperty('--rumi-xrp-recovery-height'");
    expect(banner).toContain('class="sol-recovery-slot" bind:clientHeight={recoveryHeight}');
  });

  it('stacks below the XRP recovery banner instead of overlapping it', () => {
    const banner = readFileSync(componentPath, 'utf8');

    // Own top offset sums the XRP banner's height, so when both are present the
    // SOL banner renders in the next row down rather than on top of the XRP one.
    expect(banner).toContain('top: calc(3.5rem + var(--rumi-xrp-recovery-height, 0px))');
  });

  it('sums BOTH recovery-height vars in the position strip and page padding, never replacing one with the other', () => {
    const layout = readFileSync(layoutPath, 'utf8');
    const strip = readFileSync(stripPath, 'utf8');

    // A wrong sum (e.g. only one var present, or one replacing the other) would
    // silently let the position strip or page content overlap a visible banner.
    expect(strip).toContain(
      'top: calc(3.5rem + var(--rumi-xrp-recovery-height, 0px) + var(--rumi-sol-recovery-height, 0px))'
    );
    expect(layout).toContain(
      'calc(4.75rem + var(--rumi-xrp-recovery-height, 0px) + var(--rumi-sol-recovery-height, 0px) + var(--rumi-strip-height, 0px))'
    );
    expect(layout).toContain(
      'calc(4.25rem + var(--rumi-xrp-recovery-height, 0px) + var(--rumi-sol-recovery-height, 0px) + var(--rumi-strip-height, 0px))'
    );

    // Both source vars must actually appear (guards against one being dropped
    // during a future edit that only "fixes" the desktop or only the mobile rule).
    for (const css of [layout, strip]) {
      expect(css).toContain('--rumi-xrp-recovery-height');
      expect(css).toContain('--rumi-sol-recovery-height');
    }
  });

  it('is mounted in the root layout alongside the XRP banner', () => {
    const layout = readFileSync(layoutPath, 'utf8');

    expect(layout).toContain('import SolPendingDepositBanner');
    expect(layout).toContain('<SolPendingDepositBanner />');
    expect(layout).toContain('<XrpPendingDepositBanner />');
    // SOL banner is mounted after XRP in DOM order, matching the visual stack.
    expect(layout.indexOf('<XrpPendingDepositBanner />')).toBeLessThan(
      layout.indexOf('<SolPendingDepositBanner />')
    );
  });

  it('cleans up its CSS var on destroy, mirroring the XRP banner', () => {
    const banner = readFileSync(componentPath, 'utf8');
    const xrpBanner = readFileSync(xrpComponentPath, 'utf8');

    expect(banner).toContain("document.documentElement.style.setProperty('--rumi-sol-recovery-height', '0px')");
    expect(xrpBanner).toContain("document.documentElement.style.setProperty('--rumi-xrp-recovery-height', '0px')");
  });
});
