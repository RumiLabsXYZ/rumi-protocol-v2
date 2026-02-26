<script lang="ts">
  import { onMount } from 'svelte';
  import { walletStore as wallet } from '$lib/stores/wallet';
  import { protocolService } from '$lib/services/protocol';
  import { formatNumber } from '$lib/utils/format';
  import { CANISTER_IDS } from '$lib/config';
  import { TokenService } from '$lib/services/tokenService';
  import { Principal } from '@dfinity/principal';
  import ProtocolStats from '$lib/components/dashboard/ProtocolStats.svelte';

  // ── State ───────────────────────────────────────────────────────────
  let isConnected = false;
  let icpPrice = 0;
  let vaultRedemptionFee = 0;     // dynamic vault redemption fee (from get_fees)
  let reserveRedemptionFee = 0;   // flat reserve fee (from protocol status)
  let reserveRedemptionsEnabled = false;
  let icusdBalance = 0;
  let icusdAmount = 0;
  let isLoading = true;
  let actionInProgress = false;
  let errorMessage = '';
  let successMessage = '';

  // Reserve balances (raw e6s — fetched directly from ledger, not backend query)
  let ckusdtReserve = 0;
  let ckusdcReserve = 0;

  const protocolPrincipal = Principal.fromText(CANISTER_IDS.PROTOCOL);

  // Preferred ckStable token
  type StableToken = 'auto' | 'ckUSDT' | 'ckUSDC';
  let preferredStable: StableToken = 'auto';

  // ── Wallet subscription ─────────────────────────────────────────────
  wallet.subscribe(state => {
    isConnected = state.isConnected;
    icusdBalance = state.tokenBalances?.ICUSD ? Number(state.tokenBalances.ICUSD.formatted) : 0;
  });

  // ── Computed values ─────────────────────────────────────────────────
  $: totalReserveUsd = (ckusdtReserve + ckusdcReserve) / 1e6;
  $: hasReserves = reserveRedemptionsEnabled && totalReserveUsd > 0.01;

  // How the redemption splits between reserves and vaults
  $: reserveFeeAmount = icusdAmount * reserveRedemptionFee;
  $: reserveNetAmount = icusdAmount - reserveFeeAmount;
  // Portion covered by reserves (capped at available)
  $: reservePortion = hasReserves ? Math.min(reserveNetAmount, totalReserveUsd) : 0;
  // Portion that spills over to vault redemption
  $: vaultSpilloverAmount = hasReserves
    ? Math.max(0, reserveNetAmount - totalReserveUsd)
    : icusdAmount; // if no reserves, everything goes to vaults
  $: hasVaultSpillover = vaultSpilloverAmount > 0.01;

  // Per-token breakdown of what user receives from reserves
  // Backend fills preferred token first, then spills to the other.
  // Amounts in human-readable units (e6s → divide by 1e6).
  $: reserveTokenBreakdown = computeReserveBreakdown(reservePortion, preferredStable);
  function computeReserveBreakdown(portion: number, pref: StableToken) {
    if (portion <= 0) return [];
    const usdtAvail = ckusdtReserve / 1e6;
    const usdcAvail = ckusdcReserve / 1e6;
    // Order tokens by preference (auto = ckUSDT first, like backend)
    const ordered = pref === 'ckUSDC'
      ? [{ sym: 'ckUSDC', avail: usdcAvail }, { sym: 'ckUSDT', avail: usdtAvail }]
      : [{ sym: 'ckUSDT', avail: usdtAvail }, { sym: 'ckUSDC', avail: usdcAvail }];
    let remaining = portion;
    const result: Array<{ symbol: string; amount: number }> = [];
    for (const t of ordered) {
      if (remaining <= 0) break;
      const take = Math.min(remaining, t.avail);
      if (take > 0.001) {
        result.push({ symbol: t.sym, amount: take });
        remaining -= take;
      }
    }
    return result;
  }

  // Reactively fetch the vault fee for the actual spillover amount (debounced)
  let feeDebounceTimer: ReturnType<typeof setTimeout> | null = null;
  let isFetchingFee = false;
  $: if (vaultSpilloverAmount > 0.01) {
    debouncedFetchVaultFee(vaultSpilloverAmount);
  }
  function debouncedFetchVaultFee(amount: number) {
    if (feeDebounceTimer) clearTimeout(feeDebounceTimer);
    feeDebounceTimer = setTimeout(async () => {
      isFetchingFee = true;
      try {
        const fees = await protocolService.getFees(amount);
        vaultRedemptionFee = fees.redemptionFee;
      } catch (e) {
        console.error('Error fetching vault fee:', e);
      } finally {
        isFetchingFee = false;
      }
    }, 300);
  }

  // ICP estimate for the vault portion
  $: vaultFeeOnSpillover = vaultSpilloverAmount * vaultRedemptionFee;
  $: icpFromVaults = hasVaultSpillover && icpPrice > 0
    ? (vaultSpilloverAmount - vaultFeeOnSpillover) / icpPrice : 0;

  // Display fee — flat reserve fee if fully covered by reserves, otherwise blended
  $: displayFee = hasReserves
    ? (hasVaultSpillover ? 'blended' : `${(reserveRedemptionFee * 100).toFixed(1)}%`)
    : `${(vaultRedemptionFee * 100).toFixed(2)}%`;

  // Preferred token principal for API call
  $: preferredTokenPrincipal = preferredStable === 'ckUSDT'
    ? CANISTER_IDS.CKUSDT_LEDGER
    : preferredStable === 'ckUSDC'
      ? CANISTER_IDS.CKUSDC_LEDGER
      : undefined;

  // ── Data fetching ───────────────────────────────────────────────────
  async function fetchData() {
    isLoading = true;
    try {
      const status = await protocolService.getProtocolStatus();

      icpPrice = status.lastIcpRate;
      // Initialize vault fee to 0 — the reactive $: block will fetch the
      // actual fee once the user enters an amount and we know the spillover.
      vaultRedemptionFee = 0;
      reserveRedemptionFee = status.reserveRedemptionFee || 0;
      reserveRedemptionsEnabled = status.reserveRedemptionsEnabled || false;

      // Fetch reserve balances directly from ledger canisters
      // (backend query can't do inter-canister calls, so it always returns 0)
      await fetchReserveBalances();

      if (isConnected) await wallet.refreshBalance();
    } catch (error) {
      console.error('Error fetching protocol data:', error);
    } finally {
      isLoading = false;
    }
  }

  async function fetchReserveBalances() {
    try {
      const [usdt, usdc] = await Promise.all([
        TokenService.getTokenBalance(CANISTER_IDS.CKUSDT_LEDGER, protocolPrincipal),
        TokenService.getTokenBalance(CANISTER_IDS.CKUSDC_LEDGER, protocolPrincipal),
      ]);
      ckusdtReserve = Number(usdt);
      ckusdcReserve = Number(usdc);
    } catch (e) {
      console.error('Reserve balance fetch error:', e);
    }
  }

  // ── Actions ─────────────────────────────────────────────────────────
  async function handleRedeem() {
    if (!isConnected) { errorMessage = 'Please connect your wallet first'; return; }
    if (icusdAmount <= 0) { errorMessage = 'Please enter a valid icUSD amount'; return; }
    if (icusdAmount > icusdBalance) { errorMessage = 'Insufficient icUSD balance'; return; }

    actionInProgress = true;
    errorMessage = '';
    successMessage = '';

    try {
      if (hasReserves) {
        // Use the unified reserve redemption endpoint (reserves first, vault spillover automatic)
        const result = await protocolService.redeemReserves(icusdAmount, preferredTokenPrincipal);
        if (result.success) {
          const stableReceived = (result.stableAmountSent || 0) / 1e6;
          let msg = `Redeemed ${formatNumber(stableReceived, 2)} stablecoins`;
          if (result.vaultSpillover && result.vaultSpillover > 0) {
            const spilloverIcusd = result.vaultSpillover / 1e8;
            msg += ` + ${formatNumber(spilloverIcusd, 2)} icUSD redeemed from vaults (ICP)`;
          }
          if (result.feePaid) {
            msg += ` (fee: ${formatNumber(result.feePaid, 4)} icUSD)`;
          }
          successMessage = msg;
          icusdAmount = 0;
          await wallet.refreshBalance();
          fetchData();
        } else {
          // Oisy two-step: approval succeeded, show as info not error
          const msg = result.error || 'Failed to redeem';
          if (msg.includes('Click Redeem again')) {
            successMessage = 'Approved! Click Redeem again to complete.';
          } else {
            errorMessage = msg;
          }
        }
      } else {
        // No reserves available — fall back to direct ICP vault redemption
        const result = await protocolService.redeemIcp(icusdAmount);
        if (result.success) {
          const icpEstimate = icusdAmount > 0 && icpPrice > 0
            ? (icusdAmount - icusdAmount * vaultRedemptionFee) / icpPrice : 0;
          successMessage = `Redeemed ~${formatNumber(icpEstimate)} ICP for ${formatNumber(icusdAmount)} icUSD`;
          icusdAmount = 0;
          await wallet.refreshBalance();
        } else {
          errorMessage = result.error || 'Failed to redeem ICP';
        }
      }
    } catch (error) {
      console.error('Error redeeming:', error);
      errorMessage = error instanceof Error ? error.message : 'An unexpected error occurred';
    } finally {
      actionInProgress = false;
    }
  }

  onMount(fetchData);
</script>

<svelte:head>
  <title>Rumi Protocol - Redeem icUSD</title>
</svelte:head>

<div class="page-container">
  <h1 class="page-title">Redeem icUSD</h1>

  <div class="page-layout">
    <!-- LEFT: Protocol stats sidebar -->
    <div class="stats-column">
      <ProtocolStats />
    </div>

    <!-- RIGHT: Action card -->
    <div class="action-column">
      <!-- Reserve balances indicator -->
      {#if hasReserves}
        <div class="reserve-bar">
          <span class="reserve-label">Protocol Reserves</span>
          <div class="reserve-amounts">
            {#if ckusdtReserve > 0}
              <span class="reserve-token">{formatNumber(ckusdtReserve / 1e6, 2)} ckUSDT</span>
            {/if}
            {#if ckusdtReserve > 0 && ckusdcReserve > 0}
              <span class="reserve-sep">|</span>
            {/if}
            {#if ckusdcReserve > 0}
              <span class="reserve-token">{formatNumber(ckusdcReserve / 1e6, 2)} ckUSDC</span>
            {/if}
            <span class="reserve-sep">|</span>
            <span class="reserve-total">${formatNumber(totalReserveUsd, 2)} total</span>
          </div>
        </div>
      {/if}

      <!-- Main redeem card -->
      <div class="action-card">
        <h2 class="card-heading">Redeem icUSD</h2>

        <div class="card-body">
          <!-- Amount input -->
          <div>
            <label for="icusd-amount" class="input-label">icUSD Amount</label>
            <div class="input-wrap">
              <input
                id="icusd-amount"
                type="number"
                bind:value={icusdAmount}
                min="0"
                step="0.01"
                class="amount-input"
                placeholder="0.00"
                disabled={actionInProgress || isLoading}
              />
              <div class="input-suffix">
                <span>icUSD</span>
              </div>
            </div>
            {#if isConnected && !isLoading && icusdBalance > 0}
              <div class="max-btn-row">
                <button
                  class="max-btn"
                  on:click={() => icusdAmount = icusdBalance}
                  disabled={actionInProgress}
                >
                  Max: {formatNumber(icusdBalance)}
                </button>
              </div>
            {/if}
          </div>

          <!-- Token preference (only when reserves are available) -->
          {#if hasReserves}
            <div>
              <label class="input-label">Preferred Token</label>
              <div class="token-selector">
                <button
                  class="token-btn" class:selected={preferredStable === 'auto'}
                  on:click={() => preferredStable = 'auto'}
                >Auto</button>
                <button
                  class="token-btn" class:selected={preferredStable === 'ckUSDT'}
                  on:click={() => preferredStable = 'ckUSDT'}
                >ckUSDT</button>
                <button
                  class="token-btn" class:selected={preferredStable === 'ckUSDC'}
                  on:click={() => preferredStable = 'ckUSDC'}
                >ckUSDC</button>
              </div>
            </div>
          {/if}

          <!-- Fee breakdown -->
          {#if icusdAmount > 0}
            <div class="fee-breakdown">
              {#if hasReserves}
                <div class="fee-row muted">
                  <span>Reserve fee ({(reserveRedemptionFee * 100).toFixed(1)}%):</span>
                  <span>{formatNumber(reserveFeeAmount, 4)} icUSD</span>
                </div>
                {#each reserveTokenBreakdown as token, i}
                  <div class="fee-row">
                    <span>From reserves{reserveTokenBreakdown.length > 1 ? ` (${i + 1}/${reserveTokenBreakdown.length})` : ''}:</span>
                    <span class="value-highlight">~{formatNumber(token.amount, 2)} {token.symbol}</span>
                  </div>
                {/each}
                {#if hasVaultSpillover}
                  <div class="fee-row spillover">
                    <span>Vault spillover ({(vaultRedemptionFee * 100).toFixed(2)}% fee):</span>
                    <span>~{formatNumber(icpFromVaults, 4)} ICP</span>
                  </div>
                {/if}
              {:else}
                <div class="fee-row muted">
                  <span>Redemption fee ({(vaultRedemptionFee * 100).toFixed(2)}%):</span>
                  <span>{formatNumber(icusdAmount * vaultRedemptionFee, 4)} icUSD</span>
                </div>
                <div class="fee-row">
                  <span>You will receive:</span>
                  <span class="value-highlight">~{formatNumber(icpFromVaults, 4)} ICP</span>
                </div>
                <div class="fee-row price-row">
                  <span>Current ICP price:</span>
                  <span>${formatNumber(icpPrice)}</span>
                </div>
              {/if}
            </div>
          {/if}

          <!-- Messages -->
          {#if errorMessage}
            <div class="msg msg-error">{errorMessage}</div>
          {/if}
          {#if successMessage}
            <div class="msg msg-success">{successMessage}</div>
          {/if}

          <!-- Submit -->
          <button
            class="submit-btn"
            on:click={handleRedeem}
            disabled={actionInProgress || !isConnected || icusdAmount <= 0 || icusdAmount > icusdBalance || isLoading}
          >
            {#if !isConnected}
              Connect Wallet to Continue
            {:else if actionInProgress}
              Processing Redemption...
            {:else if isLoading}
              Loading...
            {:else}
              Redeem icUSD
            {/if}
          </button>
        </div>
      </div>

      <!-- How it works (collapsed under the action card) -->
      <details class="how-it-works">
        <summary class="how-heading">How Redemption Works</summary>
        <div class="how-body">
          <ol class="how-steps">
            <li>
              <strong>Burn icUSD</strong>
              <p>Your icUSD is burned, reducing total supply and strengthening the peg.</p>
            </li>
            {#if hasReserves}
              <li>
                <strong>Receive from reserves</strong>
                <p>You receive stablecoins from the protocol's reserves at a flat {(reserveRedemptionFee * 100).toFixed(1)}% fee. Fees go to the treasury.</p>
              </li>
              <li>
                <strong>Vault spillover (if needed)</strong>
                <p>If reserves don't fully cover your redemption, the remainder is automatically redeemed from vaults — you receive ICP at the dynamic fee rate.</p>
              </li>
            {:else}
              <li>
                <strong>Pay dynamic fee</strong>
                <p>A dynamic fee ({(vaultRedemptionFee * 100).toFixed(2)}%) is charged, increasing with volume to protect vault owners.</p>
              </li>
              <li>
                <strong>Receive ICP</strong>
                <p>ICP is taken from the lowest-CR vaults using a water-filling algorithm that spreads the redemption evenly.</p>
              </li>
            {/if}
          </ol>
          <div class="how-note">
            <p>
              Redemptions create a hard price floor for icUSD — every 1 icUSD can always be exchanged for $1 of assets, minus a small fee.
              {#if hasReserves}
                Protocol reserves are used first at the lowest fee; vault collateral is only tapped when reserves are exhausted.
              {/if}
            </p>
          </div>
        </div>
      </details>
    </div>
  </div>
</div>

<style>
  /* ── Page layout ───────────────────────────────────────────────── */
  .page-container {
    max-width: 820px;
    margin: 0 auto;
    padding: 0 1rem;
  }
  .page-title {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 1.75rem;
    font-weight: 700;
    color: var(--rumi-text-primary);
    margin-bottom: 1.5rem;
  }
  .page-layout {
    display: grid;
    grid-template-columns: 280px 1fr;
    gap: 1.5rem;
    align-items: start;
  }
  .stats-column {
    position: sticky;
    top: 5rem;
  }
  .action-column {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  /* ── Reserve bar ───────────────────────────────────────────────── */
  .reserve-bar {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 0.625rem 1rem;
    border-radius: 0.5rem;
    background: rgba(16, 185, 129, 0.06);
    border: 1px solid rgba(16, 185, 129, 0.15);
  }
  .reserve-label {
    font-size: 0.75rem;
    font-weight: 500;
    color: #6ee7b7;
  }
  .reserve-amounts {
    display: flex;
    gap: 0.5rem;
    align-items: center;
    font-size: 0.75rem;
  }
  .reserve-token {
    font-variant-numeric: tabular-nums;
    color: #d1d5db;
  }
  .reserve-sep {
    color: #4b5563;
  }
  .reserve-total {
    font-weight: 600;
    color: #6ee7b7;
  }

  /* ── Action card ───────────────────────────────────────────────── */
  .action-card {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    padding: 1.5rem;
  }
  .card-heading {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 1.25rem;
    font-weight: 600;
    color: var(--rumi-text-primary);
    margin-bottom: 1.25rem;
  }
  .card-body {
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }

  /* ── Inputs ────────────────────────────────────────────────────── */
  .input-label {
    display: block;
    font-size: 0.75rem;
    font-weight: 500;
    color: var(--rumi-text-secondary);
    margin-bottom: 0.375rem;
  }
  .input-wrap {
    position: relative;
  }
  .amount-input {
    width: 100%;
    background: var(--rumi-bg-surface2);
    border: 1px solid var(--rumi-border);
    border-radius: 0.5rem;
    padding: 0.625rem 3.5rem 0.625rem 0.75rem;
    font-size: 0.9375rem;
    font-variant-numeric: tabular-nums;
    color: var(--rumi-text-primary);
    outline: none;
    transition: border-color 0.15s ease;
  }
  .amount-input:focus {
    border-color: rgba(139, 92, 246, 0.5);
  }
  .amount-input:disabled {
    opacity: 0.5;
  }
  .input-suffix {
    position: absolute;
    inset: 0 0 0 auto;
    display: flex;
    align-items: center;
    padding-right: 0.75rem;
    pointer-events: none;
    font-size: 0.8125rem;
    color: var(--rumi-text-muted);
  }
  .max-btn-row {
    text-align: right;
    margin-top: 0.25rem;
  }
  .max-btn {
    font-size: 0.6875rem;
    color: #60a5fa;
    cursor: pointer;
    background: none;
    border: none;
    padding: 0;
  }
  .max-btn:hover {
    color: #93bbfd;
  }

  /* Hide number input spinners */
  .amount-input::-webkit-outer-spin-button,
  .amount-input::-webkit-inner-spin-button {
    -webkit-appearance: none;
    margin: 0;
  }
  .amount-input {
    -moz-appearance: textfield;
  }

  /* ── Token selector ────────────────────────────────────────────── */
  .token-selector {
    display: flex;
    gap: 0.5rem;
  }
  .token-btn {
    flex: 1;
    padding: 0.4375rem 0.75rem;
    border-radius: 0.5rem;
    border: 1px solid rgba(107, 114, 128, 0.3);
    background: rgba(31, 41, 55, 0.4);
    color: #9ca3af;
    font-size: 0.8125rem;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
  }
  .token-btn:hover {
    border-color: rgba(139, 92, 246, 0.4);
  }
  .token-btn.selected {
    border-color: rgba(139, 92, 246, 0.6);
    background: rgba(139, 92, 246, 0.12);
    color: #e5e7eb;
  }

  /* ── Fee breakdown ─────────────────────────────────────────────── */
  .fee-breakdown {
    padding: 0.625rem 0.75rem;
    background: var(--rumi-bg-surface2);
    border-radius: 0.5rem;
    border: 1px solid var(--rumi-border);
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }
  .fee-row {
    display: flex;
    justify-content: space-between;
    font-size: 0.75rem;
    color: var(--rumi-text-secondary);
  }
  .fee-row.muted {
    color: var(--rumi-text-muted);
  }
  .fee-row.spillover {
    color: #fbbf24;
    margin-top: 0.25rem;
  }
  .fee-row.price-row {
    color: var(--rumi-text-muted);
    font-size: 0.6875rem;
    margin-top: 0.25rem;
  }
  .value-highlight {
    font-weight: 600;
    color: var(--rumi-text-primary);
  }

  /* ── Messages ──────────────────────────────────────────────────── */
  .msg {
    padding: 0.625rem 0.75rem;
    border-radius: 0.5rem;
    font-size: 0.8125rem;
  }
  .msg-error {
    background: rgba(239, 68, 68, 0.1);
    border: 1px solid rgba(239, 68, 68, 0.25);
    color: #fca5a5;
  }
  .msg-success {
    background: rgba(16, 185, 129, 0.1);
    border: 1px solid rgba(16, 185, 129, 0.25);
    color: #6ee7b7;
  }

  /* ── Submit button ─────────────────────────────────────────────── */
  .submit-btn {
    width: 100%;
    padding: 0.625rem 1rem;
    border-radius: 0.5rem;
    font-size: 0.875rem;
    font-weight: 600;
    color: #fff;
    background: var(--rumi-accent, #8b5cf6);
    border: none;
    cursor: pointer;
    transition: opacity 0.15s ease;
  }
  .submit-btn:hover:not(:disabled) {
    opacity: 0.9;
  }
  .submit-btn:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }

  /* ── How it works ──────────────────────────────────────────────── */
  .how-it-works {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    overflow: hidden;
  }
  .how-heading {
    padding: 0.75rem 1rem;
    font-size: 0.8125rem;
    font-weight: 600;
    color: var(--rumi-text-secondary);
    cursor: pointer;
    list-style: none;
  }
  .how-heading::-webkit-details-marker { display: none; }
  .how-heading::before {
    content: '▸ ';
    font-size: 0.6875rem;
  }
  .how-it-works[open] .how-heading::before { content: '▾ '; }
  .how-body {
    padding: 0 1rem 1rem;
  }
  .how-steps {
    list-style: none;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    counter-reset: step;
  }
  .how-steps li {
    counter-increment: step;
    padding-left: 2rem;
    position: relative;
  }
  .how-steps li::before {
    content: counter(step);
    position: absolute;
    left: 0;
    top: 0;
    width: 1.375rem;
    height: 1.375rem;
    border-radius: 50%;
    background: rgba(139, 92, 246, 0.2);
    color: #c4b5fd;
    font-size: 0.6875rem;
    font-weight: 600;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .how-steps strong {
    font-size: 0.8125rem;
    color: var(--rumi-text-primary);
  }
  .how-steps p {
    font-size: 0.75rem;
    color: var(--rumi-text-secondary);
    margin: 0.125rem 0 0;
    line-height: 1.4;
  }
  .how-note {
    margin-top: 0.75rem;
    padding: 0.625rem 0.75rem;
    background: var(--rumi-bg-surface2);
    border-radius: 0.5rem;
  }
  .how-note p {
    font-size: 0.6875rem;
    color: var(--rumi-text-muted);
    line-height: 1.5;
    margin: 0;
  }

  /* ── Responsive ────────────────────────────────────────────────── */
  @media (max-width: 768px) {
    .page-layout {
      grid-template-columns: 1fr;
    }
    .stats-column {
      position: static;
      order: 2;
    }
    .action-column {
      order: 1;
    }
  }
</style>
