<script lang="ts">
  import { goto } from '$app/navigation';
  import { toastStore } from '$lib/stores/toast';

  let query = '';

  function handleSearch() {
    const q = query.trim();
    if (!q) return;

    // Pure numeric → vault ID
    if (/^\d+$/.test(q)) {
      goto(`/explorer/vault/${q}`);
      return;
    }

    // Event index prefixed with #
    if (/^#\d+$/.test(q)) {
      goto(`/explorer/event/${q.slice(1)}`);
      return;
    }

    // Principal-like (contains dashes, at least 10 chars)
    if (q.includes('-') && q.length >= 10) {
      goto(`/explorer/address/${q}`);
      return;
    }

    toastStore.error('Enter a vault ID (number), principal (with dashes), or event index (#number)');
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') handleSearch();
  }
</script>

<div class="explorer-search">
  <input
    type="text"
    class="icp-input search-input"
    bind:value={query}
    on:keydown={handleKeydown}
    placeholder="Search by vault ID, principal, or event index (#)"
  />
  <button class="icp-button search-btn" on:click={handleSearch}>
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" width="18" height="18">
      <circle cx="11" cy="11" r="8"/><path d="M21 21l-4.35-4.35"/>
    </svg>
  </button>
</div>

<style>
  .explorer-search { display:flex; gap:0.5rem; max-width:600px; width:100%; }
  .search-input { flex:1; font-size:0.9375rem; }
  .search-btn { display:flex; align-items:center; justify-content:center; padding:0.5rem 0.75rem; }
</style>
