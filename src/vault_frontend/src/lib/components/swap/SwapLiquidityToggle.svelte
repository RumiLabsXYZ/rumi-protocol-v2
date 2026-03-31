<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  export let mode: 'swap' | 'liquidity' = 'swap';
  const dispatch = createEventDispatcher();

  function setMode(m: 'swap' | 'liquidity') {
    mode = m;
    dispatch('change', { mode: m });
  }
</script>

<div class="toggle-bar">
  <button
    class="toggle-btn"
    class:active={mode === 'swap'}
    on:click={() => setMode('swap')}
  >
    Swap
  </button>
  <button
    class="toggle-btn"
    class:active={mode === 'liquidity'}
    on:click={() => setMode('liquidity')}
  >
    Liquidity
  </button>
</div>

<style>
  .toggle-bar {
    display: flex;
    gap: 0.25rem;
    padding: 0.1875rem;
    background: var(--rumi-bg-surface2);
    border: 1px solid var(--rumi-border);
    border-radius: 0.5rem;
    margin-bottom: 1.25rem;
  }

  .toggle-btn {
    flex: 1;
    padding: 0.5rem 0;
    border: none;
    border-radius: 0.375rem;
    font-family: 'Inter', sans-serif;
    font-size: 0.8125rem;
    font-weight: 500;
    color: var(--rumi-text-muted);
    background: transparent;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .toggle-btn:hover:not(.active) {
    color: var(--rumi-text-secondary);
  }

  .toggle-btn.active {
    background: var(--rumi-bg-surface1);
    color: var(--rumi-text-primary);
    font-weight: 600;
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.15);
  }
</style>
