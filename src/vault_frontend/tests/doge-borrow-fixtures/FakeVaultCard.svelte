<script lang="ts">
  import { createEventDispatcher } from 'svelte';

  /**
   * Stand-in for $lib/components/vault/VaultCard.svelte in dogeBorrowPage.fixture.spec.ts.
   * The real VaultCard pulls in its own large web of stores/services (protocolService,
   * vaultStore, walletStore, collateralStore, ProtocolManager, tokenService, toastStore,
   * seasonStore...) that are out of scope for a /doge/borrow route fixture — this stub
   * exposes exactly the same prop/event contract (vault, icpPrice, expandedVaultId /
   * updated, toggle) so the route's data-filtering and wiring can be verified without
   * standing up VaultCard's entire dependency graph.
   */
  export let vault: any;
  export let icpPrice: number = 0;
  export let expandedVaultId: number | null = null;

  const dispatch = createEventDispatcher<{ updated: void; toggle: { vaultId: number } }>();
</script>

<div
  class="fake-vault-card"
  data-testid="fake-vault-card"
  data-vault-id={vault.vaultId}
  data-owner={vault.owner}
  data-collateral-type={vault.collateralType}
  data-collateral-amount={vault.collateralAmount}
  data-borrowed-icusd={vault.borrowedIcusd}
  data-icp-price={icpPrice}
  data-expanded={expandedVaultId === vault.vaultId}
>
  Fake VaultCard #{vault.vaultId}
  <button type="button" on:click={() => dispatch('toggle', { vaultId: vault.vaultId })}>toggle</button>
  <button type="button" on:click={() => dispatch('updated')}>updated</button>
</div>
