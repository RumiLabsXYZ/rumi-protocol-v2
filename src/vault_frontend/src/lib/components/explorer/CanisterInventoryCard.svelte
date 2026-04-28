<script lang="ts">
  import { CANISTER_IDS, vault_frontend } from '$lib/config';
  import EntityLink from './EntityLink.svelte';
  import CopyButton from './CopyButton.svelte';

  // Order: protocol core first, then DeFi, then ledgers, then frontends.
  // Liquidation bot principal isn't in $lib/config; sourced from dfx.json.
  type Row = { label: string; principal: string; role: string };
  const LIQUIDATION_BOT = 'nygob-3qaaa-aaaap-qttcq-cai';

  const rows: Row[] = [
    { label: 'rumi_protocol_backend', principal: CANISTER_IDS.PROTOCOL, role: 'Core CDP engine' },
    { label: 'rumi_treasury', principal: CANISTER_IDS.TREASURY, role: 'Treasury' },
    { label: 'rumi_stability_pool', principal: CANISTER_IDS.STABILITY_POOL, role: 'Stability pool' },
    { label: 'rumi_3pool', principal: CANISTER_IDS.THREEPOOL, role: 'Stableswap (3USD)' },
    { label: 'rumi_amm', principal: CANISTER_IDS.RUMI_AMM, role: '3USD/ICP AMM' },
    { label: 'rumi_analytics', principal: CANISTER_IDS.ANALYTICS, role: 'Analytics tailer' },
    { label: 'liquidation_bot', principal: LIQUIDATION_BOT, role: 'Liquidations' },
    { label: 'icusd_ledger', principal: CANISTER_IDS.ICUSD_LEDGER, role: 'icUSD ICRC-1/2 ledger' },
    { label: 'vault_frontend', principal: vault_frontend, role: 'Explorer + dApp UI' },
  ].filter((r) => r.principal);

  function dashboardUrl(principal: string): string {
    return `https://dashboard.internetcomputer.org/canister/${principal}`;
  }
</script>

<div class="explorer-card">
  <h3 class="text-sm font-medium text-gray-300 mb-1">Canister inventory</h3>
  <p class="text-xs text-gray-500 mb-3">Every protocol canister with its principal ID. Dashboard link shows live cycles, controllers, and module hash.</p>
  <table class="w-full text-sm">
    <thead>
      <tr class="border-b border-white/5">
        <th class="text-left py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Canister</th>
        <th class="text-left py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Role</th>
        <th class="text-left py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Principal</th>
        <th class="text-right py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Dashboard</th>
      </tr>
    </thead>
    <tbody>
      {#each rows as r (r.principal)}
        <tr class="border-b border-white/[0.03] hover:bg-white/[0.02]">
          <td class="py-2 px-2 font-mono text-xs text-gray-200">{r.label}</td>
          <td class="py-2 px-2 text-gray-400">{r.role}</td>
          <td class="py-2 px-2 font-mono text-xs">
            <span class="inline-flex items-center gap-1">
              <EntityLink type="canister" value={r.principal} />
              <CopyButton text={r.principal} />
            </span>
          </td>
          <td class="py-2 px-2 text-right">
            <a href={dashboardUrl(r.principal)} target="_blank" rel="noopener" class="text-teal-400 hover:text-teal-300 text-xs">View ↗</a>
          </td>
        </tr>
      {/each}
    </tbody>
  </table>
</div>
