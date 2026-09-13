import { beforeEach, describe, expect, it, vi } from "vitest";
import { Principal } from "@dfinity/principal";

const mocks = vi.hoisted(() => ({
  be: { get_protocol_status: vi.fn(), get_collateral_totals: vi.fn(), get_reserve_balances: vi.fn(), get_collateral_config: vi.fn() },
  createActor: vi.fn(),
}));
vi.mock("./backend", () => ({ backend: async () => mocks.be, queryAgent: async () => ({}) }));
vi.mock("@dfinity/agent", () => ({ Actor: { createActor: mocks.createActor } }));
import { fetchProtocolOverview } from "./protocolOverview";

const ledger = Principal.fromText("aaaaa-aa");
const value = (result: Awaited<ReturnType<typeof fetchProtocolOverview>>, label: string) => result.rows.find(row => row.label === label)?.value;

beforeEach(() => {
  vi.resetAllMocks();
  mocks.be.get_protocol_status.mockResolvedValue({ mode: { GeneralAvailability: null }, frozen: false, total_collateral_ratio: 1.8, total_icusd_borrowed: 100_000_000n });
  mocks.be.get_collateral_totals.mockResolvedValue([{ collateral_type: ledger, symbol: "ICP", total_collateral: 200_000_000n, decimals: 8, price: 3 }]);
  mocks.be.get_reserve_balances.mockResolvedValue([{ ledger, balance: 0n }]);
  mocks.createActor.mockReturnValue({ icrc1_balance_of: async () => 12_000_000n, icrc1_decimals: async () => 6 });
});

describe("public protocol overview", () => {
  it("queries actual reserves rather than displaying the backend placeholder zero", async () => {
    const result = await fetchProtocolOverview();
    expect(value(result, "Stablecoin reserves")).toBe("≈ $12.00");
    expect(value(result, "Total collateral value")).toBe("$6.00");
    expect(value(result, "Protocol collateral ratio")).toBe("180.00%");
    expect(result.composition?.[0].label).toBe("ICP ecosystem");
    expect(result.note).toContain("Conflux vaults are tracked separately");
    expect(result.note).toContain("$1 per token");
  });
  it("does not claim zero reserves when a ledger is unavailable", async () => {
    mocks.createActor.mockReturnValue({ icrc1_balance_of: async () => { throw Error("offline"); }, icrc1_decimals: async () => 6 });
    expect(value(await fetchProtocolOverview(), "Stablecoin reserves")).toBe("Unavailable");
  });
  it("does not silently omit unpriced collateral", async () => {
    mocks.be.get_collateral_totals.mockResolvedValue([{ collateral_type: ledger, symbol: "ckBTC", total_collateral: 10n, decimals: 8, price: 0 }]);
    const result = await fetchProtocolOverview();
    expect(value(result, "Total collateral value")).toBe("Unavailable");
    expect(result.composition?.[0]).toMatchObject({ label: "BTC", value: "Unavailable" });
  });
  it("does not label an invalid ratio as no debt", async () => {
    mocks.be.get_protocol_status.mockResolvedValue({ mode: { ReadOnly: null }, frozen: false, total_collateral_ratio: NaN, total_icusd_borrowed: 100n });
    const result = await fetchProtocolOverview();
    expect(value(result, "Protocol collateral ratio")).toBe("Unavailable");
    expect(value(result, "System mode")).toBe("Read-only");
  });
  it("shows frozen and no-debt states explicitly", async () => {
    mocks.be.get_protocol_status.mockResolvedValue({ mode: { GeneralAvailability: null }, frozen: true, total_collateral_ratio: NaN, total_icusd_borrowed: 0n });
    const result = await fetchProtocolOverview();
    expect(value(result, "Protocol collateral ratio")).toBe("No debt");
    expect(value(result, "System mode")).toBe("Frozen");
  });
  it("returns an explicit unavailable snapshot on query failure", async () => {
    mocks.be.get_protocol_status.mockRejectedValue(Error("offline"));
    const result = await fetchProtocolOverview();
    expect(result.composition).toBeNull();
    expect(result.rows.every(row => row.value === "Unavailable")).toBe(true);
    expect(result.note).toContain("Refresh to try again");
  });
});
