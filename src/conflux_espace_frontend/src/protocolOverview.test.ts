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
const noConfig = () => Promise.resolve([]);

beforeEach(() => {
  vi.resetAllMocks();
  mocks.be.get_protocol_status.mockResolvedValue({ mode: { GeneralAvailability: null }, frozen: false, total_collateral_ratio: 1.8, total_icusd_borrowed: 100_000_000n });
  mocks.be.get_collateral_totals.mockResolvedValue([{ collateral_type: ledger, symbol: "ICP", total_collateral: 200_000_000n, decimals: 8, price: 3 }]);
  mocks.be.get_reserve_balances.mockResolvedValue([{ ledger, balance: 0n }]);
  mocks.be.get_collateral_config.mockImplementation(noConfig);
  mocks.createActor.mockReturnValue({ icrc1_balance_of: async () => 12_000_000n, icrc1_decimals: async () => 6 });
});

describe("public protocol overview", () => {
  it("queries actual reserves rather than displaying the backend placeholder zero", async () => {
    const result = await fetchProtocolOverview();
    expect(value(result, "Stablecoin reserves")).toBe("≈ $12.00");
    expect(value(result, "Total collateral value")).toBe("$6.00");
    expect(value(result, "Protocol collateral ratio")).toBe("180.00%");
    expect(result.composition?.[0].label).toBe("ICP Ecosystem");
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
    expect(result.composition?.[0]).toMatchObject({ label: "BTC", value: "Unavailable", usd: null });
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

describe("collateral composition sorting and grouping", () => {
  const p = (id: string) => Principal.fromText(id);

  it("sorts groups by descending USD, keeps unknown-value groups last, and breaks ties by label", async () => {
    mocks.be.get_collateral_totals.mockResolvedValue([
      { collateral_type: p("aaaaa-aa"), symbol: "ckETH", total_collateral: 100_000_000n, decimals: 8, price: 20 },
      { collateral_type: p("2vxsx-fae"), symbol: "ckDOGE", total_collateral: 100_000_000n, decimals: 8, price: 200 },
      { collateral_type: p("rrkah-fqaaa-aaaaa-aaaaq-cai"), symbol: "ckXAUT", total_collateral: 100_000_000n, decimals: 8, price: 0 },
      { collateral_type: p("qoctq-giaaa-aaaaa-aaaea-cai"), symbol: "ICP", total_collateral: 100_000_000n, decimals: 8, price: 50 },
    ]);
    const result = await fetchProtocolOverview();
    const labels = result.composition?.map(g => g.label);
    expect(labels).toEqual(["DOGE", "ICP Ecosystem", "ETH", "XAUT"]);
    expect(result.composition?.[1].label).toBe("ICP Ecosystem");
    expect(result.composition?.[3]).toMatchObject({ label: "XAUT", usd: null });
  });

  it("groups ICP, nICP, BOB and EXE as one independently collapsible ICP Ecosystem entry sorted internally by USD", async () => {
    mocks.be.get_collateral_totals.mockResolvedValue([
      { collateral_type: p("aaaaa-aa"), symbol: "ICP", total_collateral: 2n, decimals: 0, price: 3 },
      { collateral_type: p("2vxsx-fae"), symbol: "nICP", total_collateral: 3n, decimals: 0, price: 4 },
      { collateral_type: p("rrkah-fqaaa-aaaaa-aaaaq-cai"), symbol: "BOB", total_collateral: 50n, decimals: 0, price: 1 },
      { collateral_type: p("qoctq-giaaa-aaaaa-aaaea-cai"), symbol: "EXE", total_collateral: 10n, decimals: 0, price: 0.5 },
    ]);
    const result = await fetchProtocolOverview();
    const ecosystem = result.composition?.find(g => g.label === "ICP Ecosystem");
    expect(ecosystem?.usd).toBe(6 + 12 + 50 + 5);
    expect(ecosystem?.members.map(m => m.symbol)).toEqual(["BOB", "nICP", "ICP", "EXE"]);
  });

  it("reuses the collateral configs actual display color, with neutral gray and native-XRP blue fallbacks", async () => {
    mocks.be.get_collateral_totals.mockResolvedValue([
      { collateral_type: p("aaaaa-aa"), symbol: "ckBTC", total_collateral: 100_000_000n, decimals: 8, price: 10 },
      { collateral_type: p("2vxsx-fae"), symbol: "", total_collateral: 1_000_000n, decimals: 6, price: 1 },
    ]);
    mocks.be.get_collateral_config.mockImplementation(async (id: Principal) => {
      if (id.toText() === "aaaaa-aa") return [{ custody_kind: [], display_color: ["#F7931A"] }];
      return [{ custody_kind: [{ NativeXrp: null }], display_color: [] }];
    });
    mocks.createActor.mockReturnValue({ icrc1_symbol: async () => "XRP" });
    const result = await fetchProtocolOverview();
    const btc = result.composition?.find(g => g.label === "BTC")?.members[0];
    const xrp = result.composition?.find(g => g.label === "XRP")?.members[0];
    expect(btc?.color).toBe("#F7931A");
    expect(xrp?.color).toBe("#4A90D9");
  });
});
