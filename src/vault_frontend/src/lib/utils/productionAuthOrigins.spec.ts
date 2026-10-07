import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const here = path.dirname(fileURLToPath(import.meta.url));

describe('production Internet Identity origins', () => {
  it('does not authorize localhost origins', () => {
    const origins = JSON.parse(readFileSync(
      path.resolve(here, '../../../static/.well-known/ii-alternative-origins'),
      'utf8',
    )).alternativeOrigins as string[];

    expect(origins.every(origin => !/^https?:\/\/localhost(?::|\/)/i.test(origin)
      && !/\.localhost(?::|\/)/i.test(origin))).toBe(true);
  });
});
