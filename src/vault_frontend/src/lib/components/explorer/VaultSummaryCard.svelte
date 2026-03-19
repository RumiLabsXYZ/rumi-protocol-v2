<script lang="ts">
  import { truncatePrincipal } from '$lib/utils/principalHelpers';
  import { formatAmount } from '$lib/utils/eventFormatters';

  export let vault: any;
  export let collateralSymbol: string = 'tokens';
  export let collateralDecimals: number = 8;
  export let collateralPrice: number = 0;

  $: collateralValue = Number(vault.collateral_amount) / Math.pow(10, collateralDecimals) * collateralPrice;
  $: debtValue = Number(vault.borrowed_icusd_amount) / 1e8;
  $: cr = debtValue > 0 ? (collateralValue / debtValue) * 100 : Infinity;
  $: crColor = cr >= 200 ? 'var(--rumi-safe)' : cr >= 150 ? 'var(--rumi-caution)' : 'var(--rumi-danger)';
  $: ownerStr = vault.owner?.toString?.() || vault.owner || '';
</script>

<a class="vault-card glass-card" href="/explorer/vault/{vault.vault_id}">
  <div class="vault-header">
    <span class="vault-id">Vault #{Number(vault.vault_id)}</span>
    <span class="vault-cr" style="color:{crColor}">
      {cr === Infinity ? '∞' : cr.toFixed(0)}% CR
    </span>
  </div>
  <div class="vault-details">
    <div class="vault-detail">
      <span class="label">Owner</span>
      <span class="value link">{truncatePrincipal(ownerStr)}</span>
    </div>
    <div class="vault-detail">
      <span class="label">Collateral</span>
      <span class="value key-number">{formatAmount(vault.collateral_amount, collateralDecimals)} {collateralSymbol}</span>
    </div>
    <div class="vault-detail">
      <span class="label">Debt</span>
      <span class="value key-number">{formatAmount(vault.borrowed_icusd_amount)} icUSD</span>
    </div>
  </div>
</a>

<style>
  .vault-card { display:block; padding:1rem; text-decoration:none; color:var(--rumi-text-primary); transition:border-color 0.15s; cursor:pointer; }
  .vault-card:hover { border-color:var(--rumi-border-hover); }
  .vault-header { display:flex; justify-content:space-between; align-items:center; margin-bottom:0.75rem; }
  .vault-id { font-weight:600; font-size:1rem; }
  .vault-cr { font-weight:600; font-size:0.875rem; font-variant-numeric:tabular-nums; }
  .vault-details { display:flex; flex-direction:column; gap:0.375rem; }
  .vault-detail { display:flex; justify-content:space-between; font-size:0.8125rem; }
  .label { color:var(--rumi-text-muted); }
  .value { color:var(--rumi-text-secondary); }
  .link { color:var(--rumi-purple-accent); text-decoration:none; }
  .link:hover { text-decoration:underline; }
</style>
