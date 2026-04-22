<script lang="ts">
  import { shortenPrincipal, getCanisterName, getTokenSymbol, isKnownCanister } from '$utils/explorerHelpers';

  interface Props {
    type: 'vault' | 'address' | 'token' | 'event' | 'canister' | 'block_index';
    value: string;
    label?: string;
    short?: boolean;
  }

  let { type, value, label, short = true }: Props = $props();

  const icons: Record<string, string> = {
    vault: '🏦',
    address: '👤',
    canister: '📦',
    token: '🪙',
    event: '',
    block_index: '🔗',
  };

  const icon = $derived(icons[type] ?? '');

  const href = $derived.by(() => {
    switch (type) {
      case 'vault': return `/explorer/e/vault/${value}`;
      case 'address': return `/explorer/address/${value}`;
      case 'token': return `/explorer/token/${value}`;
      case 'event': return `/explorer/e/event/${value}`;
      case 'canister': return `/explorer/address/${value}`;
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
      case 'event': return `Event #${value}`;
      case 'canister': {
        const name = getCanisterName(value);
        if (name) return name;
        return short ? shortenPrincipal(value) : value;
      }
      case 'block_index': return `#${value}`;
    }
  });
</script>

{#if href}
  <a {href} class="inline-flex items-center gap-1 text-blue-400 hover:underline font-mono text-sm" title={value}>
    <span>{displayIcon}</span><span>{displayText}</span>
  </a>
{:else}
  <span class="inline-flex items-center gap-1 text-gray-300 font-mono text-sm" title={value}>
    <span>{displayIcon}</span><span>{displayText}</span>
  </span>
{/if}
