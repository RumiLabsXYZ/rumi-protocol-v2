<script lang="ts">
  import { shortenPrincipal, getCanisterName, getTokenSymbol, isKnownCanister } from '$utils/explorerHelpers';
  import { ammPoolShortLabel } from '$utils/ammNaming';
  import { CANISTER_IDS } from '$lib/config';

  interface Props {
    type: 'vault' | 'address' | 'token' | 'event' | 'canister' | 'pool' | 'block_index';
    value: string;
    label?: string;
    short?: boolean;
    /** When set, replaces the default link styling. Use for chip-pill variants in event rows. */
    class?: string;
  }

  let { type, value, label, short = true, class: extraClass }: Props = $props();

  const icons: Record<string, string> = {
    vault: '🏦',
    address: '👤',
    canister: '📦',
    token: '🪙',
    pool: '🌊',
    event: '',
    block_index: '🔗',
  };

  const icon = $derived(icons[type] ?? '');

  const href = $derived.by(() => {
    switch (type) {
      case 'vault': return `/explorer/e/vault/${value}`;
      case 'address': return `/explorer/e/address/${value}`;
      case 'token': return `/explorer/e/token/${value}`;
      case 'pool': return `/explorer/e/pool/${value}`;
      case 'event': return `/explorer/e/event/${value}`;
      case 'canister': return `/explorer/e/address/${value}`;
      case 'block_index': return null;
    }
  });

  const addressIcon = $derived(isKnownCanister(value) ? '📦' : '👤');

  const displayIcon = $derived(type === 'address' ? addressIcon : icon);

  const displayText = $derived.by(() => {
    if (label) return label;
    switch (type) {
      case 'vault': return `Vault #${value}`;
      case 'address': {
        const name = getCanisterName(value);
        if (name) return name;
        return short ? shortenPrincipal(value) : value;
      }
      case 'token': return getTokenSymbol(value) ?? (short ? shortenPrincipal(value) : value);
      case 'pool': {
        if (value === '3pool' || value === CANISTER_IDS.THREEPOOL) return '3pool';
        // AMM pool ids are joined principals like `pA_pB`. Resolve via the
        // pool registry for "AMM1"-style labels; fall back to the shortened
        // principal pair if the registry hasn't loaded yet.
        const ammLabel = ammPoolShortLabel(value);
        if (ammLabel && ammLabel !== 'AMM') return ammLabel;
        return short ? shortenPrincipal(value) : value;
      }
      case 'event': return `Event #${value}`;
      case 'canister': {
        const name = getCanisterName(value);
        if (name) return name;
        return short ? shortenPrincipal(value) : value;
      }
      case 'block_index': return `#${value}`;
    }
  });

  const linkClass = $derived(
    extraClass ?? 'inline-flex items-center gap-1 text-blue-400 hover:underline font-mono text-sm',
  );
  const spanClass = $derived(
    extraClass ?? 'inline-flex items-center gap-1 text-gray-300 font-mono text-sm',
  );
</script>

{#if href}
  <a {href} class={linkClass} title={value}>
    <span>{displayIcon}</span><span>{displayText}</span>
  </a>
{:else}
  <span class={spanClass} title={value}>
    <span>{displayIcon}</span><span>{displayText}</span>
  </span>
{/if}
