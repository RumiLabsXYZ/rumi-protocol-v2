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
