/**
 * Local-replica target for the dev-only "local" mode (CONFIG.isLocal, i.e. the app is
 * served from localhost / 127.0.0.1).
 *
 * Defaults are the dfx replica on port 4943 and its Internet Identity canister, so with
 * no environment variables set every value here is byte-identical to the previously
 * hard-coded strings. Two optional Vite variables retarget a different local gateway,
 * e.g. a PocketIC HTTP gateway on 4950 for the XRPL testnet rehearsal:
 *
 *   VITE_LOCAL_IC_HOST            full gateway origin, default http://localhost:4943
 *   VITE_LOCAL_II_CANISTER_ID     local Internet Identity canister id,
 *                                 default uxrrr-q7777-77774-qaaaq-cai
 *
 * Neither variable has any effect on production: they are read only on the local-mode
 * code paths, and an unset or blank value falls back to the default.
 */

export const DEFAULT_LOCAL_IC_HOST = 'http://localhost:4943';
export const DEFAULT_LOCAL_II_CANISTER_ID = 'uxrrr-q7777-77774-qaaaq-cai';

type EnvLike = Record<string, unknown> | undefined;

function nonBlank(value: unknown): string | undefined {
  if (typeof value !== 'string') return undefined;
  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : undefined;
}

/** Gateway origin without a trailing slash. */
export function resolveLocalIcHost(env: EnvLike = import.meta.env): string {
  const raw = nonBlank(env?.VITE_LOCAL_IC_HOST);
  return raw ? raw.replace(/\/+$/, '') : DEFAULT_LOCAL_IC_HOST;
}

export function resolveLocalIiCanisterId(env: EnvLike = import.meta.env): string {
  return nonBlank(env?.VITE_LOCAL_II_CANISTER_ID) ?? DEFAULT_LOCAL_II_CANISTER_ID;
}

/** Subdomain URL of the local II canister: `<id>.<gateway host>[:port]`. */
export function localIiUrl(host: string, canisterId: string): string {
  const u = new URL(host);
  return `${u.protocol}//${canisterId}.${u.host}`;
}

/** Numeric port of the gateway (4943 for the default host). */
export function localReplicaPort(host: string): number {
  const u = new URL(host);
  if (u.port) return Number(u.port);
  return u.protocol === 'https:' ? 443 : 80;
}

export const LOCAL_IC_HOST = resolveLocalIcHost();
export const LOCAL_II_CANISTER_ID = resolveLocalIiCanisterId();
export const LOCAL_II_URL = localIiUrl(LOCAL_IC_HOST, LOCAL_II_CANISTER_ID);
export const LOCAL_REPLICA_PORT = localReplicaPort(LOCAL_IC_HOST);
/** True only when the dev override moved the local II off the dfx default. */
export const LOCAL_II_OVERRIDDEN = LOCAL_II_URL !== localIiUrl(DEFAULT_LOCAL_IC_HOST, DEFAULT_LOCAL_II_CANISTER_ID);
