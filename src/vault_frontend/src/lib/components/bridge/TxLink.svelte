<script lang="ts">
  import { isBytes32 } from '$lib/services/baseBridge/abi';
  import { explorerTxUrl, type BaseChainConfig } from '$lib/services/baseBridge/chains';
  import { shortHash } from '$lib/services/baseBridge/status';

  interface Props {
    hash: string;
    chain: BaseChainConfig;
    /** Wallet-reported hashes are hints, not proof. */
    hint?: boolean;
    label?: string;
  }

  let { hash, chain, hint = false, label = 'Base transaction' }: Props = $props();
  // Only a well-formed hash is ever placed in a link.
  const url = $derived(isBytes32(hash) ? explorerTxUrl(chain, hash) : null);
</script>

<span class="tx-link">
  <span class="tx-label">{label}{hint ? ' (hint)' : ''}:</span>
  {#if url}
    <a href={url} target="_blank" rel="noopener noreferrer" title={hash}>{shortHash(hash)}<span class="sr-only"> (opens the block explorer)</span></a>
  {:else}
    <code title={hash}>{shortHash(hash)}</code>
  {/if}
</span>

<style>
  .tx-link { display:inline-flex; flex-wrap:wrap; gap:0.35rem; align-items:baseline; font-size:0.75rem; color:var(--rumi-text-secondary); overflow-wrap:anywhere; }
  .tx-label { color:var(--rumi-text-muted); }
  a { color:var(--rumi-action-bright); }
  code { color:var(--rumi-text-secondary); }
  .sr-only { position:absolute; width:1px; height:1px; padding:0; margin:-1px; overflow:hidden; clip:rect(0,0,0,0); white-space:nowrap; border:0; }
</style>
