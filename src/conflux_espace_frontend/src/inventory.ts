import type { Backend, ChainVault, ChainVaultPage } from "./backend";

export const CHAIN_VAULT_PAGE_SIZE = 500;
export const CHAIN_VAULT_MAX_CLIENT_PAGES = 200;

type PageBackend = Pick<Backend, "list_chain_vaults_page">;

/** Exhaust the backend's bounded cursor query. Never return a partial inventory. */
export async function listCompleteChainVaultInventory(
  be: PageBackend,
  chainId: number,
  maxPages = CHAIN_VAULT_MAX_CLIENT_PAGES,
): Promise<ChainVault[]> {
  if (!Number.isInteger(maxPages) || maxPages < 1) {
    throw new Error("The chain-vault inventory page ceiling is invalid.");
  }

  const vaults: ChainVault[] = [];
  const seenVaultIds = new Set<string>();
  let cursor: [] | [bigint] = [];

  for (let pageNumber = 0; pageNumber < maxPages; pageNumber++) {
    const page: ChainVaultPage = await be.list_chain_vaults_page(chainId, cursor, CHAIN_VAULT_PAGE_SIZE);
    if (!Number.isInteger(page.scanned_count) || page.scanned_count < 0 ||
        page.scanned_count > CHAIN_VAULT_PAGE_SIZE) {
      throw new Error("The backend returned an invalid chain-vault page size. Inventory is incomplete; writes remain paused.");
    }

    for (const vault of page.vaults) {
      if (vault.collateral_chain !== chainId) {
        throw new Error("The backend returned a vault for the wrong chain. Inventory is incomplete; writes remain paused.");
      }
      const id = vault.vault_id.toString();
      if (seenVaultIds.has(id)) {
        throw new Error("The backend repeated a vault across cursor pages. Inventory is incomplete; writes remain paused.");
      }
      seenVaultIds.add(id);
      vaults.push(vault);
    }

    if (page.done) {
      if (page.next_start_after.length) {
        throw new Error("The backend returned a terminal page with another cursor. Inventory is incomplete; writes remain paused.");
      }
      return vaults;
    }

    const next: bigint | undefined = page.next_start_after[0];
    const previous: bigint | undefined = cursor[0];
    if (next === undefined || (previous !== undefined && next <= previous)) {
      throw new Error("The backend chain-vault cursor did not advance. Inventory is incomplete; writes remain paused.");
    }
    cursor = [next];
  }

  throw new Error(
    `Chain-vault inventory exceeded the ${maxPages}-page client safety ceiling. ` +
    "No partial inventory was used; writes remain paused.",
  );
}
