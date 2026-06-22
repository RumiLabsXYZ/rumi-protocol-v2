<script lang="ts">
  /**
   * Native-XRP collateral panel (P5).
   *
   * Drives the XRP-specific parts of the CDP lifecycle that the generic vault UI
   * doesn't cover:
   *   - open an XRP vault and show the per-vault XRPL custody address to fund,
   *   - confirm a deposit once the user has sent XRP to that address,
   *   - list outstanding XRP claims (withdraw / close / liquidation payouts) and
   *     settle each to an XRPL destination address.
   *
   * Once a deposit is confirmed the vault is a normal CDP vault, so borrow / repay /
   * margin / partial-withdraw happen through the existing VaultCard. This panel only
   * owns the deposit-in and claim-out edges.
   *
   * NOTE: the rail is gated behind the backend `register_xrp_collateral` switch and an
   * independent audit — this panel is inert until XRP collateral is registered.
   */
  import { onMount } from 'svelte';
  import {
    XrpVaultService,
    type XrpPendingDepositView,
    type XrpClaimView,
  } from '$lib/services/xrpVaultService';
  import { walletStore } from '$lib/stores/wallet';
  import { toastStore } from '$lib/stores/toast';
  import { formatAddress } from '$lib/utils/format';

  let pending: XrpPendingDepositView[] = [];
  let claims: XrpClaimView[] = [];
  let loading = false;
  let opening = false;
  let busyVaultId: number | null = null;
  let busyClaimId: number | null = null;
  // Per-claim destination address inputs, keyed by claim id.
  let claimDest: Record<number, string> = {};

  $: connected = $walletStore.isConnected;

  async function refresh() {
    if (!connected) {
      pending = [];
      claims = [];
      return;
    }
    loading = true;
    try {
      [pending, claims] = await Promise.all([
        XrpVaultService.getMyPendingDeposits(),
        XrpVaultService.getMyClaims(),
      ]);
    } finally {
      loading = false;
    }
  }

  onMount(refresh);
  // Reload when the wallet connects/disconnects.
  $: if (connected !== undefined) refresh();

  async function openVault() {
    opening = true;
    try {
      const res = await XrpVaultService.openXrpVault();
      if (res.success && res.data) {
        toastStore.success(`XRP vault #${res.data.vaultId} opened — send XRP to the custody address`);
        await refresh();
      } else {
        toastStore.error(res.error ?? 'Could not open XRP vault');
      }
    } finally {
      opening = false;
    }
  }

  async function confirmDeposit(vaultId: number) {
    busyVaultId = vaultId;
    try {
      const res = await XrpVaultService.confirmXrpDeposit(vaultId);
      if (res.success) {
        const xrp = res.data ? Number(res.data.creditedDrops) / 1_000_000 : 0;
        toastStore.success(xrp > 0 ? `Deposit confirmed — credited ${xrp} XRP` : 'Deposit confirmed');
        await refresh();
      } else {
        toastStore.error(res.error ?? 'Deposit not found yet — send XRP first, then retry');
      }
    } finally {
      busyVaultId = null;
    }
  }

  async function settleClaim(claim: XrpClaimView) {
    const dest = (claimDest[claim.claimId] ?? '').trim();
    if (!dest.startsWith('r') || dest.length < 25) {
      toastStore.error('Enter a valid XRPL destination address (starts with r)');
      return;
    }
    busyClaimId = claim.claimId;
    try {
      const res = await XrpVaultService.settleXrpClaim(claim.claimId, dest);
      if (res.success) {
        toastStore.success('Settlement submitted — confirming on the XRP Ledger…');
        // The claim clears once the Payment validates; poll a couple of times.
        await refresh();
        setTimeout(refresh, 4000);
      } else {
        toastStore.error(res.error ?? 'Could not settle claim');
      }
    } finally {
      busyClaimId = null;
    }
  }

  function copy(text: string) {
    navigator.clipboard?.writeText(text).then(
      () => toastStore.info('Copied'),
      () => {}
    );
  }
</script>

<section class="xrp-panel">
  <header class="xrp-head">
    <h3>Native XRP</h3>
    <button class="xrp-open" disabled={!connected || opening} on:click={openVault}>
      {opening ? 'Opening…' : 'Open XRP vault'}
    </button>
  </header>

  {#if !connected}
    <p class="xrp-muted">Connect your wallet to use XRP collateral.</p>
  {:else}
    {#if pending.length > 0}
      <div class="xrp-group">
        <div class="xrp-group-title">Awaiting deposit</div>
        {#each pending as p (p.vaultId)}
          <div class="xrp-row">
            <div class="xrp-row-main">
              <span class="xrp-label">Vault #{p.vaultId}</span>
              <button class="xrp-addr" title={p.custodyAddress} on:click={() => copy(p.custodyAddress)}>
                {formatAddress(p.custodyAddress, 8, 6)} ⧉
              </button>
              <span class="xrp-hint">Send XRP to this address, then confirm.</span>
            </div>
            <button
              class="xrp-action"
              disabled={busyVaultId === p.vaultId}
              on:click={() => confirmDeposit(p.vaultId)}
            >
              {busyVaultId === p.vaultId ? 'Checking…' : "I've sent it — confirm"}
            </button>
          </div>
        {/each}
      </div>
    {/if}

    {#if claims.length > 0}
      <div class="xrp-group">
        <div class="xrp-group-title">XRP claims to settle</div>
        {#each claims as c (c.claimId)}
          <div class="xrp-row">
            <div class="xrp-row-main">
              <span class="xrp-label">Claim #{c.claimId}</span>
              <span class="xrp-amt">{c.xrp} XRP</span>
              {#if c.inFlight}
                <span class="xrp-hint">Settlement in flight — confirming on-ledger.</span>
              {/if}
              <input
                class="xrp-input"
                placeholder="Your XRPL address (r…)"
                bind:value={claimDest[c.claimId]}
                spellcheck="false"
                autocomplete="off"
              />
            </div>
            <button class="xrp-action" disabled={busyClaimId === c.claimId} on:click={() => settleClaim(c)}>
              {busyClaimId === c.claimId ? 'Settling…' : c.inFlight ? 'Confirm' : 'Settle'}
            </button>
          </div>
        {/each}
      </div>
    {/if}

    {#if !loading && pending.length === 0 && claims.length === 0}
      <p class="xrp-muted">No pending XRP deposits or claims.</p>
    {/if}
  {/if}
</section>

<style>
  .xrp-panel {
    display: flex;
    flex-direction: column;
    gap: 14px;
    padding: 16px;
    border: 1px solid var(--rumi-border, rgba(148, 163, 184, 0.2));
    border-radius: 14px;
    background: var(--rumi-surface, rgba(15, 23, 42, 0.4));
  }
  .xrp-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .xrp-head h3 {
    margin: 0;
    font-size: 1rem;
    color: var(--rumi-text, #e2e8f0);
  }
  .xrp-open {
    padding: 7px 14px;
    border-radius: 10px;
    border: none;
    background: var(--rumi-accent, #14b8a6);
    color: #04211d;
    font-weight: 600;
    cursor: pointer;
  }
  .xrp-open:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .xrp-group {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .xrp-group-title {
    font-size: 0.78rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--rumi-text-muted, #94a3b8);
  }
  .xrp-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 10px 12px;
    border-radius: 10px;
    background: var(--rumi-surface-2, rgba(30, 41, 59, 0.5));
  }
  .xrp-row-main {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
  }
  .xrp-label {
    font-weight: 600;
    color: var(--rumi-text, #e2e8f0);
  }
  .xrp-amt {
    color: var(--rumi-accent, #2dd4bf);
    font-variant-numeric: tabular-nums;
  }
  .xrp-addr {
    background: none;
    border: none;
    padding: 0;
    color: var(--rumi-text-secondary, #cbd5e1);
    font-family: ui-monospace, monospace;
    cursor: pointer;
    text-align: left;
  }
  .xrp-hint,
  .xrp-muted {
    font-size: 0.8rem;
    color: var(--rumi-text-muted, #94a3b8);
  }
  .xrp-input {
    margin-top: 4px;
    padding: 6px 9px;
    border-radius: 8px;
    border: 1px solid var(--rumi-border, rgba(148, 163, 184, 0.25));
    background: var(--rumi-bg, rgba(2, 6, 23, 0.6));
    color: var(--rumi-text, #e2e8f0);
    font-family: ui-monospace, monospace;
    font-size: 0.82rem;
    min-width: 240px;
  }
  .xrp-action {
    padding: 7px 13px;
    border-radius: 10px;
    border: 1px solid var(--rumi-accent, #14b8a6);
    background: transparent;
    color: var(--rumi-accent, #2dd4bf);
    font-weight: 600;
    cursor: pointer;
    white-space: nowrap;
  }
  .xrp-action:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
