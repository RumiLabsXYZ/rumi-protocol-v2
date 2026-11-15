import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

describe('production Internet Identity alternative origins', () => {
  it('contains only HTTPS non-local origins', () => {
    const path = resolve(process.cwd(), 'static/.well-known/ii-alternative-origins');
    const parsed = JSON.parse(readFileSync(path, 'utf8')) as { alternativeOrigins: string[] };

    expect(parsed.alternativeOrigins.length).toBeGreaterThan(0);
    for (const origin of parsed.alternativeOrigins) {
      const url = new URL(origin);
      expect(url.protocol).toBe('https:');
      expect(url.hostname).not.toBe('localhost');
      expect(url.hostname).not.toBe('127.0.0.1');
      expect(url.hostname.endsWith('.localhost')).toBe(false);
    }
  });
});

describe('production frontend content security policy', () => {
  it('keeps development localhost out and limits asset wildcards', () => {
    const path = resolve(process.cwd(), 'static/.ic-assets.json');
    const assets = JSON.parse(readFileSync(path, 'utf8')) as Array<{
      match: string;
      headers?: Record<string, string>;
    }>;
    const policy = assets.find((asset) => asset.match === '**/*')?.headers?.['Content-Security-Policy'];

    expect(policy).toBeDefined();
    const directives = Object.fromEntries(
      policy!.split(';').filter(Boolean).map((directive) => {
        const [name, ...sources] = directive.trim().split(/\s+/);
        return [name, sources];
      })
    );

    expect(directives['connect-src']).not.toContain('http://localhost:*');
    expect(directives['connect-src']).toEqual(
      expect.arrayContaining([
        'https://icp0.io',
        'https://*.icp0.io',
        'https://icp-api.io',
        'https://ic0.app',
        'https://*.ic0.app',
        'https://oisy.com',
        'https://beta.oisy.com',
        'https://xumm.app',
        'wss://xumm.app',
        'https://xaman.app',
        'https://s.altnet.rippletest.net:51234',
        'https://xrplcluster.com',
      ])
    );
    // The production HTML has a SvelteKit inline hydration script; generated bundles do not use eval().
    expect(directives['script-src']).toContain("'unsafe-inline'");
    expect(directives['script-src']).not.toContain("'unsafe-eval'");
    expect(directives['img-src']).toEqual(
      expect.arrayContaining(["'self'", 'blob:', 'data:', 'https://xumm.app', 'https://*.xumm.app'])
    );
    expect(directives['style-src']).toEqual(
      expect.arrayContaining(["'self'", "'unsafe-inline'", 'https://fonts.googleapis.com'])
    );
    expect(directives['style-src-elem']).toEqual(expect.arrayContaining(["'self'", 'https://fonts.googleapis.com']));
    expect(directives['style-src-attr']).toEqual(["'unsafe-inline'"]);
    expect(directives['font-src']).toEqual(
      expect.arrayContaining(["'self'", 'data:', 'https://fonts.gstatic.com'])
    );
    for (const name of ['img-src', 'style-src', 'style-src-elem', 'font-src']) {
      expect(directives[name]).not.toContain('*');
    }
  });
});
