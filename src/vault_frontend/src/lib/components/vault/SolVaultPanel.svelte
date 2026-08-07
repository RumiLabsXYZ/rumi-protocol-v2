<script lang="ts">
  /**
   * Native-SOL collateral panel (P5).
   *
   * Drives the SOL-specific parts of the CDP lifecycle that the generic vault UI
   * doesn't cover:
   *   - open a SOL vault and show the per-vault Solana custody address to fund,
   *   - confirm a deposit once the user has sent SOL to that address,
   *   - list outstanding SOL claims (withdraw / close / liquidation payouts) and
   *     settle each to a Solana destination address.
   *
   * Once a deposit is confirmed the vault is a normal CDP vault, so borrow / repay /
   * margin / partial-withdraw happen through the existing VaultCard. This panel only
   * owns the deposit-in and claim-out edges. Mirrors XrpVaultPanel.svelte; the only
   * structural difference is there is no destination tag anywhere.
   *
   * NOTE: the rail is gated behind the backend `register_sol_collateral` switch and an
   * independent audit; this panel is inert until SOL collateral is registered.
   */
  import { onMount, createEventDispatcher } from 'svelte';
  import {
    SolVaultService,
    type SolPendingDepositView,
    type SolClaimView,
  } from '$lib/services/solVaultService';
  import { isPlausibleSolAddress } from '$lib/services/solPayoutHelpers';
  import { walletStore } from '$lib/stores/wallet';
  import { toastStore } from '$lib/stores/toast';
  import { formatAddress } from '$lib/utils/format';

  // Notifies the vaults page to reload the main vault list once a deposit confirms
  // (the confirmed vault becomes a normal VaultCard, which this panel doesn't own).
  const dispatch = createEventDispatcher<{ confirmed: void }>();

  let pending: SolPendingDepositView[] = [];
  let hiddenPending: SolPendingDepositView[] = [];
  let claims: SolClaimView[] = [];
  let loading = false;
  let opening = false;
  let checkingWallet = false;
  let busyVaultId: number | null = null;
  let busyClaimId: string | null = null;
  let lastConnected = false;
  // Per-claim destination address inputs, keyed by claim id.
  let claimDest: Record<string, string> = {};

  $: connected = $walletStore.isConnected;

  function refreshHiddenPending() {
    hiddenPending = connected ? SolVaultService.getHiddenPendingDeposits() : [];
  }

  async function refresh(options: { allowSigner?: boolean } = {}) {
    if (!connected) {
      pending = [];
      hiddenPending = [];
      claims = [];
      return;
    }
    loading = true;
    try {
      if (options.allowSigner) {
        // Oisy signer requests must be serialized; parallel calls can leave the
        // wallet popup in a busy/retry loop.
        pending = await SolVaultService.getMyPendingDeposits(options);
        claims = await SolVaultService.getMyClaims(options);
      } else {
        [pending, claims] = await Promise.all([
          SolVaultService.getMyPendingDeposits(options),
          SolVaultService.getMyClaims(options),
        ]);
      }
    } finally {
      refreshHiddenPending();
      loading = false;
    }
  }

  async function checkWalletStatus() {
    checkingWallet = true;
    loading = true;
    try {
      pending = await SolVaultService.getMyPendingDeposits({ allowSigner: true });
      refreshHiddenPending();
    } finally {
      loading = false;
      checkingWallet = false;
    }
  }

  function hidePendingDeposit(vaultId: number) {
    SolVaultService.hidePendingDeposit(vaultId);
    pending = pending.filter((p) => p.vaultId !== vaultId);
    refreshHiddenPending();
    toastStore.info('Hidden from this browser. Use Restore if you need it again.');
  }

  function restorePendingDeposit(vaultId: number) {
    SolVaultService.restorePendingDeposit(vaultId);
    const restored = hiddenPending.find((p) => p.vaultId === vaultId);
    if (restored) pending = [...pending, restored].sort((a, b) => a.vaultId - b.vaultId);
    refreshHiddenPending();
  }

  onMount(() => {
    lastConnected = connected;
    refresh();
  });
  // Reload when the wallet connects/disconnects.
  $: if (connected !== lastConnected) {
    lastConnected = connected;
    refresh();
  }

  async function openVault() {
    opening = true;
    try {
      const res = await SolVaultService.openSolVault();
      if (res.success && res.data) {
        toastStore.success(`SOL vault #${res.data.vaultId} opened, send SOL to the custody address`);
        await refresh();
      } else {
        toastStore.error(res.error ?? 'Could not open SOL vault');
      }
    } finally {
      opening = false;
    }
  }

  async function confirmDeposit(vaultId: number) {
    busyVaultId = vaultId;
    try {
      const res = await SolVaultService.confirmSolDeposit(vaultId);
      if (res.success) {
        const sol = res.data ? Number(res.data.creditedLamports) / 1_000_000_000 : 0;
        toastStore.success(
          res.oisyResilient
            ? 'Deposit confirmed, credited on-chain (refresh to see the new vault).'
            : sol > 0
              ? `Deposit confirmed, credited ${sol} SOL`
              : 'Deposit confirmed'
        );
        await refresh();
        // The funded vault now renders as a normal VaultCard, reload the main list.
        dispatch('confirmed');
      } else {
        toastStore.error(res.error ?? 'Deposit not found yet, send SOL first, then retry');
      }
    } finally {
      busyVaultId = null;
    }
  }

  async function settleClaim(claim: SolClaimView) {
    // Phase 1 (not yet in flight) requires a valid Solana destination. Phase 2 (the
    // "Confirm" of an in-flight settlement) is a pure confirm: the backend ignores
    // `destination` on the validated path, so we pass '' and never let a freshly
    // typed address redirect an already-submitted (or to-be-re-signed) transfer.
    let dest = (claimDest[claim.claimId] ?? '').trim();
    if (!claim.inFlight) {
      if (!isPlausibleSolAddress(dest)) {
        toastStore.error('Enter a valid Solana address (base58, 32 bytes)');
        return;
      }
    } else if (dest !== '' && !isPlausibleSolAddress(dest)) {
      toastStore.error('Enter a valid replacement Solana address (base58, 32 bytes)');
      return;
    }
    busyClaimId = claim.claimId;
    try {
      const res = await SolVaultService.settleSolClaim(claim.claimId, dest);
      if (res.success) {
        // Two-phase: the first call submits the durable-nonce transfer but KEEPS the
        // claim; it clears only after a follow-up "Confirm" once the transfer
        // confirms (a few seconds). We re-read so the button flips to "Confirm".
        toastStore.success(
          claim.inFlight
            ? 'Confirming settlement…'
            : 'Payment submitted, once it confirms, click Confirm to clear the claim.'
        );
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

<section class="sol-panel">
  <header class="sol-head">
    <h3>Native SOL</h3>
    <div class="sol-head-actions">
      <button class="sol-secondary" disabled={!connected || checkingWallet || opening} on:click={checkWalletStatus}>
        {checkingWallet ? 'Checking...' : 'Check deposits'}
      </button>
      <button class="sol-open" disabled={!connected || opening} on:click={openVault}>
        {opening ? 'Opening…' : 'Open SOL vault'}
      </button>
    </div>
  </header>

  {#if !connected}
    <p class="sol-muted">Connect your wallet to use SOL collateral.</p>
  {:else}
    {#if pending.length > 0}
      <div class="sol-group">
        <div class="sol-group-title">Awaiting deposit</div>
        {#each pending as p (p.vaultId)}
          <div class="sol-row">
            <div class="sol-row-main">
              <span class="sol-label">Vault #{p.vaultId}</span>
              <button class="sol-addr" title={p.custodyAddress} on:click={() => copy(p.custodyAddress)}>
                {formatAddress(p.custodyAddress, 8, 6)} ⧉
              </button>
              <span class="sol-hint">Send SOL from any Solana wallet (e.g. Phantom) to this address, then confirm.</span>
            </div>
            <div class="sol-row-actions">
              <button
                class="sol-action"
                disabled={busyVaultId === p.vaultId}
                on:click={() => confirmDeposit(p.vaultId)}
              >
                {busyVaultId === p.vaultId ? 'Checking…' : "I've sent it, confirm"}
              </button>
              <button
                class="sol-hide"
                disabled={busyVaultId === p.vaultId}
                title="Hide this pending deposit locally. The custody address remains recoverable."
                on:click={() => hidePendingDeposit(p.vaultId)}
              >
                Hide
              </button>
            </div>
          </div>
        {/each}
      </div>
    {/if}

    {#if hiddenPending.length > 0}
      <div class="sol-group sol-hidden-group">
        <div class="sol-group-title">Hidden pending deposits</div>
        {#each hiddenPending as p (p.vaultId)}
          <div class="sol-row sol-hidden-row">
            <div class="sol-row-main">
              <span class="sol-label">Vault #{p.vaultId}</span>
              <button class="sol-addr" title={p.custodyAddress} on:click={() => copy(p.custodyAddress)}>
                {formatAddress(p.custodyAddress, 8, 6)} ⧉
              </button>
              <span class="sol-hint">Hidden only in this browser. Restore it before confirming a deposit.</span>
            </div>
            <button class="sol-action" on:click={() => restorePendingDeposit(p.vaultId)}>Restore</button>
          </div>
        {/each}
      </div>
    {/if}

    {#if claims.length > 0}
      <div class="sol-group">
        <div class="sol-group-title">SOL claims to settle</div>
        {#each claims as c (c.claimId)}
          <div class="sol-row">
            <div class="sol-row-main">
              <span class="sol-label">Claim #{c.claimId}</span>
              <span class="sol-amt">{c.sol} SOL</span>
              {#if c.inFlight}
                <span class="sol-hint">
                  Settlement in flight{c.inFlightSignature ? ` (tx ${formatAddress(c.inFlightSignature, 8, 6)})` : ''}, click Confirm once it validates, or enter a replacement address if it expired or failed.
                </span>
              {/if}
                <div class="sol-inputs">
                  <input
                    class="sol-input"
                    placeholder={c.inFlight ? 'Replacement address (optional)' : 'Your Solana address'}
                    bind:value={claimDest[c.claimId]}
                    spellcheck="false"
                    autocomplete="off"
                  />
                </div>
            </div>
            <button class="sol-action" disabled={busyClaimId === c.claimId} on:click={() => settleClaim(c)}>
              {busyClaimId === c.claimId ? 'Settling…' : c.inFlight ? 'Confirm' : 'Settle'}
            </button>
          </div>
        {/each}
      </div>
    {/if}

    {#if !loading && pending.length === 0 && claims.length === 0 && hiddenPending.length === 0}
      <p class="sol-muted">No pending SOL deposits or claims.</p>
    {/if}
  {/if}
</section>

<style>
  .sol-panel {
    display: flex;
    flex-direction: column;
    gap: 14px;
    padding: 16px;
    border: 1px solid var(--rumi-border, rgba(148, 163, 184, 0.2));
    border-radius: 14px;
    background: var(--rumi-surface, rgba(15, 23, 42, 0.4));
  }
  .sol-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }
  .sol-head h3 {
    margin: 0;
    font-size: 1rem;
    color: var(--rumi-text, #e2e8f0);
  }
  .sol-head-actions {
    display: flex;
    gap: 8px;
    align-items: center;
    flex-wrap: wrap;
    justify-content: flex-end;
  }
  .sol-open {
    padding: 7px 14px;
    border-radius: 10px;
    border: none;
    background: #9945ff;
    color: #f4f0ff;
    font-weight: 600;
    cursor: pointer;
  }
  .sol-open:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .sol-secondary {
    padding: 7px 13px;
    border-radius: 10px;
    border: 1px solid var(--rumi-border, rgba(148, 163, 184, 0.25));
    background: rgba(15, 23, 42, 0.45);
    color: var(--rumi-text-secondary, #cbd5e1);
    font-weight: 600;
    cursor: pointer;
  }
  .sol-secondary:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .sol-group {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .sol-group-title {
    font-size: 0.78rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--rumi-text-muted, #94a3b8);
  }
  .sol-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 10px 12px;
    border-radius: 10px;
    background: var(--rumi-surface-2, rgba(30, 41, 59, 0.5));
  }
  .sol-hidden-row {
    opacity: 0.72;
  }
  .sol-hidden-group {
    border-top: 1px solid var(--rumi-border, rgba(148, 163, 184, 0.18));
    padding-top: 10px;
  }
  .sol-row-main {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
  }
  .sol-label {
    font-weight: 600;
    color: var(--rumi-text, #e2e8f0);
  }
  .sol-amt {
    color: #b48bff;
    font-variant-numeric: tabular-nums;
  }
  .sol-addr {
    background: none;
    border: none;
    padding: 0;
    color: var(--rumi-text-secondary, #cbd5e1);
    font-family: ui-monospace, monospace;
    cursor: pointer;
    text-align: left;
  }
  .sol-hint,
  .sol-muted {
    font-size: 0.8rem;
    color: var(--rumi-text-muted, #94a3b8);
  }
  .sol-input {
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
  .sol-inputs {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .sol-action {
    padding: 7px 13px;
    border-radius: 10px;
    border: 1px solid #9945ff;
    background: transparent;
    color: #b48bff;
    font-weight: 600;
    cursor: pointer;
    white-space: nowrap;
  }
  .sol-action:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .sol-row-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    justify-content: flex-end;
  }
  .sol-hide {
    padding: 7px 11px;
    border-radius: 10px;
    border: 1px solid var(--rumi-border, rgba(148, 163, 184, 0.25));
    background: rgba(15, 23, 42, 0.45);
    color: var(--rumi-text-muted, #94a3b8);
    font-weight: 600;
    cursor: pointer;
  }
  .sol-hide:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
