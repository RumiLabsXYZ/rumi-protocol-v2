<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import { Principal } from '@dfinity/principal';
  import {
    stabilityPoolService,
    formatE8s,
    formatTokenAmount,
    symbolForLedger,
    decimalsForLedger,
  } from '../../services/stabilityPoolService';
  import type { PoolStatus, UserPosition, CollateralInfo } from '../../services/stabilityPoolService';

  export let poolStatus: PoolStatus | null = null;
  export let userPosition: UserPosition | null = null;

  const dispatch = createEventDispatcher();

  let claimLoading: Record<string, boolean> = {};
  let claimAllLoading = false;
  let toggleLoading: Record<string, boolean> = {};
  let error = '';

  $: stablecoinRegistry = poolStatus?.stablecoin_registry ?? [];
  $: collateralRegistry = poolStatus?.collateral_registry ?? [];
  $: registries = { stablecoins: stablecoinRegistry, collateral: collateralRegistry };

  $: userStables = userPosition?.stablecoin_balances ?? [];
  $: totalUsdValue = userPosition ? formatE8s(userPosition.total_usd_value_e8s) : '0';
  $: gains = userPosition?.collateral_gains ?? [];
  $: hasAnyGains = gains.some(([_, a]) => a > 0n);
  $: optedOut = new Set((userPosition?.opted_out_collateral ?? []).map(p => p.toText()));

  $: depositDate = userPosition?.deposit_timestamp
    ? new Date(Number(userPosition.deposit_timestamp) / 1_000_000).toLocaleDateString('en-US', { month: 'short', day: 'numeric', year: 'numeric' })
    : '—';

  $: poolShare = (() => {
    if (!poolStatus || !userPosition || poolStatus.total_deposits_e8s === 0n) return '0.00';
    const share = (Number(userPosition.total_usd_value_e8s) / Number(poolStatus.total_deposits_e8s)) * 100;
    return share < 0.01 && share > 0 ? '<0.01' : share.toFixed(2);
  })();

  async function claimSingle(ledger: Principal) {
    const key = ledger.toText();
    try {
      claimLoading = { ...claimLoading, [key]: true };
      error = '';
      await stabilityPoolService.claimCollateral(ledger);
      dispatch('success', { action: 'claim' });
    } catch (err: any) {
      error = err.message || 'Claim failed';
    } finally {
      claimLoading = { ...claimLoading, [key]: false };
    }
  }

  async function claimAll() {
    try {
      claimAllLoading = true;
      error = '';
      await stabilityPoolService.claimAllCollateral();
      dispatch('success', { action: 'claimAll' });
    } catch (err: any) {
      error = err.message || 'Claim all failed';
    } finally {
      claimAllLoading = false;
    }
  }

  async function toggleOptOut(collateral: CollateralInfo) {
    const key = collateral.ledger_id.toText();
    const isCurrentlyOut = optedOut.has(key);
    try {
      toggleLoading = { ...toggleLoading, [key]: true };
      error = '';
      if (isCurrentlyOut) {
        await stabilityPoolService.optInCollateral(collateral.ledger_id);
      } else {
        await stabilityPoolService.optOutCollateral(collateral.ledger_id);
      }
      dispatch('success', { action: isCurrentlyOut ? 'optIn' : 'optOut' });
    } catch (err: any) {
      error = err.message || 'Toggle failed';
    } finally {
      toggleLoading = { ...toggleLoading, [key]: false };
    }
  }
</script>

{#if userPosition}
  <div class="position-card">
    <div class="card-header">
      <h3 class="card-title">Your Position</h3>
      <div class="pool-share-badge">{poolShare}% of pool</div>
    </div>

    <div class="total-value-row">
      <span class="tv-label">Total Deposited</span>
      <span class="tv-amount"><span class="tv-dollar">$</span>{totalUsdValue}</span>
    </div>

    <div class="token-breakdown">
      {#each userStables as [ledger, amount]}
        {@const sym = symbolForLedger(ledger, registries)}
        {@const dec = decimalsForLedger(ledger, registries)}
        {#if amount > 0n}
          <div class="breakdown-row">
            <span class="br-symbol">{sym}</span>
            <span class="br-amount">{formatTokenAmount(amount, dec)}</span>
          </div>
        {/if}
      {/each}
    </div>

    <div class="meta-row">
      <div class="meta-item">
        <span class="meta-label">Since</span>
        <span class="meta-value">{depositDate}</span>
      </div>
    </div>

    <!-- Collateral gains -->
    <div class="gains-section">
      <div class="gains-header">
        <h4 class="gains-title">Collateral Gains</h4>
        {#if hasAnyGains}
          <button class="claim-all-btn" on:click={claimAll} disabled={claimAllLoading}>
            {#if claimAllLoading}
              <span class="mini-spinner"></span>
            {:else}
              Claim All
            {/if}
          </button>
        {/if}
      </div>

      <div class="gains-list">
        {#each collateralRegistry as collateral}
          {@const key = collateral.ledger_id.toText()}
          {@const gainEntry = gains.find(([l]) => l.toText() === key)}
          {@const gainAmount = gainEntry ? gainEntry[1] : 0n}
          {@const isOut = optedOut.has(key)}

          <div class="gain-row" class:opted-out={isOut}>
            <div class="gain-info">
              <div class="gain-token">
                <span class="gain-symbol">{collateral.symbol}</span>
                {#if isOut}
                  <span class="opted-out-badge">OUT</span>
                {/if}
              </div>
              <div class="gain-amount">
                {#if gainAmount > 0n}
                  <span class="gain-value">{formatTokenAmount(gainAmount, collateral.decimals)}</span>
                {:else}
                  <span class="gain-zero">0</span>
                {/if}
              </div>
            </div>

            <div class="gain-actions">
              {#if gainAmount > 0n}
                <button class="claim-btn" on:click={() => claimSingle(collateral.ledger_id)} disabled={claimLoading[key]}>
                  {claimLoading[key] ? '…' : 'Claim'}
                </button>
              {/if}
              <button
                class="toggle-btn" class:is-out={isOut}
                on:click={() => toggleOptOut(collateral)}
                disabled={toggleLoading[key]}
                title={isOut ? 'Opt in to receive this collateral' : 'Opt out of receiving this collateral'}
              >
                {#if toggleLoading[key]}
                  <span class="mini-spinner"></span>
                {:else}
                  {isOut ? '✕' : '✓'}
                {/if}
              </button>
            </div>
          </div>
        {/each}
      </div>
    </div>

    {#if error}
      <div class="error-bar">{error}</div>
    {/if}
  </div>
{/if}

<style>
  .position-card {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    padding: 1.5rem;
    box-shadow: inset 0 1px 0 0 rgba(200, 210, 240, 0.03), 0 2px 8px -2px rgba(8, 11, 22, 0.6);
  }

  .card-header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 1.25rem; }

  .card-title {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 1rem; font-weight: 600; color: var(--rumi-text-primary);
  }

  .pool-share-badge {
    padding: 0.1875rem 0.625rem;
    background: var(--rumi-teal-dim); border: 1px solid var(--rumi-border-teal);
    border-radius: 1rem; font-size: 0.6875rem; font-weight: 600; color: var(--rumi-teal);
  }

  .total-value-row {
    display: flex; justify-content: space-between; align-items: baseline;
    margin-bottom: 0.75rem; padding-bottom: 0.75rem; border-bottom: 1px solid var(--rumi-border);
  }

  .tv-label { font-size: 0.8125rem; color: var(--rumi-text-secondary); }
  .tv-amount { font-size: 1.5rem; font-weight: 700; font-variant-numeric: tabular-nums; color: var(--rumi-text-primary); }
  .tv-dollar { color: var(--rumi-text-secondary); font-weight: 400; font-size: 1rem; }

  .token-breakdown { display: flex; flex-direction: column; gap: 0.375rem; margin-bottom: 1rem; }
  .breakdown-row { display: flex; justify-content: space-between; align-items: center; padding: 0.375rem 0; }
  .br-symbol { font-size: 0.8125rem; color: var(--rumi-text-secondary); }
  .br-amount { font-size: 0.875rem; font-weight: 600; font-variant-numeric: tabular-nums; color: var(--rumi-text-primary); }

  .meta-row {
    display: flex; gap: 1.5rem; margin-bottom: 1.25rem;
    padding: 0.625rem 0.75rem; background: var(--rumi-bg-surface2); border-radius: 0.5rem;
  }
  .meta-item { display: flex; flex-direction: column; gap: 0.125rem; }
  .meta-label { font-size: 0.625rem; text-transform: uppercase; letter-spacing: 0.06em; color: var(--rumi-text-muted); }
  .meta-value { font-size: 0.8125rem; font-weight: 500; color: var(--rumi-text-primary); }

  .gains-section { border-top: 1px solid var(--rumi-border); padding-top: 1.25rem; }
  .gains-header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 0.75rem; }

  .gains-title {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 0.875rem; font-weight: 600; color: var(--rumi-text-primary);
  }

  .claim-all-btn {
    padding: 0.25rem 0.75rem; background: var(--rumi-action); color: var(--rumi-bg-primary);
    border: none; border-radius: 0.375rem; font-size: 0.75rem; font-weight: 600;
    cursor: pointer; transition: all 0.15s ease; display: flex; align-items: center; gap: 0.375rem;
  }
  .claim-all-btn:hover:not(:disabled) { background: var(--rumi-action-bright); }
  .claim-all-btn:disabled { opacity: 0.4; cursor: not-allowed; }

  .gains-list { display: flex; flex-direction: column; gap: 0.5rem; }

  .gain-row {
    display: flex; justify-content: space-between; align-items: center;
    padding: 0.625rem 0.75rem; background: var(--rumi-bg-surface2);
    border: 1px solid transparent; border-radius: 0.5rem; transition: all 0.15s ease;
  }
  .gain-row:hover { border-color: var(--rumi-border); }
  .gain-row.opted-out { opacity: 0.5; }

  .gain-info { display: flex; align-items: center; gap: 1rem; }
  .gain-token { display: flex; align-items: center; gap: 0.375rem; }
  .gain-symbol { font-size: 0.8125rem; font-weight: 600; color: var(--rumi-text-primary); min-width: 3.5rem; }

  .opted-out-badge {
    font-size: 0.5625rem; font-weight: 700; text-transform: uppercase;
    color: var(--rumi-danger); padding: 0.0625rem 0.375rem;
    background: rgba(224, 107, 159, 0.1); border-radius: 0.25rem;
  }

  .gain-value { font-size: 0.875rem; font-weight: 600; color: var(--rumi-teal); }
  .gain-zero { font-size: 0.875rem; color: var(--rumi-text-muted); }

  .gain-actions { display: flex; align-items: center; gap: 0.375rem; }

  .claim-btn {
    padding: 0.25rem 0.625rem; background: var(--rumi-teal-dim);
    border: 1px solid var(--rumi-border-teal); border-radius: 0.25rem;
    color: var(--rumi-teal); font-size: 0.6875rem; font-weight: 600;
    cursor: pointer; transition: all 0.15s ease;
  }
  .claim-btn:hover:not(:disabled) { background: rgba(45, 212, 191, 0.15); }
  .claim-btn:disabled { opacity: 0.4; cursor: not-allowed; }

  .toggle-btn {
    width: 1.5rem; height: 1.5rem; padding: 0;
    display: flex; align-items: center; justify-content: center;
    background: transparent; border: 1px solid var(--rumi-border);
    border-radius: 0.25rem; color: var(--rumi-teal); font-size: 0.625rem;
    cursor: pointer; transition: all 0.15s ease;
  }
  .toggle-btn:hover:not(:disabled) { border-color: var(--rumi-border-hover); }
  .toggle-btn.is-out { color: var(--rumi-danger); border-color: rgba(224, 107, 159, 0.2); }
  .toggle-btn:disabled { opacity: 0.4; cursor: not-allowed; }

  .mini-spinner {
    display: inline-block; width: 0.75rem; height: 0.75rem;
    border: 1.5px solid transparent; border-top-color: currentColor;
    border-radius: 50%; animation: spin 0.8s linear infinite;
  }
  @keyframes spin { to { transform: rotate(360deg); } }

  .error-bar {
    margin-top: 0.75rem; padding: 0.625rem 0.75rem;
    background: rgba(224, 107, 159, 0.08); border: 1px solid rgba(224, 107, 159, 0.2);
    border-radius: 0.375rem; color: var(--rumi-danger); font-size: 0.8125rem;
  }
</style>
