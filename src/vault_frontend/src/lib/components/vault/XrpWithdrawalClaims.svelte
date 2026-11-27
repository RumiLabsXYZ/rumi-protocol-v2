<script lang="ts">
  import { captureActionBoundContext } from '$lib/services/protocol/walletOperations';
  import { isValidXrplClassicAddress, validateXrpPayoutInput } from '$lib/services/xrpPayoutHelpers';
  import { XrpVaultService, type XrpClaimView } from '$lib/services/xrpVaultService';
  import { formatBigintWithDecimals } from '$lib/utils/bigint';

  let claims: XrpClaimView[] | null = null;
  let addresses: Record<string, string> = {};
  let tags: Record<string, string> = {};
  let busy = false;
  let message = '';

  async function checkClaims() {
    if (busy) return;
    busy = true;
    claims = null;
    message = '';
    try {
      const ctx = captureActionBoundContext();
      const rows = await XrpVaultService.readMyClaims(ctx);
      if (ctx.assertCurrent()) claims = rows;
    } catch (err) {
      message = `Could not check XRP claims. ${err instanceof Error ? err.message : String(err)}`;
    } finally {
      busy = false;
    }
  }

  async function settle(claim: XrpClaimView) {
    if (busy || claim.quarantineReason != null) return;
    let destination = '';
    let tag: number | undefined;
    if (!claim.inFlight) {
      const payout = validateXrpPayoutInput(addresses[claim.claimId] ?? '', tags[claim.claimId]);
      if (!payout.ok || !payout.address || !isValidXrplClassicAddress(payout.address)) {
        message = payout.error ?? 'Enter a valid XRPL classic address beginning with r.';
        return;
      }
      destination = payout.address;
      tag = payout.destinationTag;
    }
    busy = true;
    message = '';
    try {
      const ctx = captureActionBoundContext();
      const result = await XrpVaultService.settleXrpClaim(claim.claimId, destination, tag, ctx);
      if (!ctx.assertCurrent()) return;
      if (!result.success) throw new Error(result.error ?? 'The wallet did not return a result.');
      message = claim.inFlight
        ? 'Check XRP claims again to see whether settlement is complete.'
        : 'Check XRP claims again after the payment validates, then select Confirm settlement.';
    } catch (err) {
      message = `${err instanceof Error ? err.message : String(err)} Check XRP claims before trying again.`;
    } finally {
      // A lost response can follow a real payment. Re-read the owner's claim before another action.
      claims = null;
      busy = false;
    }
  }
</script>

<section class="glass-card claims" id="xrp-claims" aria-label="XRP withdrawal claims">
  <div class="claims-heading">
    <h2>XRP withdrawal claims</h2>
    <button type="button" class="btn-secondary" disabled={busy} on:click={checkClaims}>Check XRP claims</button>
  </div>
  {#if busy}<p role="status">Waiting for your wallet…</p>{/if}
  {#if message}<p role="status">{message}</p>{/if}
  {#if claims?.length === 0}<p role="status">No outstanding XRP withdrawal claims for this wallet.</p>{/if}
  {#each claims ?? [] as claim (claim.claimId)}
    <div class="claim">
      <h3>Claim #{claim.claimId} · Vault #{claim.vaultId}</h3>
      <p><strong>{formatBigintWithDecimals(claim.drops, 6)} XRP</strong> before the XRP Ledger fee.</p>
      {#if claim.quarantineReason != null}
        <p>This claim is on hold for review.</p>
      {:else if claim.inFlight}
        {#if claim.inFlightTxHash}<p class="tx">Transaction <code>{claim.inFlightTxHash}</code></p>{/if}
        <p>Confirm after the payment validates. If it expired or failed, Rumi can retry using the saved payout details.</p>
        <button type="button" class="btn-primary" disabled={busy} on:click={() => settle(claim)}>Confirm settlement</button>
      {:else}
        <form on:submit|preventDefault={() => settle(claim)}>
          <label>XRP receiving address
            <input type="text" bind:value={addresses[claim.claimId]} placeholder="r…" required
              autocomplete="off" autocapitalize="none" spellcheck="false" disabled={busy} />
          </label>
          <label>Destination tag (if your recipient requires one)
            <input type="text" inputmode="numeric" bind:value={tags[claim.claimId]}
              autocomplete="off" disabled={busy} />
          </label>
          <button type="submit" class="btn-primary" disabled={busy}>Settle claim</button>
        </form>
      {/if}
    </div>
  {/each}
</section>

<style>
  .claims { padding: 1rem; margin-bottom: 1rem; }
  .claims-heading { display: flex; align-items: center; justify-content: space-between; flex-wrap: wrap; gap: .75rem; }
  h2, h3 { font-size: 1rem; font-weight: 600; margin: 0; }
  p { margin: .65rem 0; }
  .claim { border-top: 1px solid var(--rumi-border); margin-top: 1rem; padding-top: 1rem; }
  form, label { display: grid; gap: .4rem; }
  form { gap: .75rem; }
  input { width: 100%; padding: .55rem; color: var(--rumi-text-primary); background: var(--rumi-bg-surface-1); border: 1px solid var(--rumi-border-hover); border-radius: .4rem; }
  form button { justify-self: start; }
  .tx { overflow-wrap: anywhere; }
</style>
