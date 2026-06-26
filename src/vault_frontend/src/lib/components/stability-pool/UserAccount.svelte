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
  import { formatStableTokenDisplay } from '../../utils/format';
  import type { PoolStatus, UserPosition, CollateralInfo } from '../../services/stabilityPoolService';

  export let poolStatus: PoolStatus | null = null;
  export let userPosition: UserPosition | null = null;

  const dispatch = createEventDispatcher();

  let claimLoading: Record<string, boolean> = {};
  let claimAllLoading = false;
  let toggleLoading: Record<string, boolean> = {};
  let nativeAddressInputs: Record<string, string> = {};
  let error = '';
  let showOptOutMenu = false;
  let showOptOutTooltip = false;

  $: stablecoinRegistry = poolStatus?.stablecoin_registry ?? [];
  // Hide PHASMA (test-only collateral) and enforce display order matching borrow page.
  const HIDDEN_COLLATERAL = new Set(['PHASMA']);
  const COLLATERAL_ORDER: Record<string, number> = { ICP: 0, XRP: 1, ckBTC: 2, ckETH: 3, ckXAUT: 4, nICP: 5, BOB: 6, EXE: 7 };
  $: collateralRegistryRaw = poolStatus?.collateral_registry ?? [];
  $: collateralRegistry = collateralRegistryRaw
    .filter(c => !HIDDEN_COLLATERAL.has(c.symbol))
    .sort((a, b) => (COLLATERAL_ORDER[a.symbol] ?? 99) - (COLLATERAL_ORDER[b.symbol] ?? 99));
  $: registries = { stablecoins: stablecoinRegistry, collateral: collateralRegistry };

  $: userStables = userPosition?.stablecoin_balances ?? [];
  $: totalUsdValue = userPosition ? formatStableTokenDisplay(userPosition.total_usd_value_e8s, 8) : '0.0000';
  $: gains = userPosition?.collateral_gains ?? [];
  $: optedOut = new Set((userPosition?.opted_out_collateral ?? []).map(p => p.toText()));
  $: nativePayoutByCollateral = new Map((userPosition?.native_payout_addresses ?? []).map(([p, address]) => [p.toText(), address]));

  $: {
    let nextInputs = nativeAddressInputs;
    for (const [key, address] of nativePayoutByCollateral) {
      if (nextInputs[key] === undefined) {
        nextInputs = { ...nextInputs, [key]: address };
      }
    }
    nativeAddressInputs = nextInputs;
  }

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

  function isNativeCollateral(collateral: CollateralInfo) {
    return collateral.symbol === 'XRP';
  }

  function isCollateralOut(collateral: CollateralInfo) {
    const key = collateral.ledger_id.toText();
    return isNativeCollateral(collateral) ? !nativePayoutByCollateral.has(key) : optedOut.has(key);
  }

  $: hasClaimableIcrcGains = gains.some(([ledger, amount]) => {
    if (amount <= 0n) return false;
    const collateral = collateralRegistry.find(c => c.ledger_id.toText() === ledger.toText());
    return collateral ? !isNativeCollateral(collateral) : false;
  });

  function setNativeAddressInput(key: string, value: string) {
    nativeAddressInputs = { ...nativeAddressInputs, [key]: value };
  }

  async function saveNativeOptIn(collateral: CollateralInfo) {
    const key = collateral.ledger_id.toText();
    try {
      toggleLoading = { ...toggleLoading, [key]: true };
      error = '';
      await stabilityPoolService.optInNativeCollateral(collateral.ledger_id, nativeAddressInputs[key] ?? '');
      dispatch('success', { action: 'optInNative' });
    } catch (err: any) {
      error = err.message || 'XRP opt-in failed';
    } finally {
      toggleLoading = { ...toggleLoading, [key]: false };
    }
  }

  async function toggleOptOut(collateral: CollateralInfo) {
    const key = collateral.ledger_id.toText();
    const isCurrentlyOut = isCollateralOut(collateral);
    try {
      toggleLoading = { ...toggleLoading, [key]: true };
      error = '';
      if (isNativeCollateral(collateral)) {
        if (isCurrentlyOut) {
          await stabilityPoolService.optInNativeCollateral(collateral.ledger_id, nativeAddressInputs[key] ?? '');
        } else {
          await stabilityPoolService.optOutCollateral(collateral.ledger_id);
        }
      } else if (isCurrentlyOut) {
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

  function closeOptOutMenu() {
    showOptOutMenu = false;
  }
</script>

<svelte:window on:click={closeOptOutMenu} />

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
            <span class="br-amount">{formatStableTokenDisplay(amount, dec)}</span>
          </div>
        {/if}
      {/each}
    </div>

    <div class="meta-row">
      <div class="meta-item">
        <span class="meta-label">Since</span>
        <span class="meta-value">{depositDate}</span>
      </div>
      {#if userPosition && (userPosition as any).total_interest_earned_e8s > 0n}
        <div class="meta-item">
          <span class="meta-label">Interest Earned</span>
          <span class="meta-value interest-earned">
            <span class="tv-dollar">$</span>{formatStableTokenDisplay((userPosition as any).total_interest_earned_e8s, 8)}
          </span>
        </div>
      {/if}
    </div>

    <!-- Collateral gains -->
    <div class="gains-section">
      <div class="gains-header">
        <h4 class="gains-title">Collateral Gains</h4>
        <div class="gains-header-actions">
          {#if hasClaimableIcrcGains}
            <button class="claim-all-btn" on:click={claimAll} disabled={claimAllLoading}>
              {#if claimAllLoading}
                <span class="mini-spinner"></span>
              {:else}
                Claim All
              {/if}
            </button>
          {/if}
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div class="opt-out-wrapper" on:click|stopPropagation>
            <!-- svelte-ignore a11y-mouse-events-have-key-events -->
            <button
              class="opt-out-info-btn"
              on:mouseover={() => { showOptOutTooltip = true; }}
              on:mouseleave={() => { showOptOutTooltip = false; }}
              on:click|stopPropagation={() => { showOptOutMenu = !showOptOutMenu; showOptOutTooltip = false; }}
            >
              Routing
              <svg class="info-icon" width="12" height="12" viewBox="0 0 16 16" fill="none">
                <circle cx="8" cy="8" r="7" stroke="currentColor" stroke-width="1.5"/>
                <path d="M8 7v4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
                <circle cx="8" cy="5" r="0.75" fill="currentColor"/>
              </svg>
            </button>
            {#if showOptOutTooltip && !showOptOutMenu}
              <div class="opt-out-tooltip">
                Choose which collateral types you receive during liquidations. XRP requires a payout address.
              </div>
            {/if}
            {#if showOptOutMenu}
              <div class="opt-out-menu">
                {#each collateralRegistry as collateral}
                  {@const key = collateral.ledger_id.toText()}
                  {@const isNative = isNativeCollateral(collateral)}
                  {@const isOut = isCollateralOut(collateral)}
                  {#if isNative}
                    <div class="native-opt-row" class:is-out={isOut}>
                      <div class="native-opt-head">
                        <span class="opt-out-symbol">{collateral.symbol}</span>
                        {#if toggleLoading[key]}
                          <span class="mini-spinner"></span>
                        {:else}
                          <span class="opt-out-status" class:opted-out-label={isOut}>{isOut ? 'Off' : 'Receiving'}</span>
                        {/if}
                      </div>
                      <input
                        class="native-address-input"
                        type="text"
                        inputmode="text"
                        placeholder="r..."
                        value={nativeAddressInputs[key] ?? nativePayoutByCollateral.get(key) ?? ''}
                        disabled={toggleLoading[key]}
                        on:input={(event) => setNativeAddressInput(key, (event.currentTarget as HTMLInputElement).value)}
                      />
                      <div class="native-opt-actions">
                        <button
                          class="native-save-btn"
                          on:click={() => saveNativeOptIn(collateral)}
                          disabled={toggleLoading[key]}
                        >
                          {nativePayoutByCollateral.has(key) ? 'Update' : 'Enable'}
                        </button>
                        {#if nativePayoutByCollateral.has(key)}
                          <button
                            class="native-disable-btn"
                            on:click={() => toggleOptOut(collateral)}
                            disabled={toggleLoading[key]}
                          >
                            Disable
                          </button>
                        {/if}
                      </div>
                    </div>
                  {:else}
                    <button
                      class="opt-out-row" class:is-out={isOut}
                      on:click={() => toggleOptOut(collateral)}
                      disabled={toggleLoading[key]}
                    >
                      <span class="opt-out-symbol">{collateral.symbol}</span>
                      {#if toggleLoading[key]}
                        <span class="mini-spinner"></span>
                      {:else}
                        <span class="opt-out-status" class:opted-out-label={isOut}>{isOut ? 'Opted out' : 'Receiving'}</span>
                      {/if}
                    </button>
                  {/if}
                {/each}
              </div>
            {/if}
          </div>
        </div>
      </div>

      <div class="gains-list">
        {#each collateralRegistry as collateral}
          {@const key = collateral.ledger_id.toText()}
          {@const gainEntry = gains.find(([l]) => l.toText() === key)}
          {@const gainAmount = gainEntry ? gainEntry[1] : 0n}
          {@const isNative = isNativeCollateral(collateral)}
          {@const isOut = isCollateralOut(collateral)}

          <div class="gain-row" class:opted-out={isOut}>
            <div class="gain-info">
              <div class="gain-token">
                <span class="gain-symbol">{collateral.symbol}</span>
                {#if isOut}
                  <span class="opted-out-badge">{isNative ? 'OFF' : 'OUT'}</span>
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
              {#if gainAmount > 0n && !isNative}
                <button class="claim-btn" on:click={() => claimSingle(collateral.ledger_id)} disabled={claimLoading[key]}>
                  {claimLoading[key] ? '…' : 'Claim'}
                </button>
              {/if}
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
  .interest-earned { color: var(--rumi-teal); font-weight: 600; }

  .gains-section { border-top: 1px solid var(--rumi-border); padding-top: 1.25rem; }
  .gains-header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 0.75rem; }
  .gains-header-actions { display: flex; align-items: center; gap: 0.5rem; }

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

  /* ── Opt-out button + menu ── */
  .opt-out-wrapper { position: relative; }

  .opt-out-info-btn {
    display: flex; align-items: center; gap: 0.25rem;
    padding: 0.1875rem 0.5rem; background: transparent;
    border: 1px solid var(--rumi-border); border-radius: 0.375rem;
    color: var(--rumi-text-muted); font-size: 0.6875rem; font-weight: 500;
    cursor: pointer; transition: all 0.15s ease; white-space: nowrap;
  }
  .opt-out-info-btn:hover { border-color: var(--rumi-border-hover); color: var(--rumi-text-secondary); }
  .info-icon { flex-shrink: 0; opacity: 0.6; }

  .opt-out-tooltip {
    position: absolute; top: calc(100% + 0.5rem); right: 0; z-index: 20;
    width: 14rem; padding: 0.625rem 0.75rem;
    background: var(--rumi-bg-surface1); border: 1px solid var(--rumi-border);
    border-radius: 0.5rem; box-shadow: 0 4px 16px rgba(0,0,0,0.4);
    font-size: 0.6875rem; line-height: 1.5; color: var(--rumi-text-secondary);
    pointer-events: none;
  }

  .opt-out-menu {
    position: absolute; top: calc(100% + 0.375rem); right: 0; z-index: 30;
    min-width: 12rem; width: min(18rem, calc(100vw - 2rem)); padding: 0.375rem;
    background: var(--rumi-bg-surface1); border: 1px solid var(--rumi-border);
    border-radius: 0.5rem; box-shadow: 0 4px 16px rgba(0,0,0,0.4);
    display: flex; flex-direction: column; gap: 0.125rem;
  }

  .opt-out-row {
    display: flex; justify-content: space-between; align-items: center;
    padding: 0.4375rem 0.625rem; background: transparent;
    border: none; border-radius: 0.375rem; cursor: pointer;
    transition: background 0.15s ease; width: 100%; text-align: left;
  }
  .opt-out-row:hover:not(:disabled) { background: var(--rumi-bg-surface2); }
  .opt-out-row:disabled { opacity: 0.4; cursor: not-allowed; }
  .opt-out-symbol { font-size: 0.75rem; font-weight: 600; color: var(--rumi-text-primary); }
  .opt-out-status { font-size: 0.6875rem; color: var(--rumi-teal); font-weight: 500; }
  .opt-out-status.opted-out-label { color: var(--rumi-danger); }

  .native-opt-row {
    display: flex; flex-direction: column; gap: 0.5rem;
    padding: 0.625rem; border-radius: 0.375rem;
    background: var(--rumi-bg-surface2); border: 1px solid var(--rumi-border);
  }
  .native-opt-row.is-out { border-color: rgba(74, 144, 217, 0.22); }
  .native-opt-head { display: flex; align-items: center; justify-content: space-between; gap: 0.75rem; }
  .native-address-input {
    width: 100%; min-width: 0; height: 2rem;
    padding: 0 0.625rem; border-radius: 0.375rem;
    border: 1px solid var(--rumi-border); background: var(--rumi-bg-surface1);
    color: var(--rumi-text-primary); font-size: 0.75rem;
    font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
    outline: none; transition: border-color 0.15s ease, background 0.15s ease;
  }
  .native-address-input:focus {
    border-color: rgba(74, 144, 217, 0.55);
    background: var(--rumi-bg-surface3);
  }
  .native-address-input::placeholder { color: var(--rumi-text-muted); }
  .native-opt-actions { display: flex; gap: 0.375rem; }
  .native-save-btn,
  .native-disable-btn {
    flex: 1; min-width: 0; height: 1.75rem;
    border-radius: 0.3125rem; font-size: 0.6875rem; font-weight: 600;
    cursor: pointer; transition: all 0.15s ease;
  }
  .native-save-btn {
    border: 1px solid rgba(74, 144, 217, 0.45);
    background: rgba(74, 144, 217, 0.14); color: #8ec5ff;
  }
  .native-save-btn:hover:not(:disabled) { background: rgba(74, 144, 217, 0.2); }
  .native-disable-btn {
    border: 1px solid var(--rumi-border);
    background: transparent; color: var(--rumi-text-secondary);
  }
  .native-disable-btn:hover:not(:disabled) { border-color: var(--rumi-border-hover); color: var(--rumi-text-primary); }
  .native-save-btn:disabled,
  .native-disable-btn:disabled { opacity: 0.45; cursor: not-allowed; }

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
