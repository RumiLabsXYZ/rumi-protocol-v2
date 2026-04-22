<script lang="ts">
  export interface SavedView {
    id: string;
    name: string;
    params: string; // URL search string including the leading '?'
  }

  interface Props {
    views: SavedView[];
    currentParams: string;
    onApply: (v: SavedView) => void;
    onRename: (id: string, name: string) => void;
    onDelete: (id: string) => void;
  }

  let { views, currentParams, onApply, onRename, onDelete }: Props = $props();

  let menuOpenFor: string | null = $state(null);

  function toggleMenu(id: string) {
    menuOpenFor = menuOpenFor === id ? null : id;
  }

  function close() {
    menuOpenFor = null;
  }

  function handleDocClick(ev: MouseEvent) {
    const target = ev.target as HTMLElement | null;
    if (!target?.closest('[data-saved-view]')) close();
  }

  $effect(() => {
    if (typeof document === 'undefined') return;
    document.addEventListener('click', handleDocClick);
    return () => document.removeEventListener('click', handleDocClick);
  });

  function promptRename(v: SavedView) {
    if (typeof window === 'undefined') return;
    const next = window.prompt('Rename saved view', v.name);
    if (next != null && next.trim() !== '' && next !== v.name) {
      onRename(v.id, next.trim());
    }
    close();
  }

  function confirmDelete(v: SavedView) {
    if (typeof window === 'undefined') return;
    if (window.confirm(`Delete saved view "${v.name}"?`)) {
      onDelete(v.id);
    }
    close();
  }
</script>

{#if views.length > 0}
  <div class="flex flex-wrap items-center gap-2">
    <span class="text-xs uppercase tracking-wider text-gray-500 mr-1">Saved views</span>
    {#each views as v (v.id)}
      {@const isCurrent = v.params === currentParams}
      <div class="relative" data-saved-view>
        <div
          class="inline-flex items-center gap-0 rounded-full border text-xs {isCurrent ? 'border-teal-500/40 bg-teal-500/15 text-teal-200' : 'border-gray-700 bg-gray-800/60 text-gray-200 hover:border-gray-500'}"
        >
          <button
            type="button"
            class="px-3 py-1 rounded-l-full"
            onclick={() => onApply(v)}
          >
            {v.name}
          </button>
          <button
            type="button"
            aria-label="View options"
            class="px-2 py-1 rounded-r-full border-l border-gray-700/70 hover:text-white"
            onclick={(e) => { e.stopPropagation(); toggleMenu(v.id); }}
          >
            ⋯
          </button>
        </div>
        {#if menuOpenFor === v.id}
          <div class="absolute left-0 top-full mt-1 z-20 min-w-[140px] bg-gray-900 border border-gray-700 rounded-md shadow-xl py-1 text-sm">
            <button type="button" class="block w-full text-left px-3 py-1 hover:bg-gray-800 text-gray-200" onclick={() => promptRename(v)}>
              Rename
            </button>
            <button type="button" class="block w-full text-left px-3 py-1 hover:bg-gray-800 text-red-400" onclick={() => confirmDelete(v)}>
              Delete
            </button>
          </div>
        {/if}
      </div>
    {/each}
  </div>
{/if}
