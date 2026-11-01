<script lang="ts">
  import type { BaseToIcpIntent } from '$lib/services/baseBridge/intents';
  import type { BaseChainConfig } from '$lib/services/baseBridge/chains';
  import { formatE8s } from '$lib/services/baseBridge/amounts';
  import type { BaseOutStage, BaseOutTracking } from '$lib/services/baseBridge/status';
  import StatusSteps from './StatusSteps.svelte';
  import TxLink from './TxLink.svelte';

  interface Props {
    intent: BaseToIcpIntent;
    tracking: BaseOutTracking;
    chain: BaseChainConfig;
    onNotify?: () => void;
    notifyBusy?: boolean;
    notifyMessage?: string;
  }

  let { intent, tracking, chain, onNotify, notifyBusy = false, notifyMessage = '' }: Props = $props();

  const STEPS = ['Submitted', 'Included on Base', 'Finality', 'Released'];
  const ACTIVE_STEP: Partial<Record<BaseOutStage, number>> = {
    submitted: 0,
    awaiting_finality: 2,
    release_pending: 3,
    confirming: 3,
    released: 4,
    held: 3,
    reminting: 3,
    reminted: 4,
  };
  const activeStep = $derived(ACTIVE_STEP[tracking.stage]);
  const attention = $derived(tracking.stage === 'held' || tracking.stage === 'reminting');
</script>

<section class="status-card" class:warn={tracking.tone === 'warning'} class:bad={tracking.tone === 'error'} class:good={tracking.tone === 'success'} aria-live="polite" aria-label="Transfer status">
  <header>
    <span class="kicker">BASE TO ICP</span>
    <strong class="label">{tracking.label}</strong>
  </header>
  <p class="detail">{tracking.detail}</p>
  {#if activeStep !== undefined}
    <StatusSteps steps={STEPS} active={activeStep} {attention} label="Transfer progress" />
  {/if}
  <dl class="facts">
    <div><dt>Burned on Base</dt><dd>{formatE8s(intent.amountE8s)} icUSD</dd></div>
    <div><dt>ICP destination</dt><dd><code>{intent.destinationPrincipal}</code></dd></div>
    <div><dt>Transfer id</dt><dd><code>{intent.transferId}</code></dd></div>
  </dl>
  <p class="authority">Only the bridge canister's release record marks this transfer as released. Wallet and Base receipts shown here are hints.</p>
  {#if intent.txHash}
    <div class="hash"><TxLink hash={intent.txHash} {chain} hint /></div>
  {/if}
  {#if tracking.includedBlock !== undefined}
    <p class="hint">Included on Base in block {tracking.includedBlock} (wallet report).</p>
  {/if}
  {#if intent.txHash && onNotify && !intent.notified && !tracking.terminal}
    <div class="actions">
      <button type="button" class="btn-secondary" onclick={onNotify} disabled={notifyBusy}>
        {notifyBusy ? 'Telling the bridge...' : 'Tell the bridge'}
      </button>
      <span class="hint">Optional. Asks the bridge to look at this transaction sooner. It moves no funds and does not change the finality wait.</span>
    </div>
  {/if}
  {#if intent.notified}<p class="hint">The bridge has been told about this transaction.</p>{/if}
  {#if notifyMessage}<p class="hint" role="status">{notifyMessage}</p>{/if}
</section>

<style>
  .status-card { margin-top:1rem; padding:0.9rem 1rem; border:1px solid var(--rumi-border); border-radius:0.6rem; background:var(--rumi-bg-primary); }
  .status-card.good { border-color:var(--rumi-border-teal); }
  .status-card.warn { border-color:rgba(229,168,90,0.4); }
  .status-card.bad { border-color:rgba(224,107,159,0.4); }
  header { display:flex; flex-direction:column; gap:0.15rem; }
  .kicker { font-size:0.65rem; letter-spacing:0.09em; color:var(--rumi-text-muted); font-weight:600; }
  .label { font-size:1rem; color:var(--rumi-text-primary); }
  .detail { margin:0.35rem 0 0; font-size:0.8rem; line-height:1.5; color:var(--rumi-text-secondary); }
  .facts { display:grid; gap:0.35rem; margin:0.6rem 0 0; }
  .facts div { display:flex; justify-content:space-between; gap:1rem; font-size:0.75rem; }
  dt { color:var(--rumi-text-muted); flex-shrink:0; }
  dd { margin:0; text-align:right; color:var(--rumi-text-primary); overflow-wrap:anywhere; font-variant-numeric:tabular-nums; }
  code { font-size:0.7rem; }
  .authority { margin:0.6rem 0 0; font-size:0.7rem; color:var(--rumi-text-muted); line-height:1.45; }
  .hash { margin-top:0.4rem; }
  .actions { display:flex; flex-wrap:wrap; align-items:center; gap:0.6rem; margin-top:0.8rem; }
  .hint { margin:0.4rem 0 0; font-size:0.7rem; color:var(--rumi-text-muted); line-height:1.45; }
</style>
