<script lang="ts">
  /**
   * Standard 4-zone entity page shell used by every /e/{type}/{id} page.
   * Zones: Identity, Relationships, Activity, Analytics. Any zone with no slot
   * content is simply omitted.
   */
  import type { Snippet } from 'svelte';

  interface Props {
    title: string;
    subtitle?: string;
    backHref?: string;
    backLabel?: string;
    identity?: Snippet;
    relationships?: Snippet;
    activity?: Snippet;
    analytics?: Snippet;
    loading?: boolean;
    error?: string | null;
  }

  let {
    title, subtitle, backHref = '/explorer', backLabel = 'Back to Explorer',
    identity, relationships, activity, analytics, loading = false, error = null,
  }: Props = $props();
</script>

<div class="max-w-6xl mx-auto px-4 py-8 space-y-6">
  <a href={backHref} class="inline-flex items-center gap-1.5 text-sm text-blue-400 hover:text-blue-300 transition-colors">
    <svg class="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
      <path stroke-linecap="round" stroke-linejoin="round" d="M15 19l-7-7 7-7" />
    </svg>
    {backLabel}
  </a>

  {#if loading}
    <div class="text-center py-16 text-gray-400">
      <div class="inline-block w-6 h-6 border-2 border-gray-500 border-t-blue-400 rounded-full animate-spin mb-3"></div>
      <p>Loading {title}...</p>
    </div>
  {:else if error}
    <div class="text-center py-16">
      <p class="text-2xl font-bold text-gray-300 mb-2">{title}</p>
      <p class="text-gray-500">{error}</p>
      <a href={backHref} class="inline-block mt-4 text-blue-400 hover:underline text-sm">{backLabel}</a>
    </div>
  {:else}
    <header class="space-y-1">
      <h1 class="text-2xl sm:text-3xl font-bold text-white">{title}</h1>
      {#if subtitle}<p class="text-sm text-gray-400">{subtitle}</p>{/if}
    </header>

    {#if identity}
      <section aria-label="Identity" class="space-y-3">
        {@render identity()}
      </section>
    {/if}

    {#if relationships}
      <section aria-label="Relationships" class="space-y-3">
        <h2 class="text-xs font-semibold text-gray-400 uppercase tracking-wider">Relationships</h2>
        {@render relationships()}
      </section>
    {/if}

    {#if activity}
      <section aria-label="Activity" class="space-y-3">
        <h2 class="text-xs font-semibold text-gray-400 uppercase tracking-wider">Activity</h2>
        {@render activity()}
      </section>
    {/if}

    {#if analytics}
      <section aria-label="Analytics" class="space-y-3">
        <h2 class="text-xs font-semibold text-gray-400 uppercase tracking-wider">Analytics</h2>
        {@render analytics()}
      </section>
    {/if}
  {/if}
</div>
