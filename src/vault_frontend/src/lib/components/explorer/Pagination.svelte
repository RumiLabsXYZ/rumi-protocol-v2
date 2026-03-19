<script lang="ts">
  export let currentPage: number = 0;
  export let totalPages: number = 1;
  export let onPageChange: (page: number) => void = () => {};

  $: pages = getVisiblePages(currentPage, totalPages);

  function getVisiblePages(current: number, total: number): (number | '...')[] {
    if (total <= 7) return Array.from({ length: total }, (_, i) => i);
    const result: (number | '...')[] = [0];
    if (current > 2) result.push('...');
    for (let i = Math.max(1, current - 1); i <= Math.min(total - 2, current + 1); i++) {
      result.push(i);
    }
    if (current < total - 3) result.push('...');
    result.push(total - 1);
    return result;
  }
</script>

{#if totalPages > 1}
<nav class="pagination">
  <button class="page-btn" disabled={currentPage === 0} on:click={() => onPageChange(currentPage - 1)}>
    ← Prev
  </button>
  {#each pages as p}
    {#if p === '...'}
      <span class="page-ellipsis">…</span>
    {:else}
      <button
        class="page-btn"
        class:active={p === currentPage}
        on:click={() => onPageChange(p as number)}
      >{(p as number) + 1}</button>
    {/if}
  {/each}
  <button class="page-btn" disabled={currentPage >= totalPages - 1} on:click={() => onPageChange(currentPage + 1)}>
    Next →
  </button>
</nav>
{/if}

<style>
  .pagination { display:flex; gap:0.25rem; align-items:center; justify-content:center; margin-top:1.5rem; }
  .page-btn {
    padding:0.375rem 0.625rem; font-size:0.8125rem; border:1px solid var(--rumi-border);
    border-radius:0.375rem; background:transparent; color:var(--rumi-text-secondary);
    cursor:pointer; transition:all 0.15s;
  }
  .page-btn:hover:not(:disabled) { background:var(--rumi-bg-surface-2); border-color:var(--rumi-border-hover); }
  .page-btn.active { background:var(--rumi-purple-accent); color:white; border-color:var(--rumi-purple-accent); }
  .page-btn:disabled { opacity:0.4; cursor:not-allowed; }
  .page-ellipsis { padding:0 0.25rem; color:var(--rumi-text-muted); }
</style>
