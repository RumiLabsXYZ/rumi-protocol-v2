<script lang="ts">
  import EntityLink from './EntityLink.svelte';
  import VaultHealthBar from './VaultHealthBar.svelte';
  import { resolveCollateralSymbol } from '$lib/utils/eventFormatters';

  interface AtRiskVault {
    vault_id: number;
    owner: string;
    collateral_ratio: number;
    collateral_type: any;
    liquidation_ratio: number;
    borrowed_icusd_amount: number;
    collateral_amount: number;
    collateral_decimals: number;
  }

  interface Props {
    vaults: AtRiskVault[];
    totalAtRisk: number;
    loading?: boolean;
  }

  let { vaults, totalAtRisk, loading = false }: Props = $props();

  const E8S = 100_000_000;
</script>

<div class="bg-gray-800/50 border border-gray-700/50 rounded-xl overflow-hidden">
  <div class="px-5 py-4 border-b border-gray-700/50">
    <div class="flex items-center justify-between">
      <h3 class="text-sm font-semibold text-gray-200">Liquidation Risk</h3>
      {#if totalAtRisk > 0}
        <span class="text-xs font-medium px-2 py-0.5 rounded-full bg-red-400/10 text-red-400 border border-red-400/30">
          {totalAtRisk} at risk
        </span>
      {:else}
        <span class="text-xs font-medium px-2 py-0.5 rounded-full bg-green-400/10 text-green-400 border border-green-400/30">
          All healthy
        </span>
      {/if}
    </div>
  </div>

  {#if loading}
    <div class="px-5 py-8 text-center text-gray-500 text-sm">Loading...</div>
  {:else if vaults.length === 0}
    <div class="px-5 py-8 text-center text-gray-500 text-sm">No vaults near liquidation</div>
  {:else}
    <div class="overflow-x-auto">
      <table class="w-full text-sm">
        <thead>
          <tr class="border-b border-gray-700/50">
            <th class="px-4 py-2.5 text-xs font-medium text-gray-400 uppercase tracking-wider text-left">Vault</th>
            <th class="px-4 py-2.5 text-xs font-medium text-gray-400 uppercase tracking-wider text-left">Owner</th>
            <th class="px-4 py-2.5 text-xs font-medium text-gray-400 uppercase tracking-wider text-left">Collateral</th>
            <th class="px-4 py-2.5 text-xs font-medium text-gray-400 uppercase tracking-wider text-right">Debt</th>
            <th class="px-4 py-2.5 text-xs font-medium text-gray-400 uppercase tracking-wider text-left w-48">Health</th>
          </tr>
        </thead>
        <tbody>
          {#each vaults as vault}
            <tr class="border-b border-gray-700/30 hover:bg-gray-700/20 transition-colors">
              <td class="px-4 py-2.5">
                <EntityLink type="vault" id={vault.vault_id} />
              </td>
              <td class="px-4 py-2.5">
                <EntityLink type="address" id={vault.owner} />
              </td>
              <td class="px-4 py-2.5 text-gray-300 text-xs">
                {resolveCollateralSymbol(vault.collateral_type)}
              </td>
              <td class="px-4 py-2.5 text-right text-gray-200 tabular-nums text-xs">
                {(vault.borrowed_icusd_amount / E8S).toLocaleString(undefined, { maximumFractionDigits: 2 })} icUSD
              </td>
              <td class="px-4 py-2.5">
                <VaultHealthBar collateralRatio={vault.collateral_ratio} liquidationRatio={vault.liquidation_ratio} />
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</div>
