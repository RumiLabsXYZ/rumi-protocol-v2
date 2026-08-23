import { describe, expect, it, vi } from "vitest";
import type { Backend, ChainVaultPage } from "./backend";
import { listCompleteChainVaultInventory } from "./inventory";

const vault = (id: bigint, chain = 1030) => ({ vault_id: id, collateral_chain: chain }) as any;
const backend = (pages: ChainVaultPage[]) => ({
  list_chain_vaults_page: vi.fn(async () => {
    const page = pages.shift();
    if (!page) throw new Error("unexpected page request");
    return page;
  }),
}) as unknown as Pick<Backend, "list_chain_vaults_page">;

describe("complete bounded chain-vault inventory", () => {
  it("exhausts cursor pages sequentially and returns the complete inventory", async () => {
    const be = backend([
      { done: false, vaults: [vault(1n)], scanned_count: 500, next_start_after: [500n] },
      { done: true, vaults: [vault(700n)], scanned_count: 200, next_start_after: [] },
    ]);
    await expect(listCompleteChainVaultInventory(be, 1030)).resolves.toMatchObject([
      { vault_id: 1n }, { vault_id: 700n },
    ]);
    expect(be.list_chain_vaults_page).toHaveBeenNthCalledWith(1, 1030, [], 500);
    expect(be.list_chain_vaults_page).toHaveBeenNthCalledWith(2, 1030, [500n], 500);
  });

  it("fails closed instead of returning partial inventory", async () => {
    await expect(listCompleteChainVaultInventory(backend([
      { done: false, vaults: [vault(1n)], scanned_count: 500, next_start_after: [] },
    ]), 1030)).rejects.toThrow("did not advance");

    await expect(listCompleteChainVaultInventory(backend([
      { done: false, vaults: [vault(1n)], scanned_count: 500, next_start_after: [500n] },
    ]), 1030, 1)).rejects.toThrow("safety ceiling");

    await expect(listCompleteChainVaultInventory(backend([
      { done: true, vaults: [vault(1n), vault(1n)], scanned_count: 2, next_start_after: [] },
    ]), 1030)).rejects.toThrow("repeated a vault");

    await expect(listCompleteChainVaultInventory(backend([
      { done: true, vaults: [vault(1n, 71)], scanned_count: 1, next_start_after: [] },
    ]), 1030)).rejects.toThrow("wrong chain");
  });
});
