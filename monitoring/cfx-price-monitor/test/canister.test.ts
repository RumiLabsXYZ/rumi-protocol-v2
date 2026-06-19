import { describe, it, expect, vi } from "vitest";
import { Principal } from "@dfinity/principal";
import { createCanisterClient, stringifyProtocolError, type RawBackend } from "../src/canister.js";

function fakeRaw(over: Partial<RawBackend>): RawBackend {
  return {
    get_manual_collateral_price: vi.fn().mockResolvedValue([]),
    set_manual_collateral_price: vi.fn().mockResolvedValue({ Ok: null }),
    list_chain_vaults: vi.fn().mockResolvedValue([]),
    get_price_pusher_principal: vi.fn().mockResolvedValue([]),
    ...over,
  };
}

describe("stringifyProtocolError", () => {
  it("renders a text-bearing variant as 'Tag: text'", () => {
    expect(stringifyProtocolError({ ChainAdmin: "not authorized to set price" })).toBe(
      "ChainAdmin: not authorized to set price",
    );
  });
  it("renders a unit variant as just the tag", () => {
    expect(stringifyProtocolError({ CallerNotOwner: null })).toBe("CallerNotOwner");
  });
  it("never throws on odd input", () => {
    expect(typeof stringifyProtocolError(undefined)).toBe("string");
  });
});

describe("createCanisterClient", () => {
  it("getOnChainPrice maps an opt-some to OnChainPrice", async () => {
    const raw = fakeRaw({
      get_manual_collateral_price: vi.fn().mockResolvedValue([{ price_e8: 15_000_000n, set_at_ns: 42n }]),
    });
    const c = createCanisterClient(raw);
    await expect(c.getOnChainPrice(1030, "CFX")).resolves.toEqual({ priceE8: 15_000_000n, setAtNs: 42n });
    expect(raw.get_manual_collateral_price).toHaveBeenCalledWith(1030, "CFX");
  });

  it("getOnChainPrice maps an opt-none to null", async () => {
    const c = createCanisterClient(fakeRaw({}));
    await expect(c.getOnChainPrice(1030, "CFX")).resolves.toBeNull();
  });

  it("setPrice maps Ok to { ok: true }", async () => {
    const raw = fakeRaw({ set_manual_collateral_price: vi.fn().mockResolvedValue({ Ok: null }) });
    const c = createCanisterClient(raw);
    await expect(c.setPrice(1030, "CFX", 15_000_000n)).resolves.toEqual({ ok: true });
    expect(raw.set_manual_collateral_price).toHaveBeenCalledWith(1030, "CFX", 15_000_000n);
  });

  it("setPrice maps Err to { ok: false, error } with a readable message", async () => {
    const raw = fakeRaw({
      set_manual_collateral_price: vi.fn().mockResolvedValue({ Err: { ChainAdmin: "not authorized to set price" } }),
    });
    const c = createCanisterClient(raw);
    const r = await c.setPrice(1030, "CFX", 15_000_000n);
    expect(r.ok).toBe(false);
    expect(r.error).toContain("not authorized");
  });

  it("listChainVaults projects the three fields we need", async () => {
    const raw = fakeRaw({
      list_chain_vaults: vi.fn().mockResolvedValue([
        { vault_id: 1n, collateral_amount_e18: 10n ** 18n, debt_e8s: 15_000_000n, owner: "x", status: { Open: null } },
      ]),
    });
    const c = createCanisterClient(raw);
    await expect(c.listChainVaults(1030)).resolves.toEqual([
      { vaultId: 1n, collateralAmountE18: 10n ** 18n, debtE8s: 15_000_000n },
    ]);
  });

  it("getPricePusher maps opt principal", async () => {
    const p = Principal.fromText("aaaaa-aa");
    const some = createCanisterClient(fakeRaw({ get_price_pusher_principal: vi.fn().mockResolvedValue([p]) }));
    await expect(some.getPricePusher()).resolves.toBe(p);
    const none = createCanisterClient(fakeRaw({}));
    await expect(none.getPricePusher()).resolves.toBeNull();
  });
});
