import type { Principal } from '@dfinity/principal';

/** Frontend visibility hint; the canister enforces the same list independently. */
export const SENTINEL_TELEMETRY_VIEWERS = [
	'zegjz-jpi6k-qkand-c2bgf-qw6za-xk4si-nz3gx-qzzia-fk6fg-snepb-tae',
	'stzp3-bnvwm-zqzjh-o6mv6-ci53m-wj5k6-xyhe7-fnyp2-c64o3-7vokj-bqe',
	'4alqm-afk6k-bybok-qvdyo-cnv7y-klel6-xm2pz-7h7jk-utmys-kttf3-vqe',
] as const;

const SENTINEL_TELEMETRY_VIEWER_SET: ReadonlySet<string> = new Set(SENTINEL_TELEMETRY_VIEWERS);

export function canViewSentinelTelemetry(principal: Principal | null | undefined): boolean {
	return principal !== null && principal !== undefined && SENTINEL_TELEMETRY_VIEWER_SET.has(principal.toText());
}
