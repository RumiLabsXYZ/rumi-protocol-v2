<script lang="ts">
  let {
    onSearch,
    placeholder = 'Search vault ID, address, token, event #...',
  }: {
    onSearch: (query: string) => void;
    placeholder?: string;
  } = $props();

  let query = $state('');

  function submit() {
    onSearch(query);
    query = '';
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') submit();
  }
</script>

<div class="relative flex items-center">
  <input
    type="text"
    bind:value={query}
    onkeydown={handleKeydown}
    {placeholder}
    class="w-full rounded-lg border border-white/10 bg-white/5 py-2 pl-9 pr-3
           text-sm text-white placeholder-white/40 outline-none
           transition-colors focus:border-indigo-500 focus:ring-1 focus:ring-indigo-500/50"
  />
  <!-- Search icon inside the input -->
  <button
    onclick={submit}
    aria-label="Search"
    class="absolute left-2.5 flex items-center text-white/40 transition-colors hover:text-white/70"
  >
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" width="16" height="16">
      <circle cx="11" cy="11" r="8" /><path d="M21 21l-4.35-4.35" />
    </svg>
  </button>
</div>
