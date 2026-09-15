import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

describe('public telemetry page contract', () => {
	const source = readFileSync(fileURLToPath(new URL('./+page.svelte', import.meta.url)), 'utf8');
	it('has all read-only states and no mutation controls', () => {
		expect(source).toContain('Cycle Sentinel is not configured');
		expect(source).toContain('Loading live cycle telemetry');
		expect(source).toContain('Telemetry is unavailable');
		expect(source).toContain('Showing the last successful snapshot');
		expect(source).toContain('No monitored canisters are configured yet');
		expect(source).toContain('Open operator console');
		expect(source).toContain('row.recent_topups.slice(0, 3)');
		expect(source).not.toMatch(/manual_top_up|approve_proposal|execute_proposal|pause_target/);
	});
});
