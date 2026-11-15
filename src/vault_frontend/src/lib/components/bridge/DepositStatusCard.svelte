<script lang="ts">
  import type { DepositView } from '$declarations/rumi_icusd_bridge/rumi_icusd_bridge.did';
  import type { BaseChainConfig } from '$lib/services/baseBridge/chains';
  import { formatE8s } from '$lib/services/baseBridge/amounts';
  import { displayAddress } from '$lib/services/baseBridge/eip55';
  import { describeDeposit, type DepositStage } from '$lib/services/baseBridge/status';
  import StatusSteps from './StatusSteps.svelte';
  import TxLink from './TxLink.svelte';

  interface Props {
    deposit: DepositView;
    /** Text form of the signed-in ICP principal. */
    principal: string;
    chain: BaseChainConfig;
    /** True while the bridge's Minter lane has no Base gas (`BridgeConfig.minterAwaitingGas`). */
    awaitingGas?: boolean;
    onRefund?: () => void;
    refundBusy?: boolean;
    refundMessage?: string;
  }

  let { deposit, principal, chain, awaitingGas = false, onRefund, refundBusy = false, refundMessage = '' }: Props = $props();

  const display = $derived(describeDeposit(deposit, { principal, awaitingGas }));
  const STEPS = ['Escrowed', 'Minting', 'Included on Base', 'Delivered'];
  const ACTIVE_STEP: Partial<Record<DepositStage, number>> = {
    pulling: 0,
    pull_unknown: 0,
    escrowed: 0,
    awaiting_gas: 1,
    minting: 1,
    included: 2,
    delivered: 4,
    held: 1,
  };
  const activeStep = $derived(ACTIVE_STEP[display.stage]);
  const hashes = $derived(deposit.mint_tx_hashes);
</script>

<section class="status-card" class:warn={display.tone === 'warning'} class:bad={display.tone === 'error'} class:good={display.tone === 'success'} aria-live="polite" aria-label="Deposit status">
  <header>
    <span class="kicker">DEPOSIT #{deposit.id}</span>
    <strong class="label">{display.label}</strong>
  </header>
  <p class="detail">{display.detail}</p>
  {#if activeStep !== undefined}
    <StatusSteps steps={STEPS} active={activeStep} attention={display.stage === 'held' || display.stage === 'awaiting_gas'} label="Deposit progress" />
  {/if}
  <dl class="facts">
    <div><dt>Sent</dt><dd>{formatE8s(deposit.amount_e8s)} icUSD</dd></div>
    <div><dt>Minted on Base</dt><dd>{formatE8s(deposit.mint_amount_e8s)} icUSD</dd></div>
    <div><dt>Recipient</dt><dd><code>{displayAddress(deposit.base_recipient)}</code></dd></div>
  </dl>
  {#if display.finalized}
    <p class="final">Finalized: the bridge confirmed delivery after Base L1 batch finality.</p>
  {/if}
  {#each hashes as hash (hash)}
    <div class="hash"><TxLink {hash} {chain} hint label="Mint attempt" /></div>
  {/each}
  {#if deposit.delivered[0]}
    <div class="hash"><TxLink hash={deposit.delivered[0].tx_hash} {chain} label="Delivered in" /></div>
  {/if}
  {#if display.canRefund && onRefund}
    <div class="actions">
      <button type="button" class="btn-secondary" onclick={onRefund} disabled={refundBusy}>
        {refundBusy ? 'Requesting refund...' : 'Request refund'}
      </button>
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
  .hash { margin-top:0.4rem; }
  .actions { display:flex; flex-wrap:wrap; align-items:center; gap:0.6rem; margin-top:0.8rem; }
  .hint { font-size:0.7rem; color:var(--rumi-text-muted); }
  .refund-message { margin:0.6rem 0 0; font-size:0.75rem; color:var(--rumi-text-secondary); }
</style>
