<script lang="ts">
  interface Props {
    text: string;
    size?: 'sm' | 'md';
  }

  let { text, size = 'sm' }: Props = $props();

  let copied = $state(false);

  const iconSizes: Record<string, string> = { sm: 'w-3.5 h-3.5', md: 'w-4 h-4' };
  let iconSize = $derived(iconSizes[size]);

  async function copy() {
    try {
      await navigator.clipboard.writeText(text);
      copied = true;
      setTimeout(() => (copied = false), 2000);
    } catch {
      // Fallback for insecure contexts
      const ta = document.createElement('textarea');
      ta.value = text;
      document.body.appendChild(ta);
      ta.select();
      document.execCommand('copy');
      document.body.removeChild(ta);
      copied = true;
      setTimeout(() => (copied = false), 2000);
    }
  }
</script>

<button
  onclick={copy}
  class="text-gray-400 hover:text-gray-200 transition-colors cursor-pointer"
  title="Copy to clipboard"
>
  {#if copied}
    <svg class={iconSize} fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
      <path stroke-linecap="round" stroke-linejoin="round" d="M5 13l4 4L19 7" />
    </svg>
  {:else}
    <svg class={iconSize} fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
      <path stroke-linecap="round" stroke-linejoin="round" d="M8 16H6a2 2 0 01-2-2V6a2 2 0 012-2h8a2 2 0 012 2v2m-6 12h8a2 2 0 002-2v-8a2 2 0 00-2-2h-8a2 2 0 00-2 2v8a2 2 0 002 2z" />
    </svg>
  {/if}
</button>
