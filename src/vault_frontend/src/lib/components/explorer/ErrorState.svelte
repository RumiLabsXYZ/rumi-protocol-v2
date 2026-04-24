<script lang="ts">
  import type { Snippet } from 'svelte';

  interface Props {
    title?: string;
    message?: string;
    onRetry?: () => void | Promise<void>;
    retryLabel?: string;
    compact?: boolean;
    children?: Snippet;
  }

  let {
    title = 'Something went wrong',
    message = 'We could not load this data. The canister may be busy or briefly unavailable.',
    onRetry,
    retryLabel = 'Try again',
    compact = false,
    children,
  }: Props = $props();

  let retrying = $state(false);

  async function handleRetry() {
    if (!onRetry || retrying) return;
    retrying = true;
    try {
      await onRetry();
    } finally {
      retrying = false;
    }
  }
</script>

<div class="flex flex-col items-center justify-center text-center gap-2 text-gray-400 {compact ? 'py-6' : 'py-12'}">
  <div class="text-amber-400/80">
    <svg class="w-8 h-8" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="1.5" aria-hidden="true">
      <path stroke-linecap="round" stroke-linejoin="round" d="M12 9v3.75m-9.303 3.376c-.866 1.5.217 3.374 1.948 3.374h14.71c1.73 0 2.813-1.874 1.948-3.374L13.949 3.378c-.866-1.5-3.032-1.5-3.898 0L2.697 16.126zM12 15.75h.007v.008H12v-.008z" />
    </svg>
  </div>
  <p class="text-sm font-medium text-gray-300">{title}</p>
  <p class="text-sm text-gray-500 max-w-md">{message}</p>
  {#if onRetry}
    <button
      type="button"
      onclick={handleRetry}
      disabled={retrying}
      class="mt-2 inline-flex items-center gap-1.5 px-3 py-1.5 text-sm font-medium text-blue-300 hover:text-blue-200 bg-blue-500/10 hover:bg-blue-500/15 border border-blue-500/30 rounded-md transition-colors disabled:opacity-60 disabled:cursor-not-allowed"
    >
      {#if retrying}
        <span class="w-3.5 h-3.5 border-2 border-blue-300/30 border-t-blue-300 rounded-full animate-spin"></span>
        Retrying…
      {:else}
        <svg class="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2" aria-hidden="true">
          <path stroke-linecap="round" stroke-linejoin="round" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
        </svg>
        {retryLabel}
      {/if}
    </button>
  {/if}
  {#if children}<div class="mt-1">{@render children()}</div>{/if}
</div>
