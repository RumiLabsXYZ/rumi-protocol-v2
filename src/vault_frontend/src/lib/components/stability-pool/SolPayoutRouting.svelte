<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import {
    stabilityPoolService,
    formatTokenAmount,
    type CollateralInfo,
    type UserPosition,
  } from '../../services/stabilityPoolService';
  import { SolVaultService } from '../../services/solVaultService';
  import type { NativeSolPendingPayout } from '../../services/stabilityPoolNativeSol';
  import { unwrapNativePayoutAddresses } from '../../services/xrpPayoutHelpers';
  import {
    SOL_NATIVE_PRINCIPAL_TEXT,
    isNativeSolPrincipal,
    validateSolPayoutInput,
  } from '../../services/solPayoutHelpers';

  export let collateralRegistry: CollateralInfo[] = [];
  export let userPosition: UserPosition | null = null;
  export let isConnected = false;

  const dispatch = createEventDispatcher<{ success: { action: string } }>();

  let payoutAddress = '';
  let saving = false;
  let loadingPayouts = false;
  let settlingClaimId: string | null = null;
  let error = '';
  let info = '';
  let pendingPayouts: NativeSolPendingPayout[] = [];
  let lastLoadedPosition: UserPosition | null = null;

  function errorMessage(err: unknown, fallback: string): string {
    return err instanceof Error ? err.message : fallback;
  }

  $: solCollateral = collateralRegistry.find((collateral) => isNativeSolPrincipal(collateral.ledger_id));
  $: nativePayoutByCollateral = unwrapNativePayoutAddresses(userPosition);
  $: storedAddress = nativePayoutByCollateral.get(solCollateral?.ledger_id.toText() ?? SOL_NATIVE_PRINCIPAL_TEXT) ?? '';
  $: userHasStablecoinDeposit = (userPosition?.stablecoin_balances ?? []).some(([, amount]) => amount > 0n);
  $: isEnabled = storedAddress !== '';
  $: hasPendingPayouts = pendingPayouts.length > 0;
  $: shouldRenderSolRouting =
    isConnected && userPosition && solCollateral && (userHasStablecoinDeposit || isEnabled || loadingPayouts || hasPendingPayouts);

  $: if (storedAddress && payoutAddress === '') {
    payoutAddress = storedAddress;
  }

  async function loadPendingPayouts() {
    if (!isConnected || !userPosition) {
      pendingPayouts = [];
      return;
    }
    loadingPayouts = true;
    try {
      pendingPayouts = await stabilityPoolService.getMyNativeSolPayouts();
    } catch (err: unknown) {
      // Older SP canisters do not expose this method yet. Keep the card usable
      // for opt-in while surfacing real errors from regenerated canisters.
      const message = errorMessage(err, String(err));
      if (!message.includes('not available')) {
        error = message || 'Could not load pending SOL payouts';
      }
    } finally {
      loadingPayouts = false;
    }
  }

  $: if (!isConnected) {
    lastLoadedPosition = null;
    pendingPayouts = [];
  }

  $: if (isConnected && userPosition && lastLoadedPosition !== userPosition) {
    lastLoadedPosition = userPosition;
    void loadPendingPayouts();
  }

  async function saveOptIn() {
    if (!solCollateral) return;
    const validation = validateSolPayoutInput(payoutAddress);
    if (!validation.ok) {
      error = validation.error ?? 'Check the SOL payout address';
      return;
    }

    saving = true;
    error = '';
    info = '';
    try {
      await stabilityPoolService.optInNativeSolCollateral(solCollateral.ledger_id, validation.address ?? '');
      info = 'SOL routing saved for Stability Pool liquidations.';
      dispatch('success', { action: 'solOptIn' });
    } catch (err: unknown) {
      error = errorMessage(err, 'SOL opt-in failed');
    } finally {
      saving = false;
    }
  }

  async function disableOptIn() {
    if (!solCollateral) return;
    saving = true;
    error = '';
    info = '';
    try {
      await stabilityPoolService.optOutCollateral(solCollateral.ledger_id);
      payoutAddress = '';
      info = 'SOL routing disabled.';
      dispatch('success', { action: 'solOptOut' });
    } catch (err: unknown) {
      error = errorMessage(err, 'SOL opt-out failed');
    } finally {
      saving = false;
    }
  }

  async function settlePendingPayout(payout: NativeSolPendingPayout) {
    const claimId = payout.claim_id.toString();
    settlingClaimId = claimId;
    error = '';
    info = '';
    try {
      const result = await SolVaultService.settleSolClaim(claimId, payout.payout_address);
      if (!result.success) {
        error = `SOL claim #${claimId} remains outstanding. Retry settlement when the network is available.`;
        return;
      }
      const claimOutstanding = await SolVaultService.hasOutstandingClaim(claimId);
      if (claimOutstanding) {
        info = result.data?.signature
          ? `SOL settlement submitted for claim #${claimId}. Signature: ${result.data.signature}. Retry once it confirms to clear this reminder.`
          : `SOL settlement submitted for claim #${claimId}. Retry once it confirms to clear this reminder.`;
        return;
      }
      await stabilityPoolService.ackNativeSolPayoutSettled(claimId);
      info = result.data?.signature
        ? `SOL settlement confirmed for claim #${claimId}. Signature: ${result.data.signature}.`
        : `SOL settlement confirmed for claim #${claimId}.`;
      pendingPayouts = pendingPayouts.filter((row) => row.claim_id !== payout.claim_id);
      dispatch('success', { action: 'solPayoutSettled' });
    } catch (err: unknown) {
      error = errorMessage(err, `SOL claim #${claimId} remains outstanding. Retry settlement from this card.`);
    } finally {
      settlingClaimId = null;
    }
  }
</script>

{#if shouldRenderSolRouting}
  <div class="sol-routing">
    <div class="sol-routing-head">
      <span class="sol-title">SOL payouts</span>
      <span class="sol-status" class:enabled={isEnabled}>{isEnabled ? 'Enabled' : 'Not enabled'}</span>
    </div>
    {#if userHasStablecoinDeposit || isEnabled}
      <p class="sol-copy">Provide a Solana payout address to receive SOL from SP liquidations.</p>

      <div class="sol-inputs">
        <input
          class="sol-address-input"
          type="text"
          inputmode="text"
          placeholder="Solana payout address"
          bind:value={payoutAddress}
          disabled={saving}
        />
      </div>

      <div class="sol-actions">
        <button class="sol-action" on:click={saveOptIn} disabled={saving}>
          {saving ? 'Saving…' : isEnabled ? 'Update' : 'Enable SOL'}
        </button>
        {#if isEnabled}
          <button class="sol-action secondary" on:click={disableOptIn} disabled={saving}>Disable</button>
        {/if}
      </div>
    {/if}

    {#if loadingPayouts}
      <div class="sol-note">Checking pending SOL payouts…</div>
    {:else if pendingPayouts.length > 0}
      <div class="sol-pending-list">
        {#each pendingPayouts as payout (payout.claim_id.toString())}
          {@const claimId = payout.claim_id.toString()}
          <div class="sol-pending-row">
            <span class="sol-pending-main">
              Claim #{claimId}
              <span class="sol-pending-sub">
                {formatTokenAmount(payout.lamports, 9, 9)} SOL to {payout.payout_address}
              </span>
            </span>
            <button
              class="sol-action compact"
              on:click={() => settlePendingPayout(payout)}
              disabled={settlingClaimId !== null}
            >
              {settlingClaimId === claimId ? 'Settling…' : 'Settle'}
            </button>
          </div>
        {/each}
      </div>
    {/if}

    {#if error}<div class="sol-error">{error}</div>{/if}
    {#if info}<div class="sol-info">{info}</div>{/if}
  </div>
{/if}

<style>
  .sol-routing {
    margin-top: 0.75rem;
    padding-top: 0.75rem;
    border-top: 1px solid var(--rumi-border);
  }

  .sol-routing-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
    margin-bottom: 0.25rem;
  }

  .sol-title {
    font-size: 0.75rem;
    font-weight: 700;
    color: var(--rumi-text-primary);
  }

  .sol-status {
    font-size: 0.625rem;
    font-weight: 700;
    color: var(--rumi-text-muted);
  }

  .sol-status.enabled {
    color: #b48bff;
  }

  .sol-copy,
  .sol-note,
  .sol-pending-sub {
    font-size: 0.6875rem;
    line-height: 1.35;
    color: var(--rumi-text-muted);
  }

  .sol-copy {
    margin: 0 0 0.5rem;
  }

  .sol-inputs {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: 0.375rem;
  }

  .sol-address-input {
    min-width: 0;
    height: 2rem;
    padding: 0 0.5rem;
    background: var(--rumi-bg-surface2);
    border: 1px solid var(--rumi-border);
    border-radius: 0.375rem;
    color: var(--rumi-text-primary);
    font-size: 0.75rem;
  }

  .sol-actions {
    display: flex;
    gap: 0.375rem;
    margin-top: 0.5rem;
  }

  .sol-action {
    border: 1px solid rgba(153, 69, 255, 0.4);
    border-radius: 0.375rem;
    background: rgba(153, 69, 255, 0.12);
    color: #b48bff;
    font-size: 0.6875rem;
    font-weight: 700;
    padding: 0.375rem 0.5rem;
    cursor: pointer;
  }

  .sol-action.secondary {
    border-color: var(--rumi-border);
    background: transparent;
    color: var(--rumi-text-secondary);
  }

  .sol-action.compact {
    padding: 0.25rem 0.45rem;
    flex-shrink: 0;
  }

  .sol-action:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .sol-pending-list {
    display: flex;
    flex-direction: column;
    gap: 0.375rem;
    margin-top: 0.625rem;
  }

  .sol-pending-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
    padding: 0.5rem;
    background: var(--rumi-bg-surface2);
    border: 1px solid var(--rumi-border);
    border-radius: 0.375rem;
  }

  .sol-pending-main {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 0.125rem;
    font-size: 0.75rem;
    color: var(--rumi-text-primary);
  }

  .sol-pending-sub {
    overflow-wrap: anywhere;
  }

  .sol-error,
  .sol-info {
    margin-top: 0.5rem;
    padding: 0.4375rem 0.5rem;
    border-radius: 0.375rem;
    font-size: 0.6875rem;
    line-height: 1.35;
  }

  .sol-error {
    background: rgba(224, 107, 159, 0.08);
    border: 1px solid rgba(224, 107, 159, 0.2);
    color: var(--rumi-danger);
  }

  .sol-info {
    background: rgba(153, 69, 255, 0.08);
    border: 1px solid rgba(153, 69, 255, 0.2);
    color: #b48bff;
  }
</style>
