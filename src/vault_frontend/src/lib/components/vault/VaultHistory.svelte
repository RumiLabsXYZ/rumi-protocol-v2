<script lang="ts">
  import { onMount } from 'svelte';
  import { protocolService } from '$lib/services/protocol';
  import { formatNumber } from '$lib/utils/format';

  export let vaultId: number;

  const E8S = 100_000_000;

  interface FormattedEvent {
    type: string;
    icon: string;
    color: string;
    details: string;
    index: number;
  }

  let vaultEvents: FormattedEvent[] = [];
  let isLoading = true;
  let error = '';

  function formatEvent(event: any, idx: number): FormattedEvent {
    let type = 'Unknown Event';
    let icon = '•';
    let color = 'text-gray-400';
    let details = '';

    if ('open_vault' in event) {
      type = 'Vault Opened';
      icon = '🔓';
      color = 'text-green-400';
      const vault = event.open_vault.vault;
      const margin = Number(vault.icp_margin_amount) / E8S;
      details = `Created with ${formatNumber(margin)} ICP collateral`;
    }
    else if ('add_margin_to_vault' in event) {
      type = 'Collateral Added';
      icon = '➕';
      color = 'text-blue-400';
      details = `Added ${formatNumber(Number(event.add_margin_to_vault.margin_added) / E8S)} ICP`;
    }
    else if ('borrow_from_vault' in event) {
      type = 'Borrowed';
      icon = '💵';
      color = 'text-yellow-400';
      const borrowed = Number(event.borrow_from_vault.borrowed_amount) / E8S;
      const fee = Number(event.borrow_from_vault.fee_amount) / E8S;
      details = `Borrowed ${formatNumber(borrowed)} icUSD (fee: ${formatNumber(fee)})`;
    }
    else if ('repay_to_vault' in event) {
      type = 'Repaid';
      icon = '✅';
      color = 'text-green-400';
      details = `Repaid ${formatNumber(Number(event.repay_to_vault.repayed_amount) / E8S)} icUSD`;
    }
    else if ('collateral_withdrawn' in event) {
      type = 'Collateral Withdrawn';
      icon = '📤';
      color = 'text-purple-400';
      details = `Withdrew ${formatNumber(Number(event.collateral_withdrawn.amount) / E8S)} ICP`;
    }
    else if ('partial_collateral_withdrawn' in event) {
      type = 'Partial Withdrawal';
      icon = '📤';
      color = 'text-purple-400';
      details = `Withdrew ${formatNumber(Number(event.partial_collateral_withdrawn.amount) / E8S)} ICP`;
    }
    else if ('close_vault' in event) {
      type = 'Vault Closed';
      icon = '🔒';
      color = 'text-gray-400';
      details = 'Vault closed and collateral returned';
    }
    else if ('withdraw_and_close_vault' in event) {
      type = 'Withdrawn & Closed';
      icon = '🔒';
      color = 'text-gray-400';
      const amt = Number(event.withdraw_and_close_vault.amount) / E8S;
      details = `Withdrew ${formatNumber(amt)} ICP and closed vault`;
    }
    else if ('liquidate_vault' in event) {
      type = 'Liquidated';
      icon = '⚠️';
      color = 'text-red-400';
      const e = event.liquidate_vault;
      const mode = e.mode ? (typeof e.mode === 'object' ? Object.keys(e.mode)[0] : String(e.mode)) : 'unknown';
      details = `Liquidated (${mode} mode)`;
    }
    else if ('partial_liquidate_vault' in event) {
      type = 'Partial Liquidation';
      icon = '⚠️';
      color = 'text-orange-400';
      const e = event.partial_liquidate_vault;
      const payment = Number(e.liquidator_payment) / E8S;
      details = `Partial liquidation: ${formatNumber(payment)} icUSD`;
    }
    else if ('redistribute_vault' in event) {
      type = 'Redistributed';
      icon = '🔄';
      color = 'text-red-400';
      details = 'Vault debt redistributed to other vaults';
    }
    else if ('margin_transfer' in event) {
      type = 'Margin Transfer';
      icon = '↔️';
      color = 'text-blue-300';
      details = 'Collateral transfer processed';
    }
    else if ('dust_forgiven' in event) {
      type = 'Dust Forgiven';
      icon = '🧹';
      color = 'text-gray-400';
      const amt = Number(event.dust_forgiven.amount) / E8S;
      details = `${formatNumber(amt)} icUSD dust forgiven`;
    }
    else if ('redemption_on_vaults' in event) {
      type = 'Redeemed';
      icon = '🔁';
      color = 'text-cyan-400';
      const amt = Number(event.redemption_on_vaults.icusd_amount) / E8S;
      const fee = Number(event.redemption_on_vaults.fee_amount) / E8S;
      details = `${formatNumber(amt)} icUSD redeemed (fee: ${formatNumber(fee)})`;
    }

    return { type, icon, color, details, index: idx };
  }

  async function loadVaultHistory() {
    isLoading = true;
    error = '';
    try {
      const events = await protocolService.getVaultHistory(vaultId);
      // Events come from the backend in chronological order; show newest first
      vaultEvents = events.map(formatEvent).reverse();
    } catch (err) {
      console.error('Error loading vault history:', err);
      error = 'Failed to load vault history';
    } finally {
      isLoading = false;
    }
  }

  onMount(loadVaultHistory);
</script>

<div class="bg-gray-800/50 backdrop-blur-sm border border-gray-700 rounded-lg p-5">
  <div class="flex justify-between items-center mb-4">
    <h3 class="text-lg font-semibold">Vault History</h3>
    <button
      on:click={loadVaultHistory}
      disabled={isLoading}
      class="text-xs text-gray-400 hover:text-white transition-colors"
    >
      {isLoading ? 'Loading...' : 'Refresh'}
    </button>
  </div>

  {#if isLoading}
    <div class="flex justify-center py-6">
      <div class="w-5 h-5 border-2 border-purple-500 border-t-transparent rounded-full animate-spin"></div>
    </div>
  {:else if error}
    <div class="p-3 bg-red-900/30 border border-red-800 rounded-lg text-red-200 text-sm">
      {error}
    </div>
  {:else if vaultEvents.length === 0}
    <div class="py-6 text-center text-gray-500 text-sm">
      No events recorded yet
    </div>
  {:else}
    <div class="space-y-2">
      {#each vaultEvents as event, i}
        <div class="flex items-start gap-3 py-2 {i < vaultEvents.length - 1 ? 'border-b border-gray-800' : ''}">
          <span class="text-base flex-shrink-0 mt-0.5">{event.icon}</span>
          <div class="min-w-0 flex-1">
            <div class="flex items-baseline justify-between gap-2">
              <span class="font-medium text-sm {event.color}">{event.type}</span>
              <span class="text-xs text-gray-500 flex-shrink-0">#{event.index + 1}</span>
            </div>
            <p class="text-xs text-gray-400 mt-0.5">{event.details}</p>
          </div>
        </div>
      {/each}
    </div>
  {/if}
</div>
