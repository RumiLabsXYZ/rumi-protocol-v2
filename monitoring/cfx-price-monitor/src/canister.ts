import { Actor, HttpAgent, type Identity } from "@dfinity/agent";
import { Principal } from "@dfinity/principal";
import { idlFactory } from "./idl.js";
import type { ChainVault, OnChainPrice } from "./types.js";

export interface SetPriceResult {
  ok: boolean;
  error?: string;
}

export interface CanisterClient {
  getOnChainPrice(chain: number, symbol: string): Promise<OnChainPrice | null>;
  setPrice(chain: number, symbol: string, priceE8: bigint): Promise<SetPriceResult>;
  listChainVaults(chain: number): Promise<ChainVault[]>;
  getPricePusher(): Promise<Principal | null>;
}

// Raw candid-decoded shapes from the actor (opt -> [] | [v], nat -> bigint).
interface ManualPriceInfoRaw {
  price_e8: bigint;
  set_at_ns: bigint;
}
interface ChainVaultRaw {
  vault_id: bigint;
  collateral_amount_e18: bigint;
  debt_e8s: bigint;
}
type ResultRaw = { Ok: null } | { Err: unknown };

/** The decoded actor surface for the four methods the monitor calls. */
export interface RawBackend {
  get_manual_collateral_price(chain: number, symbol: string): Promise<[] | [ManualPriceInfoRaw]>;
  set_manual_collateral_price(chain: number, symbol: string, priceE8: bigint): Promise<ResultRaw>;
  list_chain_vaults(chain: number): Promise<ChainVaultRaw[]>;
  get_price_pusher_principal(): Promise<[] | [Principal]>;
}

/** Render a decoded ProtocolError variant as a short string for logs/alerts. */
export function stringifyProtocolError(err: unknown): string {
  if (err === null || err === undefined || typeof err !== "object") return String(err);
  const entries = Object.entries(err as Record<string, unknown>);
  if (entries.length === 0) return "UnknownError";
  const [tag, val] = entries[0]!;
  if (val === null || val === undefined) return tag;
  if (typeof val === "string") return `${tag}: ${val}`;
  return `${tag}: ${JSON.stringify(val, (_k, v) => (typeof v === "bigint" ? v.toString() : v))}`;
}

/** Map the raw actor surface to the monitor's typed client. Pure given `raw`. */
export function createCanisterClient(raw: RawBackend): CanisterClient {
  return {
    async getOnChainPrice(chain, symbol) {
      const [info] = await raw.get_manual_collateral_price(chain, symbol);
      return info ? { priceE8: info.price_e8, setAtNs: info.set_at_ns } : null;
    },
    async setPrice(chain, symbol, priceE8) {
      const r = await raw.set_manual_collateral_price(chain, symbol, priceE8);
      return "Ok" in r ? { ok: true } : { ok: false, error: stringifyProtocolError(r.Err) };
    },
    async listChainVaults(chain) {
      const vaults = await raw.list_chain_vaults(chain);
      return vaults.map((v) => ({
        vaultId: v.vault_id,
        collateralAmountE18: v.collateral_amount_e18,
        debtE8s: v.debt_e8s,
      }));
    },
    async getPricePusher() {
      const [p] = await raw.get_price_pusher_principal();
      return p ?? null;
    },
  };
}

/** Build the real @dfinity actor and adapt it to `RawBackend`. */
export async function createRawBackend(opts: {
  canisterId: string;
  host: string;
  identity: Identity;
  /** Local replica only — fetch the root key. NEVER set against mainnet. */
  fetchRootKey?: boolean;
}): Promise<RawBackend> {
  const agent = await HttpAgent.create({ host: opts.host, identity: opts.identity });
  if (opts.fetchRootKey) await agent.fetchRootKey();
  return Actor.createActor(idlFactory, { agent, canisterId: opts.canisterId }) as unknown as RawBackend;
}
