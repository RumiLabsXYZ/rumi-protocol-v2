<script lang="ts">
  interface Props {
    label: string;
    value: string;
    subtitle?: string;
    trend?: 'up' | 'down' | 'neutral';
    href?: string;
  }

  let { label, value, subtitle, trend, href }: Props = $props();
</script>

{#if href}
  <a {href} class="block bg-gray-800/50 border border-gray-700/50 rounded-xl p-5 hover:border-gray-600/50 transition-colors">
    {@render content()}
  </a>
{:else}
  <div class="bg-gray-800/50 border border-gray-700/50 rounded-xl p-5">
    {@render content()}
  </div>
{/if}

{#snippet content()}
  <p class="text-sm text-gray-400 mb-1">{label}</p>
  <p class="text-2xl font-semibold text-white">{value}</p>
  {#if subtitle}
    <p class="text-xs mt-1 {trend === 'up' ? 'text-green-400' : trend === 'down' ? 'text-red-400' : 'text-gray-500'}">
      {subtitle}
    </p>
  {/if}
{/snippet}
