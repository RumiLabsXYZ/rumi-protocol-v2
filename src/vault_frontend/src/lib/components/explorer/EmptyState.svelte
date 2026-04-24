<script lang="ts">
  import type { Snippet } from 'svelte';

  interface Props {
    title?: string;
    message?: string;
    icon?: 'inbox' | 'search' | 'chart';
    compact?: boolean;
    children?: Snippet;
  }

  let {
    title,
    message = 'Nothing to show yet.',
    icon = 'inbox',
    compact = false,
    children,
  }: Props = $props();
</script>

<div class="flex flex-col items-center justify-center text-center gap-2 text-gray-500 {compact ? 'py-6' : 'py-12'}">
  <div class="text-gray-600">
    {#if icon === 'inbox'}
      <svg class="w-8 h-8" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="1.5" aria-hidden="true">
        <path stroke-linecap="round" stroke-linejoin="round" d="M20 13V6a2 2 0 00-2-2H6a2 2 0 00-2 2v7m16 0v5a2 2 0 01-2 2H6a2 2 0 01-2-2v-5m16 0h-4l-2 2H10l-2-2H4" />
      </svg>
    {:else if icon === 'search'}
      <svg class="w-8 h-8" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="1.5" aria-hidden="true">
        <path stroke-linecap="round" stroke-linejoin="round" d="M21 21l-4.35-4.35M10.5 18a7.5 7.5 0 100-15 7.5 7.5 0 000 15z" />
      </svg>
    {:else}
      <svg class="w-8 h-8" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="1.5" aria-hidden="true">
        <path stroke-linecap="round" stroke-linejoin="round" d="M9 19V6l3 2 3-2v13M5 21h14a2 2 0 002-2V5a2 2 0 00-2-2H5a2 2 0 00-2 2v14a2 2 0 002 2z" />
      </svg>
    {/if}
  </div>
  {#if title}<p class="text-sm font-medium text-gray-400">{title}</p>{/if}
  <p class="text-sm">{message}</p>
  {#if children}<div class="mt-1">{@render children()}</div>{/if}
</div>
