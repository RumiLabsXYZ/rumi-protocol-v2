<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import {
    stabilityPoolService,
    formatTokenAmount,
    type CollateralInfo,
    type UserPosition,
  } from '../../services/stabilityPoolService';
  import { XrpVaultService } from '../../services/xrpVaultService';
  import type { NativeXrpPendingPayout } from '../../services/stabilityPoolNativeXrp';
  import {
    XRP_NATIVE_PRINCIPAL_TEXT,
    isNativeXrpPrincipal,
    unwrapNativePayoutAddresses,
    unwrapNativePayoutDestinationTags,
    validateXrpPayoutInput,
  } from '../../services/xrpPayoutHelpers';
  import { CANISTER_IDS } from '../../config';

  export let collateralRegistry: CollateralInfo[] = [];
  export let userPosition: UserPosition | null = null;
  export let isConnected = false;

  const dispatch = createEventDispatcher<{ success: { action: string } }>();

  let payoutAddress = '';
  let destinationTag = '';
  let saving = false;
  let loadingPayouts = false;
  let settlingClaimId: string | null = null;
  let error = '';
  let info = '';
  let pendingPayouts: NativeXrpPendingPayout[] = [];
  let lastLoadedPosition: UserPosition | null = null;

  function errorMessage(err: unknown, fallback: string): string {
    return err instanceof Error ? err.message : fallback;
  }

  $: xrpCollateral = collateralRegistry.find((collateral) => isNativeXrpPrincipal(collateral.ledger_id));
  $: nativePayoutByCollateral = unwrapNativePayoutAddresses(userPosition);
  $: nativeTagByCollateral = unwrapNativePayoutDestinationTags(userPosition);
  $: storedAddress = nativePayoutByCollateral.get(xrpCollateral?.ledger_id.toText() ?? XRP_NATIVE_PRINCIPAL_TEXT) ?? '';
  $: storedTag = nativeTagByCollateral.get(xrpCollateral?.ledger_id.toText() ?? XRP_NATIVE_PRINCIPAL_TEXT);
  $: userHasIcusd = (userPosition?.stablecoin_balances ?? []).some(
    ([ledger, amount]) => ledger.toText() === CANISTER_IDS.ICUSD_LEDGER && amount > 0n
  );
  $: isEnabled = storedAddress !== '';
  $: hasPendingPayouts = pendingPayouts.length > 0;
  $: shouldRenderXrpRouting =
    isConnected && userPosition && xrpCollateral && (userHasIcusd || isEnabled || loadingPayouts || hasPendingPayouts);

  $: if (storedAddress && payoutAddress === '') {
    payoutAddress = storedAddress;
  }
  $: if (storedTag !== undefined && destinationTag === '') {
    destinationTag = String(storedTag);
  }

  async function loadPendingPayouts() {
    if (!isConnected || !userPosition) {
      pendingPayouts = [];
      return;
    }
    loadingPayouts = true;
    try {
      pendingPayouts = await stabilityPoolService.getMyNativeXrpPayouts();
    } catch (err: unknown) {
      // Older SP canisters do not expose this method yet. Keep the card usable
      // for opt-in while surfacing real errors from regenerated canisters.
      const message = errorMessage(err, String(err));
      if (!message.includes('not available')) {
        error = message || 'Could not load pending XRP payouts';
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
    if (!xrpCollateral) return;
    const validation = validateXrpPayoutInput(payoutAddress, destinationTag);
    if (!validation.ok) {
      error = validation.error ?? 'Check the XRP payout details';
      return;
    }

    saving = true;
    error = '';
    info = '';
    try {
      await stabilityPoolService.optInNativeCollateralWithTag(
        xrpCollateral.ledger_id,
        validation.address ?? '',
        validation.destinationTag
      );
      info = 'XRP routing saved for Stability Pool liquidations.';
      dispatch('success', { action: 'xrpOptIn' });
    } catch (err: unknown) {
      error = errorMessage(err, 'XRP opt-in failed');
    } finally {
      saving = false;
    }
  }

  async function disableOptIn() {
    if (!xrpCollateral) return;
    saving = true;
    error = '';
    info = '';
    try {
      await stabilityPoolService.optOutCollateral(xrpCollateral.ledger_id);
      payoutAddress = '';
      destinationTag = '';
      info = 'XRP routing disabled.';
      dispatch('success', { action: 'xrpOptOut' });
    } catch (err: unknown) {
      error = errorMessage(err, 'XRP opt-out failed');
    } finally {
      saving = false;
    }
  }

  async function settlePendingPayout(payout: NativeXrpPendingPayout) {
    const claimId = payout.claim_id.toString();
    settlingClaimId = claimId;
    error = '';
    info = '';
    try {
      const tag = payout.destination_tag[0];
      const result = await XrpVaultService.settleXrpClaim(claimId, payout.payout_address, tag);
      if (!result.success) {
        error = `XRP claim #${claimId} remains outstanding. Retry settlement when the network is available.`;
        return;
      }
      const claimOutstanding = await XrpVaultService.hasOutstandingClaim(claimId);
      if (claimOutstanding) {
        info = result.data?.txHash
          ? `XRP settlement submitted for claim #${claimId}. Tx hash: ${result.data.txHash}. Retry once it validates to clear this reminder.`
          : `XRP settlement submitted for claim #${claimId}. Retry once it validates to clear this reminder.`;
        return;
      }
      await stabilityPoolService.ackNativeXrpPayoutSettled(claimId);
      info = result.data?.txHash
        ? `XRP settlement confirmed for claim #${claimId}. Tx hash: ${result.data.txHash}.`
        : `XRP settlement confirmed for claim #${claimId}.`;
      pendingPayouts = pendingPayouts.filter((row) => row.claim_id !== payout.claim_id);
      dispatch('success', { action: 'xrpPayoutSettled' });
    } catch (err: unknown) {
      error = errorMessage(err, `XRP claim #${claimId} remains outstanding. Retry settlement from this card.`);
    } finally {
      settlingClaimId = null;
    }
  }
</script>

{#if shouldRenderXrpRouting}
  <div class="xrp-routing">
    <div class="xrp-routing-head">
      <span class="xrp-title">XRP payouts</span>
      <span class="xrp-status" class:enabled={isEnabled}>{isEnabled ? 'Enabled' : 'Not enabled'}</span>
    </div>
    {#if userHasIcusd || isEnabled}
      <p class="xrp-copy">Provide an XRP payout address and optional tag to receive XRP from SP liquidations.</p>

      <div class="xrp-inputs">
        <input
          class="xrp-address-input"
          type="text"
          inputmode="text"
          placeholder="XRPL payout address"
          bind:value={payoutAddress}
          disabled={saving}
        />
        <input
          class="xrp-tag-input"
          type="text"
          inputmode="numeric"
          placeholder="Tag"
          bind:value={destinationTag}
          disabled={saving}
        />
      </div>

      <div class="xrp-actions">
        <button class="xrp-action" on:click={saveOptIn} disabled={saving}>
          {saving ? 'Saving…' : isEnabled ? 'Update' : 'Enable XRP'}
        </button>
        {#if isEnabled}
          <button class="xrp-action secondary" on:click={disableOptIn} disabled={saving}>Disable</button>
        {/if}
      </div>
    {/if}

    {#if loadingPayouts}
      <div class="xrp-note">Checking pending XRP payouts…</div>
    {:else if pendingPayouts.length > 0}
      <div class="xrp-pending-list">
        {#each pendingPayouts as payout (payout.claim_id.toString())}
          {@const claimId = payout.claim_id.toString()}
          <div class="xrp-pending-row">
            <span class="xrp-pending-main">
              Claim #{claimId}
              <span class="xrp-pending-sub">
                {formatTokenAmount(payout.drops, 6, 6)} XRP to {payout.payout_address}
                {#if payout.destination_tag[0] !== undefined}
                  · tag {payout.destination_tag[0]}
                {/if}
              </span>
            </span>
            <button
              class="xrp-action compact"
              on:click={() => settlePendingPayout(payout)}
              disabled={settlingClaimId !== null}
            >
              {settlingClaimId === claimId ? 'Settling…' : 'Settle'}
            </button>
          </div>
        {/each}
      </div>
    {/if}

    {#if error}<div class="xrp-error">{error}</div>{/if}
    {#if info}<div class="xrp-info">{info}</div>{/if}
  </div>
{/if}

<style>
  .xrp-routing {
    margin-top: 0.75rem;
    padding-top: 0.75rem;
    border-top: 1px solid var(--rumi-border);
  }

  .xrp-routing-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
    margin-bottom: 0.25rem;
  }

  .xrp-title {
    font-size: 0.75rem;
    font-weight: 700;
    color: var(--rumi-text-primary);
  }

  .xrp-status {
    font-size: 0.625rem;
    font-weight: 700;
    color: var(--rumi-text-muted);
  }

  .xrp-status.enabled {
    color: var(--rumi-teal);
  }

  .xrp-copy,
  .xrp-note,
  .xrp-pending-sub {
    font-size: 0.6875rem;
    line-height: 1.35;
    color: var(--rumi-text-muted);
  }

  .xrp-copy {
    margin: 0 0 0.5rem;
  }

  .xrp-inputs {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 4.75rem;
    gap: 0.375rem;
  }

  .xrp-address-input,
  .xrp-tag-input {
    min-width: 0;
    height: 2rem;
    padding: 0 0.5rem;
    background: var(--rumi-bg-surface2);
    border: 1px solid var(--rumi-border);
    border-radius: 0.375rem;
    color: var(--rumi-text-primary);
    font-size: 0.75rem;
  }

  .xrp-actions {
    display: flex;
    gap: 0.375rem;
    margin-top: 0.5rem;
  }

  .xrp-action {
    border: 1px solid var(--rumi-border-teal);
    border-radius: 0.375rem;
    background: var(--rumi-teal-dim);
    color: var(--rumi-teal);
    font-size: 0.6875rem;
    font-weight: 700;
    padding: 0.375rem 0.5rem;
    cursor: pointer;
  }

  .xrp-action.secondary {
    border-color: var(--rumi-border);
    background: transparent;
    color: var(--rumi-text-secondary);
  }

  .xrp-action.compact {
    padding: 0.25rem 0.45rem;
    flex-shrink: 0;
  }

  .xrp-action:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .xrp-pending-list {
    display: flex;
    flex-direction: column;
    gap: 0.375rem;
    margin-top: 0.625rem;
  }

  .xrp-pending-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
    padding: 0.5rem;
    background: var(--rumi-bg-surface2);
    border: 1px solid var(--rumi-border);
    border-radius: 0.375rem;
  }

  .xrp-pending-main {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 0.125rem;
    font-size: 0.75rem;
    color: var(--rumi-text-primary);
  }

  .xrp-pending-sub {
    overflow-wrap: anywhere;
  }

  .xrp-error,
  .xrp-info {
    margin-top: 0.5rem;
    padding: 0.4375rem 0.5rem;
    border-radius: 0.375rem;
    font-size: 0.6875rem;
    line-height: 1.35;
  }

  .xrp-error {
    background: rgba(224, 107, 159, 0.08);
    border: 1px solid rgba(224, 107, 159, 0.2);
    color: var(--rumi-danger);
  }

  .xrp-info {
    background: rgba(45, 212, 191, 0.08);
    border: 1px solid rgba(45, 212, 191, 0.18);
    color: var(--rumi-teal);
  }
</style>
