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
  import { onMount, createEventDispatcher } from 'svelte';
  import {
    XrpVaultService,
    type XrpPendingDepositView,
    type XrpClaimView,
  } from '$lib/services/xrpVaultService';
  import { walletStore } from '$lib/stores/wallet';
  import { toastStore } from '$lib/stores/toast';
  import { formatAddress } from '$lib/utils/format';

  // Notifies the vaults page to reload the main vault list once a deposit confirms
  // (the confirmed vault becomes a normal VaultCard, which this panel doesn't own).
  const dispatch = createEventDispatcher<{ confirmed: void }>();

  let pending: XrpPendingDepositView[] = [];
  let claims: XrpClaimView[] = [];
  let loading = false;
  let opening = false;
  let busyVaultId: number | null = null;
  let busyClaimId: number | null = null;
  // Per-claim destination address inputs, keyed by claim id.
  let claimDest: Record<number, string> = {};
  let claimTag: Record<number, string> = {};

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
        toastStore.success(
          res.oisyResilient
            ? 'Deposit confirmed — credited on-chain (refresh to see the new vault).'
            : xrp > 0
              ? `Deposit confirmed — credited ${xrp} XRP`
              : 'Deposit confirmed'
        );
        await refresh();
        // The funded vault now renders as a normal VaultCard — reload the main list.
        dispatch('confirmed');
      } else {
        toastStore.error(res.error ?? 'Deposit not found yet — send XRP first, then retry');
      }
    } finally {
      busyVaultId = null;
    }
  }

  async function settleClaim(claim: XrpClaimView) {
    // Phase 1 (not yet in flight) requires a valid XRPL destination. Phase 2 (the
    // "Confirm" of an in-flight settlement) is a pure confirm: the backend ignores
    // `destination` on the validated path, so we pass '' and never let a freshly
    // typed address redirect an already-submitted (or to-be-re-signed) Payment.
    let dest = (claimDest[claim.claimId] ?? '').trim();
    let destinationTag: number | undefined;
    if (!claim.inFlight) {
      if (!isValidXrplClassicAddress(dest)) {
        toastStore.error('Enter a valid XRPL classic address (starts with r)');
        return;
      }
    } else if (dest !== '' && !isValidXrplClassicAddress(dest)) {
      toastStore.error('Enter a valid replacement XRPL classic address (starts with r)');
      return;
    }
    const tag = (claimTag[claim.claimId] ?? '').trim();
    if (tag !== '') {
      if (!/^\d+$/.test(tag)) {
        toastStore.error('Destination tag must be a whole number');
        return;
      }
      destinationTag = Number(tag);
      if (!Number.isSafeInteger(destinationTag) || destinationTag > 0xffffffff) {
        toastStore.error('Destination tag must be between 0 and 4294967295');
        return;
      }
    }
    busyClaimId = claim.claimId;
    try {
      const res = await XrpVaultService.settleXrpClaim(claim.claimId, dest, destinationTag);
      if (res.success) {
        // Two-phase: the first call submits the XRPL Payment but KEEPS the claim;
        // it clears only after a follow-up "Confirm" once the Payment validates
        // (a few seconds). We re-read so the button flips to "Confirm".
        toastStore.success(
          claim.inFlight
            ? 'Confirming settlement…'
            : 'Payment submitted — once it validates, click Confirm to clear the claim.'
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

  // XRPL classic-address structural check. SYNC on purpose: it runs in the click
  // handler before the signer call, and any async (e.g. crypto.subtle for a checksum)
  // would burn the browser's user-gesture window and block the Oisy popup. Base58-
  // decodes with the RIPPLE alphabet and requires a 25-byte payload with the classic-
  // address version byte (0x00). The 4-byte checksum is NOT verified here — that needs
  // SHA-256, and the backend already does a full base58+checksum decode before signing
  // (chains/xrp/address.rs). This just catches typos / wrong-alphabet / X-addresses.
  const RIPPLE_B58 = 'rpshnaf39wBUDNEGHJKLM4PQRST7VWXYZ2bcdeCg65jkm8oFqi1tuvAxyz';
  function isValidXrplClassicAddress(addr: string): boolean {
    if (!addr || addr[0] !== 'r' || addr.length < 25 || addr.length > 35) return false;
    let num = 0n;
    for (const ch of addr) {
      const idx = RIPPLE_B58.indexOf(ch);
      if (idx < 0) return false; // char outside the ripple base58 alphabet
      num = num * 58n + BigInt(idx);
    }
    const bytes: number[] = [];
    while (num > 0n) {
      bytes.unshift(Number(num & 0xffn));
      num >>= 8n;
    }
    // Leading 'r' (alphabet index 0) chars decode to leading zero bytes.
    for (const ch of addr) {
      if (ch === 'r') bytes.unshift(0);
      else break;
    }
    return bytes.length === 25 && bytes[0] === 0x00;
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
              <span class="xrp-hint">Send XRP from any XRPL wallet (e.g. Xaman) to this address, then confirm.</span>
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
                <span class="xrp-hint">
                  Settlement in flight{c.inFlightTxHash ? ` (tx ${formatAddress(c.inFlightTxHash, 8, 6)})` : ''} — click Confirm once it validates, or enter replacement details if it expired or failed.
                </span>
              {/if}
                <div class="xrp-inputs">
                  <input
                    class="xrp-input"
                    placeholder={c.inFlight ? 'Replacement address (optional)' : 'Your XRPL address (r…)'}
                    bind:value={claimDest[c.claimId]}
                    spellcheck="false"
                    autocomplete="off"
                  />
                  <input
                    class="xrp-input xrp-tag-input"
                    placeholder="Destination tag"
                    bind:value={claimTag[c.claimId]}
                    spellcheck="false"
                    autocomplete="off"
                    inputmode="numeric"
                    pattern="[0-9]*"
                  />
                </div>
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
  .xrp-inputs {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .xrp-tag-input {
    min-width: 150px;
    max-width: 180px;
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
