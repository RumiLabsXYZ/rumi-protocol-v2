<script lang="ts">
  import { createEventDispatcher, onDestroy } from 'svelte';
  import type { XrpVaultDepositAddress } from '$declarations/rumi_protocol_backend/rumi_protocol_backend.did';
  import { walletStore } from '$lib/stores/wallet';
  import { confirmXrpVaultTopup, copyXrpVaultAddress, findXrpVaultOrigin, loadXrpVaultAddress } from '$lib/services/xrpExistingDeposit';

  export let vaultId: number;
  const dispatch = createEventDispatcher<{ credited: void }>();
  let address: XrpVaultDepositAddress | null = null;
  let origin: bigint | undefined;
  let cursor = 0n;
  let historyEnd = false;
  let busy = false;
  let error = '';
  let notice = '';
  let txHash = '';
  let context = '';
  let generation = 0;
  onDestroy(() => { generation += 1; });
  $: owner = $walletStore.principal?.toText() ?? '';
  $: if (`${owner}:${vaultId}` !== context) reset(`${owner}:${vaultId}`);
  $: full = address !== null && address.receipt_count >= address.receipt_limit;

  function reset(key: string) {
    context = key;
    generation += 1;
    address = null;
    origin = undefined;
    cursor = 0n;
    historyEnd = false;
    error = '';
    notice = '';
    busy = false;
    try { txHash = localStorage.getItem(`rumi_xrp_topup:${key}`) ?? ''; }
    catch { txHash = ''; }
  }

  async function showAddress() {
    const active = generation;
    busy = true;
    error = '';
    notice = '';
    try {
      const result = await loadXrpVaultAddress(vaultId, owner, origin);
      if (active === generation) address = result;
    } catch (e) {
      if (active === generation) error = String(e instanceof Error ? e.message : e);
    } finally { if (active === generation) busy = false; }
  }

  async function findOpening() {
    const active = generation;
    const boundOwner = owner;
    const boundVault = vaultId;
    busy = true;
    error = '';
    try {
      // A bounded batch; continuing is another explicit action. Public history
      // discovery never requests a wallet signature or credits collateral.
      for (let page = 0; page < 10; page += 1) {
        const result = await findXrpVaultOrigin(boundVault, boundOwner, cursor);
        if (active !== generation) return;
        cursor = result.next;
        historyEnd = result.end;
        if (result.index !== undefined) {
          origin = result.index;
          notice = 'Vault record found. Select Show deposit address to continue.';
          return;
        }
        if (result.end) break;
      }
      notice = historyEnd
        ? 'The original vault record could not be found. Contact support before sending XRP.'
        : 'More vault history is available. Continue the search before sending XRP.';
    } catch (e) {
      if (active === generation) error = String(e instanceof Error ? e.message : e);
    } finally { if (active === generation) busy = false; }
  }

  async function copyAddress() {
    if (!address || full) return;
    const active = generation;
    try {
      await copyXrpVaultAddress(owner, address.custody_address);
      if (active === generation) notice = 'Deposit address copied.';
    } catch {
      if (active === generation) error = 'Could not copy the address. Select and copy it manually.';
    }
  }

  async function confirm() {
    const active = generation;
    const key = context;
    const hash = txHash.trim();
    busy = true;
    error = '';
    notice = '';
    try {
      // Retain only a transaction hint. The backend verifies and deduplicates it.
      try { localStorage.setItem(`rumi_xrp_topup:${key}`, hash); } catch { /* optional recovery hint */ }
      const amount = await confirmXrpVaultTopup(vaultId, owner, hash, origin);
      if (active !== generation) return;
      // Require a fresh capacity/address check before inviting another send.
      address = null;
      const whole = amount / 1_000_000n;
      const fraction = (amount % 1_000_000n).toString().padStart(6, '0').replace(/0+$/, '');
      notice = `Payment verified: ${whole}${fraction ? `.${fraction}` : ''} XRP. Rechecking the same payment cannot credit it twice.`;
      dispatch('credited');
    } catch (e) {
      if (active === generation) error = `${e instanceof Error ? e.message : String(e)} Keep the same transaction hash when checking again; do not send another payment to retry verification.`;
    } finally { if (active === generation) busy = false; }
  }
</script>

<section class="xrp-deposit" aria-label={`Deposit XRP to vault ${vaultId}`}>
  <h4>Deposit XRP</h4>
  <p>Add XRP to this existing vault. Showing an address does not send funds.</p>
  <button class="btn" on:click={showAddress} disabled={busy || !owner}>
    {busy ? 'Checking…' : 'Show deposit address'}
  </button>
  {#if error && !address && origin === undefined}
    <button class="btn secondary" on:click={findOpening} disabled={busy || historyEnd || !owner}>
      {cursor > 0n ? 'Continue finding my vault' : 'Find my vault record'}
    </button>
  {/if}
  {#if !address && origin === undefined && cursor > 0n && !historyEnd && !error}
    <button class="btn secondary" on:click={findOpening} disabled={busy}>Continue finding my vault</button>
  {/if}
  {#if address}
    <p class="network">XRP Ledger · {address.network}</p>
    {#if full}
      <p role="alert">New deposits are paused for this vault. Contact support before sending XRP.</p>
    {:else}
      <code class="address">{address.custody_address}</code>
      <button class="btn secondary" on:click={copyAddress} disabled={busy}>Copy address</button>
      <p>Send a direct native XRP payment on the network shown above, without a swap or partial payment. No destination tag is needed. After it settles, paste its transaction hash below to add it to this vault.</p>
      <p>Keep this vault open until the deposit is verified.</p>
    {/if}
    <label for={`xrp-topup-${vaultId}`}>XRP transaction hash</label>
    <input id={`xrp-topup-${vaultId}`} bind:value={txHash} maxlength="66" autocomplete="off" spellcheck="false" disabled={busy} placeholder="64-character transaction hash" />
    <button class="btn" on:click={confirm} disabled={busy || !/^[0-9a-fA-F]{64}$/.test(txHash.trim())}>
      {busy ? 'Checking…' : 'Verify deposit'}
    </button>
    <p>Already sent XRP? Verify that payment instead of sending it again. Only a confirmed payment updates your collateral.</p>
  {/if}
  {#if notice}<p role="status">{notice}</p>{/if}
  {#if error}<p class="error" role="alert">{error}</p>{/if}
</section>

<style>
  .xrp-deposit { display: grid; gap: .75rem; min-width: 0; }
  h4, p { margin: 0; }
  p, label { font-size: .8125rem; line-height: 1.5; color: #a8b4c8; }
  h4 { font-size: .95rem; color: #e2e8f0; }
  .address { overflow-wrap: anywhere; font-size: .8rem; color: #e2e8f0; user-select: all; }
  .network { color: #e2e8f0; text-transform: capitalize; }
  input { width: 100%; min-width: 0; border: 1px solid #334155; border-radius: .5rem; background: #0f172a; color: #e2e8f0; padding: .65rem; }
  .btn { padding: .6rem .8rem; border: 1px solid #64748b; border-radius: .5rem; background: #334155; color: #f1f5f9; cursor: pointer; }
  .secondary { background: transparent; }
  .btn:disabled { opacity: .5; cursor: default; }
  .error { color: #fda4af; }
</style>
