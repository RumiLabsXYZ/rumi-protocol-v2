<script lang="ts">
  /**
   * SeasonBar — a slim, persistent airdrop bar shown at the top of every page's
   * content (the layout hides it on /points* to avoid duplicating the dashboard).
   * It adapts to the season phase and the connected wallet's enrollment so the
   * airdrop is impossible to miss while walking the app. Display-only; loads
   * cached public queries plus the shared myPointsStore.
   */
  import { onMount } from 'svelte';
  import { POINTS_ENABLED } from '$lib/config';
  import { isConnected, principal } from '$lib/stores/wallet';
  import { myPointsStore } from '$lib/stores/pointsStore';
  import { seasonStore, seasonPhase } from '$lib/stores/seasonStore';
  import { formatPoints, bodyState } from '$lib/utils/points';
  import { MAX_MULTIPLIER } from '$lib/utils/pointsRules';

  let loadedFor = $state<string | null>(null);

  onMount(() => {
    seasonStore.ensureLoaded();
  });

  // Load the connected principal's points once per principal (service is cached;
  // shared with the /points page). Reloads on wallet switch.
  $effect(() => {
    if (!POINTS_ENABLED) return;
    const p = $principal;
    if ($isConnected && p) {
      const t = p.toText();
      if (loadedFor !== t) {
        loadedFor = t;
        myPointsStore.load(p);
      }
    } else if (loadedFor !== null) {
      loadedFor = null;
    }
  });

  const phase = $derived($seasonPhase);
  const body = $derived(
    bodyState({ connected: $isConnected, excluded: $myPointsStore.excluded, state: $myPointsStore.state }),
  );
  const pts = $derived($myPointsStore.state ? formatPoints($myPointsStore.state.total_points) : null);
</script>

{#if POINTS_ENABLED && phase !== 'unknown' && body !== 'excluded'}
  <a class="season-bar" href="/points" aria-label="Airdrop points">
    <span class="sb-icon" aria-hidden="true">
      <svg viewBox="0 0 24 24" width="14" height="14" fill="currentColor"><path d="M12 2l2.2 6.2L20.5 10l-6.3 1.8L12 18l-1.8-6.2L4 10l6.2-1.8z" /></svg>
    </span>
    <span class="sb-tag">Season 1</span>
    <span class="sb-sep" aria-hidden="true">·</span>

    {#if phase === 'pre'}
      <span class="sb-msg">Airdrop starts soon — get positioned to earn up to {MAX_MULTIPLIER}×</span>
      <span class="sb-cta">See how →</span>
    {:else if phase === 'ended'}
      <span class="sb-msg">Season 1 has ended — allocations are being finalized</span>
      <span class="sb-cta">View →</span>
    {:else if body === 'enrolled' && pts !== null}
      <span class="sb-msg">You're earning — <strong class="sb-pts">{pts}</strong> points so far</span>
      <span class="sb-cta">View →</span>
    {:else if body === 'not_enrolled' && $myPointsStore.loaded}
      <span class="sb-msg">You're not earning yet — take any qualifying action to enroll</span>
      <span class="sb-cta">See how →</span>
    {:else}
      <span class="sb-msg">The airdrop is live — earn up to {MAX_MULTIPLIER}× points just by using Rumi</span>
      <span class="sb-cta">{$isConnected ? 'View →' : 'Connect to earn →'}</span>
    {/if}
  </a>
{/if}

<style>
  .season-bar {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.4375rem 0.875rem;
    margin-bottom: 1.25rem;
    border-radius: 0.625rem;
    border: 1px solid rgba(52, 211, 153, 0.28);
    background: rgba(52, 211, 153, 0.06);
    text-decoration: none;
    font-size: 0.8125rem;
    line-height: 1.3;
    transition: border-color 0.15s ease, background 0.15s ease;
  }
  .season-bar:hover {
    border-color: rgba(52, 211, 153, 0.5);
    background: rgba(52, 211, 153, 0.1);
  }
  .sb-icon { display: inline-flex; color: #34d399; flex-shrink: 0; }
  .sb-tag { color: #6ee7b7; font-weight: 700; letter-spacing: 0.01em; flex-shrink: 0; }
  .sb-sep { color: var(--rumi-text-muted); flex-shrink: 0; }
  .sb-msg {
    color: var(--rumi-text-secondary);
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sb-pts { color: var(--rumi-text-primary); font-weight: 700; }
  .sb-cta { color: #6ee7b7; font-weight: 600; flex-shrink: 0; white-space: nowrap; }

  @media (max-width: 768px) {
    .season-bar { font-size: 0.75rem; padding: 0.375rem 0.625rem; margin-bottom: 0.875rem; }
  }
</style>
