<script lang="ts" context="module">
  import type { PoolStatus, UserPosition, LiquidationRecord } from '../../services/stabilityPoolService';

  let _poolStatus: PoolStatus | null = null;
  let _protocolStatus: any = null;
  let _userPosition: UserPosition | null = null;
  let _liquidationHistory: LiquidationRecord[] = [];
  let _cachedForPrincipal: string | null = null;
</script>

<script lang="ts">
  import { onMount } from 'svelte';
  import { walletStore } from '../../stores/wallet';
  import { stabilityPoolService } from '../../services/stabilityPoolService';
  import { QueryOperations } from '../../services/protocol/queryOperations';
  import DepositInterface from '../stability-pool/DepositInterface.svelte';
  import EarnInfoCard from '../stability-pool/EarnInfoCard.svelte';
  import LiquidationHistoryCard from '../stability-pool/LiquidationHistoryCard.svelte';
  import LoadingSpinner from '../common/LoadingSpinner.svelte';

  let hasCachedData = _poolStatus !== null;
  let loading = !hasCachedData;
  let error = '';
  let poolStatus: PoolStatus | null = _poolStatus;
  let protocolStatus: any = _protocolStatus;
  let userPosition: UserPosition | null = _userPosition;
  let liquidationHistory: LiquidationRecord[] = _liquidationHistory;

  $: isConnected = $walletStore.isConnected;
  $: principal = $walletStore.principal;

  $: poolApy = (() => {
    if (!protocolStatus || !poolStatus) return null;
    const poolShare = (protocolStatus.interestSplit?.find(e => e.destination === 'stability_pool')?.bps ?? 0) / 10000;
    const perC = protocolStatus.perCollateralInterest;
    if (!perC || perC.length === 0) return null;

    const eligibleMap = new Map<string, number>(
      (poolStatus.eligible_icusd_per_collateral ?? []).map(([p, v]: [any, bigint]) => [p.toText(), Number(v) / 1e8])
    );

    let totalApr = 0;
    for (const info of perC) {
      const eligible = eligibleMap.get(info.collateralType) ?? 0;
      if (eligible === 0 || info.totalDebtE8s === 0 || info.weightedInterestRate === 0) continue;
      totalApr += (info.weightedInterestRate * poolShare * info.totalDebtE8s) / eligible;
    }
    if (totalApr === 0) return null;
    const apy = Math.pow(1 + totalApr / 365, 365) - 1;
    return (apy * 100).toFixed(2);
  })();

  let showApyTooltip = false;

  async function loadAllData() {
    try {
      if (!hasCachedData) {
        loading = true;
      }
      error = '';

      const [pool, proto] = await Promise.all([
        stabilityPoolService.getPoolStatus(),
        QueryOperations.getProtocolStatus().catch(() => null),
      ]);
      poolStatus = pool;
      protocolStatus = proto;

      _poolStatus = pool;
      _protocolStatus = proto;

      if (isConnected && principal) {
        const [position, history] = await Promise.all([
          stabilityPoolService.getUserPosition(principal).catch(() => null),
          stabilityPoolService.getLiquidationHistory(50).catch(() => []),
        ]);
        userPosition = position;
        liquidationHistory = history;

        _userPosition = position;
        _liquidationHistory = history;
        _cachedForPrincipal = principal.toString();
      } else {
        userPosition = null;
        liquidationHistory = [];
        _userPosition = null;
        _liquidationHistory = [];
        _cachedForPrincipal = null;
      }

      hasCachedData = true;
    } catch (err: any) {
      console.error('Failed to load stability pool data:', err);
      if (!hasCachedData) {
        error = err.message || 'Failed to load stability pool data';
      }
    } finally {
      loading = false;
    }
  }

  let previousConnected = false;
  $: if (isConnected !== previousConnected) {
    previousConnected = isConnected;
    if (!isConnected || principal?.toString() !== _cachedForPrincipal) {
      _userPosition = null;
      _liquidationHistory = [];
      _cachedForPrincipal = null;
      userPosition = null;
      liquidationHistory = [];
    }
    loadAllData();
  }

  onMount(() => { loadAllData(); });

  function handleSuccess() {
    loadAllData();
    walletStore.refreshBalance();
  }
</script>

<div class="pool-container">
  {#if poolApy !== null}
    <div class="apy-row">
      <!-- svelte-ignore a11y-mouse-events-have-key-events -->
      <div
        class="apy-badge"
        on:mouseover={() => { showApyTooltip = true; }}
        on:mouseleave={() => { showApyTooltip = false; }}
      >
        <svg class="apy-arrow" width="10" height="10" viewBox="0 0 10 10" fill="none">
          <path d="M5 8V2M5 2L2 5M5 2L8 5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
        </svg>
        {poolApy}% APY<span class="apy-asterisk">*</span>

        {#if showApyTooltip}
          <div class="apy-tooltip">
            <div class="apy-tooltip-caret"></div>
            <p><strong>Interest APY</strong> applies to <strong>icUSD</strong> deposits. icUSD depositors earn a share of all borrowing interest paid by vault owners.</p>
            <div class="apy-tooltip-divider"></div>
            <p><strong>ckUSDC</strong>, <strong>ckUSDT</strong>, and <strong>3USD</strong> deposits don't earn interest directly, but are used <em>first</em> for liquidations, giving priority access to discounted collateral. <strong>3USD</strong> LP tokens also earn yield from swap fees and interest donations in the 3pool.</p>
          </div>
        {/if}
      </div>
    </div>
  {/if}

  {#if loading}
    <div class="loading-state">
      <LoadingSpinner />
      <p class="loading-text">Loading pool data…</p>
    </div>
  {:else if error}
    <div class="error-state">
      <div class="error-icon">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
          <circle cx="12" cy="12" r="10"/>
          <line x1="12" y1="8" x2="12" y2="12"/>
          <line x1="12" y1="16" x2="12.01" y2="16"/>
        </svg>
      </div>
      <p class="error-text">{error}</p>
      <button class="btn-primary" on:click={loadAllData}>Try Again</button>
    </div>
  {:else}
    <div class="page-layout">
      <div class="stats-column">
        <EarnInfoCard {poolStatus} {userPosition} {protocolStatus} {isConnected} on:success={handleSuccess} />
      </div>

      <div class="action-column">
        <DepositInterface {poolStatus} {userPosition} on:success={handleSuccess} />
        <div class="liq-section">
          <LiquidationHistoryCard {poolStatus} {liquidationHistory} />
        </div>
      </div>
    </div>
  {/if}
</div>

<style>
  .pool-container { max-width: 820px; margin: 0 auto; }

  .apy-row {
    display: flex;
    align-items: center;
    margin-bottom: 1rem;
    position: relative;
    z-index: 10;
  }

  .apy-badge {
    position: relative;
    display: inline-flex;
    align-items: center;
    gap: 0.3125rem;
    padding: 0.25rem 0.75rem;
    background: rgba(74, 222, 128, 0.1);
    border: 1px solid rgba(74, 222, 128, 0.3);
    border-radius: 1.25rem;
    font-size: 0.8125rem;
    font-weight: 600;
    color: #4ade80;
    cursor: default;
    white-space: nowrap;
  }

  .apy-arrow { color: #4ade80; flex-shrink: 0; }

  .apy-asterisk {
    font-size: 0.625rem;
    opacity: 0.6;
    margin-left: -0.125rem;
    vertical-align: super;
  }

  .apy-tooltip {
    position: absolute;
    top: calc(100% + 0.625rem);
    left: 50%;
    transform: translateX(-50%);
    z-index: 50;
    width: 17rem;
    padding: 0.75rem 0.875rem;
    background: #1e293b;
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 0.5rem;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.5);
    font-size: 0.6875rem;
    font-weight: 400;
    line-height: 1.5;
    color: #94a3b8;
    cursor: default;
    white-space: normal;
    word-wrap: break-word;
    overflow-wrap: break-word;
    animation: tooltipFade 0.15s ease-out;
  }

  @keyframes tooltipFade {
    from { opacity: 0; transform: translateX(-50%) translateY(4px); }
    to { opacity: 1; transform: translateX(-50%) translateY(0); }
  }

  .apy-tooltip-caret {
    position: absolute;
    top: -5px;
    left: 50%;
    transform: translateX(-50%) rotate(45deg);
    width: 10px;
    height: 10px;
    background: #1e293b;
    border-top: 1px solid rgba(255, 255, 255, 0.08);
    border-left: 1px solid rgba(255, 255, 255, 0.08);
  }

  .apy-tooltip p { margin: 0; }
  .apy-tooltip strong { color: #cbd5e1; font-weight: 600; }
  .apy-tooltip em { font-style: italic; }

  .apy-tooltip-divider {
    height: 1px;
    background: rgba(255, 255, 255, 0.06);
    margin: 0.5rem 0;
  }

  .page-layout {
    display: grid;
    grid-template-columns: 280px 1fr;
    gap: 1.5rem;
    align-items: start;
  }

  .stats-column { position: sticky; top: 5rem; }

  .action-column {
    min-width: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 1rem;
  }

  .action-column > :global(*) { width: 100%; max-width: 420px; }
  .liq-section { width: 100%; max-width: 420px; }

  .loading-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    padding: 5rem;
    color: var(--rumi-text-secondary);
  }

  .loading-text {
    margin-top: 1rem;
    font-size: 0.875rem;
  }

  .error-state {
    text-align: center;
    padding: 4rem 1rem;
  }

  .error-icon {
    width: 2.5rem;
    height: 2.5rem;
    color: var(--rumi-danger);
    margin: 0 auto 1rem;
  }

  .error-text {
    font-size: 0.875rem;
    color: var(--rumi-danger);
    margin-bottom: 1.5rem;
  }

  @media (max-width: 768px) {
    .page-layout { grid-template-columns: 1fr; }
    .stats-column { position: static; order: 2; }
    .action-column { order: 1; }
  }

  @media (max-width: 520px) {
    .pool-container {
      padding-left: 0.5rem;
      padding-right: 0.5rem;
    }

    .apy-tooltip {
      left: 0;
      transform: none;
      width: calc(100vw - 2rem);
    }

    .apy-tooltip-caret {
      left: 2rem;
      transform: rotate(45deg);
    }
  }
</style>
