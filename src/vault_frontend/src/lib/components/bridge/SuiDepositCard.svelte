<script lang="ts">
  import type { DepositView } from '$declarations/rumi_icusd_sui_bridge/rumi_icusd_sui_bridge.did';
  import { formatE8s } from '$lib/services/baseBridge/amounts';
  import { explorerTxUrl, type SuiNetworkInfo } from '$lib/services/suiBridge/networks';
  import { describeSuiDeposit, type SuiDepositStage } from '$lib/services/suiBridge/status';
  import { shortAddress } from '$lib/services/baseBridge/status';
  import StatusSteps from './StatusSteps.svelte';

  interface Props {
    deposit: DepositView;
    /** Text form of the signed-in ICP principal. */
    principal: string;
    network: SuiNetworkInfo;
    onRefund?: () => void;
    refundBusy?: boolean;
    refundMessage?: string;
  }

  let { deposit, principal, network, onRefund, refundBusy = false, refundMessage = '' }: Props = $props();

  const display = $derived(describeSuiDeposit(deposit, { principal }));
  const STEPS = ['Escrowed', 'Minting', 'Delivered'];
  const ACTIVE_STEP: Partial<Record<SuiDepositStage, number>> = { pulling: 0, pull_unknown: 0, escrowed: 0, minting: 1, delivered: 3, held: 1 };
  const activeStep = $derived(ACTIVE_STEP[display.stage]);
</script>

<section class="status-card" class:warn={display.tone === 'warning'} class:bad={display.tone === 'error'} class:good={display.tone === 'success'} aria-live="polite" aria-label="Deposit status">
  <header>
    <span class="kicker">DEPOSIT #{deposit.id}</span>
    <strong class="label">{display.label}</strong>
  </header>
  <p class="detail">{display.detail}</p>
  {#if activeStep !== undefined}
    <StatusSteps steps={STEPS} active={activeStep} attention={display.stage === 'held'} label="Deposit progress" />
  {/if}
  <dl class="facts">
    <div><dt>Sent</dt><dd>{formatE8s(deposit.amount_e8s)} icUSD</dd></div>
    <div><dt>Minted on {network.name}</dt><dd>{formatE8s(deposit.mint_amount_e8s)} icUSD</dd></div>
    <div><dt>Recipient wallet</dt><dd><code>{deposit.sui_recipient}</code></dd></div>
  </dl>
  {#if display.digest}
    <p class="final">
      Final: the bridge recorded the mint after it was final on Sui.
      <a href={explorerTxUrl(network, display.digest)} target="_blank" rel="noopener noreferrer" title={display.digest}>{shortAddress(display.digest)}<span class="sr-only"> (opens the block explorer)</span></a>
    </p>
  {/if}
  {#if display.canRefund && onRefund}
    <div class="actions">
      <button type="button" class="btn-secondary" onclick={onRefund} disabled={refundBusy}>{refundBusy ? 'Requesting refund...' : 'Request refund'}</button>
      <span class="hint">Returns your escrowed icUSD to your account (minus the ledger fee).</span>
    </div>
  {/if}
  {#if refundMessage}<p class="refund-message" role="status">{refundMessage}</p>{/if}
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
  dt { color:var(--rumi-text-muted); }
  dd { margin:0; text-align:right; color:var(--rumi-text-primary); overflow-wrap:anywhere; font-variant-numeric:tabular-nums; }
  code { font-size:0.7rem; }
  .final { margin:0.6rem 0 0; font-size:0.75rem; color:var(--rumi-safe); }
  .final a { color:var(--rumi-action-bright); }
  .actions { display:flex; flex-wrap:wrap; align-items:center; gap:0.6rem; margin-top:0.8rem; }
  .hint { font-size:0.7rem; color:var(--rumi-text-muted); }
  .refund-message { margin:0.6rem 0 0; font-size:0.75rem; color:var(--rumi-text-secondary); }
  .sr-only { position:absolute; width:1px; height:1px; padding:0; margin:-1px; overflow:hidden; clip:rect(0,0,0,0); white-space:nowrap; border:0; }
</style>
