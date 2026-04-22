<script lang="ts">
  import type { LensId } from './lenses/lensTypes';

  interface Props {
    active: LensId;
    setLens: (lens: LensId) => void;
  }
  let { active, setLens }: Props = $props();

  const LENSES: { id: LensId; label: string }[] = [
    { id: 'overview', label: 'Overview' },
    { id: 'collateral', label: 'Collateral' },
    { id: 'stability', label: 'Stability Pool' },
    { id: 'redemptions', label: 'Redemptions' },
    { id: 'revenue', label: 'Revenue' },
    { id: 'dexs', label: 'DEXs' },
    { id: 'admin', label: 'Admin' },
  ];
</script>

<div class="lens-tabs">
  <div class="flex items-center gap-1 overflow-x-auto no-scrollbar">
    <span class="text-[11px] uppercase tracking-[0.12em] font-medium mr-3 flex-shrink-0" style="color: var(--rumi-text-secondary);">Protocol</span>
    {#each LENSES as lens}
      <button
        type="button"
        class="lens-tab flex-shrink-0"
        class:active={active === lens.id}
        onclick={() => setLens(lens.id)}
      >
        {lens.label}
      </button>
    {/each}
  </div>
</div>

<style>
  .lens-tabs {
    padding: 0.25rem 0;
    border-bottom: 1px solid var(--rumi-border);
    margin-bottom: 1rem;
  }
  .lens-tab {
    position: relative;
    padding: 0.5rem 0.9rem;
    font-size: 0.8125rem;
    font-weight: 500;
    color: var(--rumi-text-secondary);
    background: transparent;
    border: none;
    border-radius: 0.375rem;
    cursor: pointer;
    transition: color 120ms ease, background 120ms ease;
    white-space: nowrap;
  }
  .lens-tab:hover {
    color: var(--rumi-text-primary);
    background: var(--rumi-bg-surface2);
  }
  .lens-tab.active {
    color: var(--rumi-teal);
    background: var(--rumi-teal-dim);
  }
  .lens-tab.active::after {
    content: '';
    position: absolute;
    left: 0.75rem;
    right: 0.75rem;
    bottom: -0.3125rem;
    height: 2px;
    background: var(--rumi-teal);
    border-radius: 1px;
  }
  .no-scrollbar::-webkit-scrollbar { display: none; }
  .no-scrollbar { scrollbar-width: none; }
</style>
