<script lang="ts">
  import { onMount } from 'svelte';
  import { getCurrentSeason, type CurrentSeason } from '$lib/services/currentSeasonService';
  import { currentSeasonMessage } from '$lib/utils/currentSeason';

  let data = $state<CurrentSeason | null>(null);
  let loading = $state(true);
  let now = $state(Date.now());
  let disposed = false;
  async function load(refresh = false) {
    loading = true;
    try {
      const result = await getCurrentSeason(refresh);
      if (!disposed) data = result;
    } catch {
      if (!disposed) data = null;
    } finally {
      if (!disposed) { loading = false; now = Date.now(); }
    }
  }
  onMount(() => {
    load();
    // Local clock only: no periodic canister polling.
    const clock = setInterval(() => { now = Date.now(); }, 60_000);
    return () => { disposed = true; clearInterval(clock); };
  });
</script>

<section class="current-season" aria-label="Season 2 status" aria-live="polite">
  <div>
    <strong>Season 2</strong>
    <p>{loading ? 'Checking current season…' : currentSeasonMessage(data, now)}</p>
    {#if data && !loading}<small>Checked {new Date(data.observedAt).toLocaleString()}.</small>{/if}
    <small>Season 1 totals are shown separately in <a href="/points">points history</a>.</small>
  </div>
  <button type="button" onclick={() => load(true)} disabled={loading}>Check again</button>
</section>

<style>
  .current-season { display: flex; align-items: center; justify-content: space-between; gap: 1rem; border: 1px solid rgba(52, 211, 153, .28); background: rgba(52, 211, 153, .06); border-radius: .625rem; padding: .625rem .875rem; margin-bottom: 1.25rem; font-size: .8125rem; }
  strong, a { color: #6ee7b7; }
  p { color: var(--rumi-text-primary); margin: .2rem 0; }
  small { color: var(--rumi-text-secondary); }
  button { flex-shrink: 0; color: #6ee7b7; border: 1px solid rgba(52, 211, 153, .28); border-radius: .375rem; padding: .35rem .5rem; }
  button:disabled { opacity: .5; }
  @media(max-width: 480px) { .current-season { align-items: flex-start; flex-direction: column; gap: .5rem; } }
</style>
